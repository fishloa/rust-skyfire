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
    finished: bool,
}

/// Upper bound on samples buffered while waiting for the first video RAP /
/// TracksResolved — prevents unbounded growth on a stream that never resolves.
const MAX_PREBUFFER_SAMPLES: usize = 4096;

impl HlsSession {
    #[must_use]
    pub fn new(cfg: HlsConfig) -> Self {
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
            finished: false,
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.segments.is_empty()
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
        if let Some(seg) = self.seg.as_mut()
            && let Ok(Some(ts)) = seg.finish()
        {
            Self::store(
                &mut self.segments,
                &mut self.next_seq,
                &mut self.media_sequence,
                &self.cfg,
                ts,
            );
        }
    }

    #[must_use]
    pub fn playlist(&self) -> String {
        let target = self
            .segments
            .iter()
            .map(|s| s.duration.ceil() as u64)
            .max()
            .unwrap_or(u64::from(self.cfg.target_secs))
            .max(1);
        let mut out = String::new();
        out.push_str("#EXTM3U\n#EXT-X-VERSION:3\n");
        out.push_str(&format!("#EXT-X-TARGETDURATION:{target}\n"));
        out.push_str(&format!("#EXT-X-MEDIA-SEQUENCE:{}\n", self.media_sequence));
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
                    let tid = track.spec.track_id;
                    if !self.known_track_ids.contains(&tid) {
                        self.known_track_ids.push(tid);
                        if matches!(track_meta(&track.spec).kind, TrackKind::Video(_)) {
                            self.video_track_id.get_or_insert(tid);
                        }
                        if self.seg.is_none() {
                            self.pending_specs.push(track.spec.clone());
                        } else if let Some(seg) = self.seg.as_mut() {
                            let _ = seg.add_track(track.spec.clone());
                        }
                    }
                }
                DemuxEvent::TrackUpdated(_) => {}
                DemuxEvent::Sample { track_id, sample } => {
                    if self.seg.is_some() {
                        self.push_sample(track_id, sample);
                    } else if !self.buffer_capped {
                        self.buffer.push((track_id, sample));
                        if self.buffer.len() >= MAX_PREBUFFER_SAMPLES {
                            self.buffer_capped = true;
                        }
                        self.try_build();
                    }
                }
                DemuxEvent::TracksResolved => {
                    self.tracks_resolved = true;
                    self.try_build();
                }
                DemuxEvent::Discontinuity { .. } => {
                    if let Some(seg) = self.seg.as_mut() {
                        seg.mark_discontinuity();
                    }
                }
                DemuxEvent::Pcr(_) => {}
                _ => {}
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
            .position(|(tid, s)| *tid == vid && s.is_sync)
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
        if let Some(seg) = self.seg.as_mut()
            && let Ok(Some(ts)) = seg.push(track_id, sample)
        {
            Self::store(
                &mut self.segments,
                &mut self.next_seq,
                &mut self.media_sequence,
                &self.cfg,
                ts,
            );
        }
    }

    fn store(
        segments: &mut VecDeque<StoredSegment>,
        next_seq: &mut u64,
        media_sequence: &mut u64,
        cfg: &HlsConfig,
        ts: transmux::ts_hls::TsSegment,
    ) {
        let name = format!("{}{}.ts", cfg.uri_prefix, *next_seq);
        *next_seq += 1;
        segments.push_back(StoredSegment {
            name,
            bytes: Arc::new(ts.bytes),
            duration: ts.duration,
            discontinuous: ts.discontinuous,
        });
        if let Some(window) = cfg.window {
            while segments.len() > window {
                segments.pop_front();
                *media_sequence += 1;
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
}
