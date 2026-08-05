//! Source-agnostic HLS-of-TS orchestration. Feed raw MPEG-TS bytes; poll a
//! rolling or VOD playlist plus keyframe-aligned `.ts` segments. Wraps
//! `transmux::ts_hls::StreamingTsHlsSegmenter` (the chop) with the
//! build-on-`TracksResolved` + RAP-trim orchestration; owns no HTTP, no async.

use std::collections::VecDeque;
use std::sync::Arc;

use skyfire_ts::{DemuxEvent, TrackKind, TsDemux, track_meta};
use transmux::ts_hls::StreamingTsHlsSegmenter;

/// How a session segments and windows.
#[derive(Debug, Clone)]
pub struct HlsConfig {
    /// Target segment duration in seconds (segments cut on the first video RAP
    /// at/after this). zenith uses 4.
    pub target_secs: u32,
    /// `None` = VOD (retain every segment; append `#EXT-X-ENDLIST` on `finish`).
    /// `Some(n)` = rolling media playlist of at most `n` segments.
    pub window: Option<usize>,
    /// Segment filename prefix; segment `k` is `"{uri_prefix}{k}.ts"`.
    pub uri_prefix: String,
}

impl HlsConfig {
    #[must_use]
    pub fn vod() -> Self {
        Self {
            target_secs: 4,
            window: None,
            uri_prefix: "seg".to_string(),
        }
    }
    #[must_use]
    pub fn rolling(window: usize) -> Self {
        Self {
            target_secs: 4,
            window: Some(window.max(1)),
            uri_prefix: "seg".to_string(),
        }
    }
}

/// A committed segment retained for serving + playlist generation.
#[derive(Clone)]
pub struct StoredSegment {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub duration: f64,
    pub discontinuous: bool,
}

/// Incremental HLS-of-TS session. See crate docs.
pub struct HlsSession {
    cfg: HlsConfig,
    demux: TsDemux,
    seg: Option<StreamingTsHlsSegmenter>,
    // Track specs collected from TrackAdded, in arrival order.
    pending_specs: Vec<transmux::TrackSpec>,
    known_track_ids: Vec<u32>,
    video_track_id: Option<u32>,
    tracks_resolved: bool,
    // Samples buffered before the segmenter is built, in arrival order.
    buffer: Vec<(u32, transmux::Sample)>,
    buffer_capped: bool,
    // Committed segments (retained fully for VOD; trimmed to window for rolling).
    segments: VecDeque<StoredSegment>,
    next_seq: u64,
    media_sequence: u64, // first retained segment's sequence (rolling eviction)
    // Monotonic non-decreasing max of `ceil(duration)` across ALL segments
    // ever produced — never shrinks when a long segment rolls off the window.
    target_duration: u64,
    // Number of discontinuous segments that have rolled OFF the front of
    // a rolling playlist (RFC 8216 §4.3.3.3).
    discontinuity_sequence: u64,
    finished: bool,
    /// Number of `transmux` segmenter errors observed since construction
    /// (`add_track`/`push`/`finish` returning `Err`) — issue #101 review,
    /// item 3: these used to be silently swallowed via `let _ = ...`, which
    /// backs `skyfire-server`'s HLS-of-TS and could silently degrade to a
    /// short/empty playlist with no diagnostic at all.
    segmenter_error_count: u64,
    /// Number of `DiscontinuityKind::TimelineReanchored` events observed
    /// since construction — issue #101 review, item 1: this arm was
    /// previously completely silent, so a field report could never confirm
    /// whether it ever fires. See the match arm in `drain_events` for why
    /// it deliberately does not mark the segmenter discontinuous.
    timeline_reanchor_count: u64,
}

/// Upper bound on samples buffered while waiting for the first video RAP /
/// TracksResolved — prevents unbounded growth on a stream that never resolves.
const MAX_PREBUFFER_SAMPLES: usize = 4096;

impl HlsSession {
    #[must_use]
    pub fn new(cfg: HlsConfig) -> Self {
        let default_td = cfg.target_secs as u64;
        Self {
            cfg,
            demux: TsDemux::new(),
            seg: None,
            pending_specs: Vec::new(),
            known_track_ids: Vec::new(),
            video_track_id: None,
            tracks_resolved: false,
            buffer: Vec::new(),
            buffer_capped: false,
            segments: VecDeque::new(),
            next_seq: 0,
            media_sequence: 0,
            target_duration: default_td,
            discontinuity_sequence: 0,
            finished: false,
            segmenter_error_count: 0,
            timeline_reanchor_count: 0,
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.segments.is_empty()
    }

    /// Number of `transmux` segmenter errors (`add_track`/`push`/`finish`
    /// failures) observed since construction. JS-observable analogue: see
    /// `skyfire-wasm`'s `SkyfireBridge::segmenter_error_count`.
    #[must_use]
    pub const fn segmenter_error_count(&self) -> u64 {
        self.segmenter_error_count
    }

    /// Number of `DiscontinuityKind::TimelineReanchored` events observed
    /// since construction (>20ms audio dts/pts re-anchor; see the
    /// `drain_events` match arm for why the segmenter is deliberately not
    /// marked discontinuous for these). JS-observable analogue: see
    /// `skyfire-wasm`'s `SkyfireBridge::timeline_reanchor_count`.
    #[must_use]
    pub const fn timeline_reanchor_count(&self) -> u64 {
        self.timeline_reanchor_count
    }

    #[must_use]
    pub fn segment(&self, name: &str) -> Option<Arc<Vec<u8>>> {
        self.segments
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.bytes.clone())
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.demux.feed(data);
        self.drain_events();
    }

    pub fn finish(&mut self) {
        self.finished = true;
        self.demux.finish();
        self.drain_events();
        if let Some(seg) = self.seg.as_mut() {
            if let Err(e) = seg.finish() {
                self.segmenter_error_count += 1;
                std::eprintln!("[skyfire-hls] segmenter finish error: {e}");
            }
            for ts in seg.take_ready() {
                Self::store(
                    &mut self.segments,
                    &mut self.next_seq,
                    &mut self.media_sequence,
                    &mut self.target_duration,
                    &mut self.discontinuity_sequence,
                    &self.cfg,
                    ts,
                );
            }
        }
    }

    #[must_use]
    pub fn playlist(&self) -> String {
        let target = self.target_duration.max(1);
        let mut out = String::new();
        out.push_str("#EXTM3U\n#EXT-X-VERSION:3\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{}\n", self.media_sequence));
        if self.cfg.window.is_some() && self.discontinuity_sequence > 0 {
            out.push_str(&format!(
                "#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
                self.discontinuity_sequence
            ));
        }
        if self.cfg.window.is_none() {
            out.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
        }
        out.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");
        for s in &self.segments {
            if s.discontinuous {
                out.push_str("#EXT-X-DISCONTINUITY\n");
            }
            out.push_str(&format!("#EXTINF:{:.6},\n{}\n", s.duration, s.name));
        }
        if self.cfg.window.is_none() && self.finished {
            out.push_str("#EXT-X-ENDLIST\n");
        }
        out
    }

    // ── private helpers ────────────────────────────────────────

    fn drain_events(&mut self) {
        while let Some(ev) = self.demux.poll_event() {
            match ev {
                DemuxEvent::TrackAdded(track) => {
                    let tid = track.track_id;
                    if !self.known_track_ids.contains(&tid) {
                        self.known_track_ids.push(tid);
                        if matches!(track_meta(&track).kind, TrackKind::Video(_)) {
                            self.video_track_id.get_or_insert(tid);
                        }
                        if self.seg.is_none() {
                            self.pending_specs.push(track.clone());
                        } else if let Some(seg) = self.seg.as_mut()
                            && let Err(e) = seg.add_track(track.clone())
                        {
                            self.segmenter_error_count += 1;
                            std::eprintln!("[skyfire-hls] segmenter add_track error: {e}");
                        }
                    }
                }
                DemuxEvent::TrackUpdated(_) => {}
                DemuxEvent::Sample {
                    track_id, sample, ..
                } => {
                    if self.seg.is_some() {
                        self.push_sample(track_id, sample);
                    } else {
                        let is_video = self.video_track_id.is_some_and(|v| v == track_id);
                        if !self.buffer_capped || is_video {
                            // Once capped (MAX_PREBUFFER_SAMPLES hit before
                            // TracksResolved + first RAP), keep only video
                            // samples so that a late sync-frame can still build
                            // the segmenter.  Non-video samples are dropped.
                            self.buffer.push((track_id, sample));
                            if self.buffer.len() >= MAX_PREBUFFER_SAMPLES * 2 {
                                // Hard guard: if even a capped video-only buffer
                                // grows beyond reason, drain from front.
                                self.buffer.remove(0);
                            }
                        }
                        if !self.buffer_capped && self.buffer.len() >= MAX_PREBUFFER_SAMPLES {
                            self.buffer_capped = true;
                        }
                        self.try_build();
                    }
                }
                DemuxEvent::TracksResolved { .. } => {
                    self.tracks_resolved = true;
                    self.try_build();
                }
                DemuxEvent::Discontinuity { kind, .. } => {
                    // Per-kind decision (issue #101 review, item 2; mirrors
                    // skyfire-wasm's `SkyfireBridge::drain_events`, whose
                    // per-arm reasoning this repeats for the HLS-of-TS path):
                    // `mark_discontinuity()` here means "emit
                    // #EXT-X-DISCONTINUITY before the next segment" (RFC 8216
                    // §4.3.2.3) — telling every HLS client the bitstream may
                    // not be contiguous with what came before, which is only
                    // true for a real splice/data-loss cause.
                    match kind {
                        transmux::DiscontinuityKind::Signalled => {
                            // Explicit adaptation-field discontinuity_indicator
                            // (ISO/IEC 13818-1 §2.4.3.5) — a genuine
                            // splice/encoder restart. Keep today's behaviour.
                            if let Some(seg) = self.seg.as_mut() {
                                seg.mark_discontinuity();
                            }
                        }
                        transmux::DiscontinuityKind::TimelineReanchored => {
                            // Corrected per #101 review, item 1: upstream
                            // transmux 0.20 does NOT classify this as
                            // ordinary drift absorption — it is the opposite.
                            // `transmux::ir::event::DiscontinuityKind::
                            // TimelineReanchored`'s own doc: the live audio
                            // anchor "drifted from the wire PES clock past
                            // the re-anchor threshold and was re-anchored —
                            // a genuine gap (splice, encoder restart), not
                            // the 90 kHz/sample-rate rounding drift ... which
                            // the anchor absorbs silently". `ts_demux`'s
                            // `AudioAnchor` re-anchor logic only fires "when
                            // the wire clock drifts ... by more than
                            // audio_discontinuity_threshold_90k, a genuine
                            // gap" — a threshold deliberately set at 20 ms /
                            // 1800 ticks @ 90 kHz specifically so ordinary
                            // muxer rounding noise never reaches it (see that
                            // constant's own derivation doc).
                            //
                            // So this event IS upstream's splice/encoder-
                            // restart/PID-reuse signal, not muxer noise. We
                            // still choose NOT to mark_discontinuity() here —
                            // an accepted risk, not a misreading of the
                            // event: `DiscontinuityKind` carries no drift
                            // magnitude, so this call site cannot tell "just
                            // over the 20 ms line" from "genuinely spliced
                            // stream", and marking every one of these
                            // discontinuous on a long-running live feed would
                            // fragment the playlist (forcing every
                            // downstream HLS client through an unnecessary
                            // decoder reset) far more often than a real
                            // splice occurs. A muxer restart that *does* set
                            // the adaptation-field discontinuity_indicator is
                            // already caught by `Signalled` above.
                            //
                            // Accepted residual risk: on a live feed whose
                            // muxer does not set discontinuity_indicator
                            // across an encoder restart/PID reuse, this path
                            // emits no #EXT-X-DISCONTINUITY for that seam, so
                            // a downstream player's own AC-3/E-AC-3 decoder
                            // keeps its IMDCT overlap-add state running
                            // across it (possible audible glitch on that
                            // client) — this crate holds no audio decoder of
                            // its own to reset. We accept that over marking
                            // every ordinary >20ms wobble discontinuous.
                            // Each `Sample`'s dts/pts is already the
                            // corrected, absolute value (media plane step
                            // 2c) by the time it reaches the segmenter
                            // regardless.
                            self.timeline_reanchor_count += 1;
                            std::eprintln!(
                                "[skyfire-hls] discontinuity: TimelineReanchored \
                                 (audio dts/pts re-anchored, >20ms drift; per \
                                 accepted risk, segmenter not marked discontinuous)"
                            );
                        }
                        transmux::DiscontinuityKind::BudgetExceeded { bytes } => {
                            // A per-PID PES buffer cap was tripped and
                            // in-flight payload was dropped — real data loss,
                            // not just a timeline correction. Treat like
                            // `Signalled`.
                            std::eprintln!(
                                "[skyfire-hls] discontinuity: PES budget exceeded, \
                                 {bytes} bytes dropped"
                            );
                            if let Some(seg) = self.seg.as_mut() {
                                seg.mark_discontinuity();
                            }
                        }
                        // `#[non_exhaustive]`: default new/unknown kinds to
                        // the conservative, mark-discontinuous behaviour —
                        // the safe assumption is "may not be contiguous",
                        // never the `TimelineReanchored` exemption, which is
                        // earned per known cause, not a default.
                        _ => {
                            if let Some(seg) = self.seg.as_mut() {
                                seg.mark_discontinuity();
                            }
                        }
                    }
                }
                DemuxEvent::ClockReference { .. } => {}
                // Explicit, not silently swallowed (#103): these three were
                // caught by the old `_ => {}` and vanished. `TrackRemoved` and
                // `TrackAbandoned` need no state change here — the HLS
                // segmenter holds only per-`TrackSpec` entries it pushes
                // samples into as they arrive, and a removed/abandoned PID
                // simply stops producing samples, so no stale reference can
                // corrupt a future `Sample`. `InputDegraded` (transport-error
                // / continuity-gap) carries no payload this crate re-muxes —
                // it is an operational metric, so we log it rather than act
                // on it (this crate holds no audio/VIS decoders to reset).
                DemuxEvent::TrackRemoved { track_id, .. } => {
                    std::eprintln!("[skyfire-hls] track removed: track_id={track_id} (no-op)");
                }
                DemuxEvent::TrackAbandoned {
                    track_id, reason, ..
                } => {
                    std::eprintln!(
                        "[skyfire-hls] track abandoned: track_id={track_id:?} \
                         reason={reason} (no-op)"
                    );
                }
                DemuxEvent::InputDegraded { kind, .. } => {
                    std::eprintln!(
                        "[skyfire-hls] input degraded: {kind} (no-op, operational metric)"
                    );
                }
                // A genuinely new `#[non_exhaustive]` `DemuxEvent` variant from
                // a future transmux: logged here instead of vanishing (#103).
                _ => {
                    std::eprintln!(
                        "[skyfire-hls] unrecognised DemuxEvent variant (future \
                         transmux #[non_exhaustive] addition)"
                    );
                }
            }
        }
    }

    fn try_build(&mut self) {
        if self.seg.is_some() || self.pending_specs.is_empty() {
            return;
        }
        if !self.tracks_resolved && !self.buffer_capped {
            return;
        }
        let Some(vid) = self.video_track_id else {
            return;
        };
        let Some(rap_idx) = self
            .buffer
            .iter()
            .position(|(tid, s)| *tid == vid && s.flags.is_sync)
        else {
            return;
        };

        let seg = match StreamingTsHlsSegmenter::new(
            self.pending_specs.clone(),
            self.cfg.target_secs,
            self.cfg.window.unwrap_or(6).max(1),
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        self.seg = Some(seg);

        let replay: Vec<(u32, transmux::Sample)> = self.buffer.split_off(rap_idx);
        self.buffer.clear();
        for (tid, s) in replay {
            self.push_sample(tid, s);
        }
    }

    fn push_sample(&mut self, track_id: u32, sample: transmux::Sample) {
        if let Some(seg) = self.seg.as_mut() {
            // Any segment this push cuts is queued (not returned inline) —
            // transmux 0.20 (issue R2) removed the `Result<Option<TsSegment>>`
            // inline return since `finish` is both an inherent and a `Stage`
            // trait method with the same name/arity, so an unqualified
            // `finish()` call always resolved to the inherent one; retrieval
            // is now exclusively via `take_ready()`.
            if let Err(e) = seg.push(track_id, sample) {
                self.segmenter_error_count += 1;
                std::eprintln!("[skyfire-hls] segmenter push error: {e}");
            }
            for ts in seg.take_ready() {
                Self::store(
                    &mut self.segments,
                    &mut self.next_seq,
                    &mut self.media_sequence,
                    &mut self.target_duration,
                    &mut self.discontinuity_sequence,
                    &self.cfg,
                    ts,
                );
            }
        }
    }

    fn store(
        segments: &mut VecDeque<StoredSegment>,
        next_seq: &mut u64,
        media_sequence: &mut u64,
        target_duration: &mut u64,
        discontinuity_sequence: &mut u64,
        cfg: &HlsConfig,
        ts: transmux::ts_hls::TsSegment,
    ) {
        let name = format!("{}{}.ts", cfg.uri_prefix, *next_seq);
        *next_seq += 1;
        let dur_ceil = ts.duration.ceil() as u64;
        *target_duration = (*target_duration).max(dur_ceil);
        segments.push_back(StoredSegment {
            name,
            bytes: Arc::new(ts.bytes),
            duration: ts.duration,
            discontinuous: ts.discontinuous,
        });
        if let Some(window) = cfg.window {
            while segments.len() > window {
                let evicted = segments.pop_front().unwrap();
                *media_sequence += 1;
                if evicted.discontinuous {
                    *discontinuity_sequence += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vod_session_is_not_ready_and_has_no_segments() {
        let s = HlsSession::new(HlsConfig::vod());
        assert!(!s.is_ready(), "no segments fed yet");
        assert_eq!(s.segment("seg0.ts"), None);
        // Playlist before any segment: a header, no segment lines, no ENDLIST yet.
        let pl = s.playlist();
        assert!(
            pl.starts_with("#EXTM3U"),
            "playlist must start with #EXTM3U"
        );
        assert!(!pl.contains(".ts"), "no segment URIs before any segment");
    }

    /// Prove a session still builds when the first video RAP arrives after the
    /// pre-buffer cap has been hit (many non-RAP samples arrive first).
    /// Feed the fixture 1 byte at a time: PAT/PMT/SDT arrive first, then video
    /// PES — the segmenter must eventually build even if the cap was hit.
    #[test]
    fn build_after_buffer_capped_with_late_rap() {
        let mut s = HlsSession::new(HlsConfig {
            target_secs: 1,
            window: None,
            uri_prefix: "seg".into(),
        });
        let data = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/france2-8s.ts"),
        )
        .unwrap();
        for b in data {
            s.feed(&[b]);
        }
        s.finish();
        assert!(
            s.is_ready(),
            "session must produce segments even when first RAP arrives late"
        );
        assert!(
            s.playlist().lines().filter(|l| l.ends_with(".ts")).count() >= 1,
            "playlist must list at least one segment"
        );
    }
}
