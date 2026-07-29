use broadcast_common::traits::Parse;
use skyfire_ts::{AudioCodec, DemuxEvent, SubtitleKind, TrackKind, TrackMeta};
use skyfire_ts::{audio_codec_str, video_codec_str};
use transmux::DiscontinuityKind;
use wasm_bindgen::prelude::*;

use crate::bridge_dto::{
    CachedVideoConfig, WasmAudioTrack, WasmMediaSegment, WasmPcmChunk, WasmSubtitleCue,
    WasmSubtitleRegion, WasmSubtitleTrack, WasmTrackList, WasmVideoAu,
};
use crate::helpers::{
    lang_bytes_to_string, parse_base_media_decode_time, parse_sample_count_from_segment,
};

/// Streaming WASM bridge between the browser and the Skyfire demux engine.
///
/// Unlike [`WasmEngine`](crate::probe::WasmEngine) (which requires probe→init→feed→finalize), this
/// struct is designed for real-time streaming:
///
/// 1. Construct with `SkyfireBridge::new()`.
/// 2. Call `feed(chunk)` repeatedly as TS data arrives over `fetch()`.
/// 3. Poll `track_list()` until it becomes `Some` (PAT+PMT have been parsed).
/// 4. Call `take_video_aus()` each tick to drain pending video access units.
/// 5. Use `pcr_pts()` for the A/V sync clock.
#[wasm_bindgen]
pub struct SkyfireBridge {
    demux: skyfire_ts::TsDemux,
    /// track_id → TrackMeta
    tracks: std::collections::HashMap<u32, TrackMeta>,
    /// Track ID of the first video track seen.
    video_track_id: Option<u32>,
    /// Cached WebCodecs video config.
    cached_video_config: Option<CachedVideoConfig>,
    /// Selected audio PID.
    selected_audio_pid: Option<u16>,
    /// Channel count per audio PID, from a header probe of the first frame.
    ///
    /// First value wins and is never invalidated (see `probe_channels`'s
    /// `contains_key` guard): a broadcast that switches its channel layout
    /// mid-stream (e.g. 5.1 -> 2.0) keeps reporting the original count.
    audio_channels: std::collections::BTreeMap<u16, u8>,
    /// Selected subtitle PID.
    selected_subtitle_pid: Option<u16>,
    /// Play/pause state.
    playing: bool,
    /// Accumulated video AUs (already AVCC length-prefixed from transmux).
    video_aus: Vec<WasmVideoAu>,
    /// MSE segmenter (lazy-built on first video TrackAdded).
    segmenter: Option<transmux::Segmenter>,
    /// Ready media segment bytes, pending drain.
    ready_segments: std::collections::VecDeque<Vec<u8>>,
    /// AC-3/E-AC-3 incremental decoder.
    audio_decoder: skyfire_ac3::IncrementalDecoder,
    /// MPEG-1/2 Layer II incremental decoder.
    mpa_decoder: skyfire_mpa::IncrementalMpaDecoder,
    /// PCM chunks pending drain.
    audio_pcm_pending: Vec<WasmPcmChunk>,
    /// Subtitle compositor.
    subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState,
    /// Subtitle cues pending drain.
    subtitle_cues_pending: Vec<WasmSubtitleCue>,
    /// Latest PCR/PTS value in 90 kHz ticks.
    latest_pcr: Option<i64>,
    /// Whether the stream has ended (flush() was called).
    ended: bool,
    /// When true (default), downmix multichannel to stereo.
    downmix_audio: bool,
    /// Native channel count of last decoded audio (before downmix).
    last_audio_channels: u16,
    /// Number of audio decode errors since construction (JS-observable).
    audio_decode_error_count: u64,
    /// Number of segmenter errors since construction (JS-observable).
    segmenter_error_count: u64,
    /// Number of `DiscontinuityKind::TimelineReanchored` events observed
    /// since construction (JS-observable) — issue #101 review, item 1: this
    /// arm was previously completely silent, so a field report could never
    /// confirm whether it ever fires. See the match arm in `drain_events`
    /// for why it deliberately does not reset the audio decoders or mark
    /// the segmenter discontinuous.
    timeline_reanchor_count: u64,
}

#[wasm_bindgen]
impl SkyfireBridge {
    /// Create a new, empty bridge.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        let audio_decoder = skyfire_ac3::IncrementalDecoder::new()
            .unwrap_or_else(|e| wasm_bindgen::throw_str(&format!("build ac3 decoder: {e}")));
        Self {
            demux: skyfire_ts::TsDemux::new(),
            tracks: std::collections::HashMap::new(),
            video_track_id: None,
            cached_video_config: None,
            selected_audio_pid: None,
            audio_channels: std::collections::BTreeMap::new(),
            selected_subtitle_pid: None,
            playing: false,
            video_aus: Vec::new(),
            segmenter: None,
            ready_segments: std::collections::VecDeque::new(),
            audio_decoder,
            mpa_decoder: skyfire_mpa::IncrementalMpaDecoder::new(),
            audio_pcm_pending: Vec::new(),
            subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState::new(),
            subtitle_cues_pending: Vec::new(),
            latest_pcr: None,
            ended: false,
            downmix_audio: true,
            last_audio_channels: 0,
            audio_decode_error_count: 0,
            segmenter_error_count: 0,
            timeline_reanchor_count: 0,
        }
    }

    /// Native channel count of the most recently decoded audio (before any downmix), or 0 if none yet.
    #[wasm_bindgen]
    #[must_use]
    pub fn audio_native_channels(&self) -> u16 {
        self.last_audio_channels
    }

    /// Enable (default) or disable the WASM stereo downmix.
    #[wasm_bindgen]
    pub fn set_audio_downmix(&mut self, enabled: bool) {
        self.downmix_audio = enabled;
    }

    /// Number of audio decode errors since construction.
    #[wasm_bindgen]
    #[must_use]
    pub fn audio_decode_error_count(&self) -> u64 {
        self.audio_decode_error_count
    }

    /// Number of segmenter errors since construction.
    #[wasm_bindgen]
    #[must_use]
    pub fn segmenter_error_count(&self) -> u64 {
        self.segmenter_error_count
    }

    /// Number of `DiscontinuityKind::TimelineReanchored` events observed
    /// since construction (>20ms audio dts/pts re-anchor; see the
    /// `drain_events` match arm for why decoders are deliberately not reset
    /// for these).
    #[wasm_bindgen]
    #[must_use]
    pub fn timeline_reanchor_count(&self) -> u64 {
        self.timeline_reanchor_count
    }

    /// Push a raw TS chunk into the bridge.
    #[wasm_bindgen]
    pub fn feed(&mut self, bytes: &[u8]) {
        self.demux.feed(bytes);
        self.drain_events();
    }

    /// The audio PID currently routed for decode (the source of emitted PCM),
    /// or `None` before any audio track is selected.
    #[wasm_bindgen(getter)]
    pub fn selected_audio_pid(&self) -> Option<u16> {
        self.selected_audio_pid
    }

    /// Select which audio PID to route and decode.
    #[wasm_bindgen]
    pub fn select_audio(&mut self, pid: u16) {
        if self.selected_audio_pid != Some(pid) {
            self.audio_decoder.reset();
            self.mpa_decoder.reset();
        }
        self.selected_audio_pid = Some(pid);
    }

    /// Select a subtitle PID, or `None` to disable subtitles.
    #[wasm_bindgen]
    pub fn select_subtitle(&mut self, pid: Option<u16>) {
        if self.selected_subtitle_pid != pid {
            self.subtitle_cues_pending.clear();
        }
        self.selected_subtitle_pid = pid;
    }

    /// Set the play/pause state.
    #[wasm_bindgen]
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Returns the track list once at least one video track has been seen, or `None`.
    #[wasm_bindgen]
    pub fn track_list(&self) -> Option<WasmTrackList> {
        let video_id = self.video_track_id?;
        let video_meta = self.tracks.get(&video_id)?;
        let video_pid = video_meta.pid.unwrap_or(0);
        let video_codec = match video_meta.kind {
            TrackKind::Video(c) => video_codec_str(c),
            _ => "H264",
        };

        let mut audio: Vec<WasmAudioTrack> = self
            .tracks
            .values()
            .filter(|m| matches!(m.kind, TrackKind::Audio(_)))
            .map(|m| {
                let pid = m.pid.unwrap_or(0);
                WasmAudioTrack {
                    pid,
                    codec: match m.kind {
                        TrackKind::Audio(c) => audio_codec_str(c).to_string(),
                        _ => "EAC3".to_string(),
                    },
                    language: m.language.map(|l| lang_bytes_to_string(&l)),
                    channels: self.audio_channels.get(&pid).copied(),
                }
            })
            .collect();
        audio.sort_by_key(|a| a.pid);

        let mut subtitles: Vec<WasmSubtitleTrack> = self
            .tracks
            .values()
            .filter(|m| matches!(m.kind, TrackKind::Subtitle(_)))
            .map(|m| WasmSubtitleTrack {
                pid: m.pid.unwrap_or(0),
                kind: match m.kind {
                    TrackKind::Subtitle(SubtitleKind::DvbSubtitles) => "DvbSubtitles",
                    TrackKind::Subtitle(SubtitleKind::Teletext) => "Teletext",
                    _ => "DvbSubtitles",
                }
                .to_string(),
                language: m.language.map(|l| lang_bytes_to_string(&l)),
            })
            .collect();
        subtitles.sort_by_key(|s| s.pid);

        Some(WasmTrackList {
            video_pid,
            video_codec: video_codec.to_string(),
            audio,
            subtitles,
        })
    }

    /// CMAF initialization segment (`ftyp` + `moov`). Empty until a video track has been seen.
    #[wasm_bindgen]
    pub fn video_init_segment(&self) -> Vec<u8> {
        self.segmenter
            .as_ref()
            .and_then(|s| s.init_segment().ok())
            .unwrap_or_default()
    }

    /// WebCodecs codec string (e.g. `"avc1.640028"`), or `None` if not yet available.
    #[wasm_bindgen]
    pub fn video_codec(&self) -> Option<String> {
        self.cached_video_config.as_ref().map(|c| c.codec.clone())
    }

    /// WebCodecs `avcC` description bytes, or empty if not yet available.
    #[wasm_bindgen]
    pub fn video_config_description(&self) -> Vec<u8> {
        self.cached_video_config
            .as_ref()
            .map(|c| c.description.clone())
            .unwrap_or_default()
    }

    /// Drain all completed video access units since the last call.
    #[wasm_bindgen]
    pub fn take_video_aus(&mut self) -> Vec<WasmVideoAu> {
        std::mem::take(&mut self.video_aus)
    }

    /// Drain the next complete media segment. Returns `None` until a full segment is ready.
    #[wasm_bindgen]
    pub fn take_video_media_segment(&mut self) -> Option<WasmMediaSegment> {
        if let Some(ref mut seg) = self.segmenter {
            for bytes in seg.take_ready() {
                self.ready_segments.push_back(bytes);
            }
        }
        let bytes = self.ready_segments.pop_front()?;
        let sample_count = parse_sample_count_from_segment(&bytes);
        let base_media_decode_time = parse_base_media_decode_time(&bytes);
        Some(WasmMediaSegment {
            base_media_decode_time,
            bytes,
            sample_count,
        })
    }

    /// Drain all decoded PCM chunks produced since the last call.
    #[wasm_bindgen]
    pub fn take_audio_pcm(&mut self) -> Vec<WasmPcmChunk> {
        std::mem::take(&mut self.audio_pcm_pending)
    }

    /// Drain all composited subtitle cues since the last call.
    #[wasm_bindgen]
    pub fn take_subtitle_cues(&mut self) -> Vec<WasmSubtitleCue> {
        for cue in self.subtitle_compositor.take_cues() {
            let regions = cue
                .regions
                .into_iter()
                .map(|r| WasmSubtitleRegion {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    rgba: r.rgba,
                })
                .collect();
            self.subtitle_cues_pending.push(WasmSubtitleCue {
                start_pts: cue.start_pts,
                end_pts: cue.end_pts,
                regions,
            });
        }
        std::mem::take(&mut self.subtitle_cues_pending)
    }

    /// Latest PCR-derived clock value in 90 kHz ticks.
    #[wasm_bindgen]
    pub fn pcr_pts(&self) -> Option<i64> {
        self.latest_pcr
    }

    /// Signal end-of-stream: flush any partial access units and emit the final segment.
    #[wasm_bindgen]
    pub fn flush(&mut self) {
        self.demux.finish();
        self.drain_events();
        if let Some(ref mut seg) = self.segmenter
            && let Err(e) = seg.flush()
        {
            self.segmenter_error_count += 1;
            std::eprintln!("[skyfire-wasm] segmenter flush error: {e}");
        }
        self.ended = true;
    }

    // ── internal ────────────────────────────────────────────────────────────

    fn drain_events(&mut self) {
        while let Some(ev) = self.demux.poll_event() {
            match ev {
                DemuxEvent::TrackAdded(track) => self.on_track_added(track),
                DemuxEvent::TrackUpdated(track) => self.on_track_updated(track),
                DemuxEvent::Sample {
                    track_id, sample, ..
                } => self.on_sample(track_id, sample),
                DemuxEvent::ClockReference {
                    ticks,
                    clock_hz,
                    discontinuous,
                    ..
                } => {
                    // Container-neutral clock reference: convert `ticks` (in
                    // `clock_hz` Hz, 27 MHz for an MPEG-2 TS PCR — but read,
                    // never assumed) to 90 kHz ticks.
                    if clock_hz > 0 {
                        let ticks_90k = u128::from(ticks) * 90_000 / u128::from(clock_hz);
                        // Assign only on success (mirrors the `clock_hz > 0`
                        // guard above): a conversion failure must never
                        // clobber a previously valid `latest_pcr` to `None`
                        // (issue #101 review, item 5) — stale-but-valid beats
                        // silently absent.
                        if let Ok(v) = i64::try_from(ticks_90k) {
                            self.latest_pcr = Some(v);
                        }
                    }
                    // `discontinuous` (issue #101 review, item 4): explicitly
                    // bound rather than dropped via `..`, so this reasoning
                    // is visible and re-examined if it ever stops holding,
                    // not an invisible assumption.
                    //
                    // Left as a no-op deliberately, not silently: `latest_pcr`
                    // above is a plain last-value overwrite with no delta/
                    // accumulator state, so a discontinuous jump in the
                    // *value* is not a corruption risk here the way it would
                    // be for e.g. `skyfire_sync::AudioClock`'s wrap-tracking.
                    // Every `WasmVideoAu`/`WasmPcmChunk` timestamp this
                    // bridge emits is each independently absolute (transmux's
                    // IR, media plane step 2c) — never derived from or
                    // adjusted by `latest_pcr` — so a PCR jump cannot
                    // retroactively corrupt an already-emitted timestamp.
                    // The audio-decoder IMDCT reset this bridge performs on a
                    // splice is triggered by `DemuxEvent::Discontinuity` (see
                    // below), not by this event; a PCR's own `discontinuous`
                    // bit and a `DiscontinuityKind::Signalled` observation
                    // are both driven by the same adaptation-field
                    // `discontinuity_indicator` bit on the same TS packet, so
                    // in practice they always co-occur for that cause.
                    // Residual risk this crate cannot fix: if the browser
                    // shell derives its own expected-next-PCR delta from
                    // consecutive `pcr_pts()` reads, a jump could still
                    // surprise that JS-side logic — out of scope for this
                    // Rust bridge (the shell owns that clock, per ADR 0008).
                    let _ = discontinuous;
                }
                DemuxEvent::Discontinuity { kind, .. } => {
                    match kind {
                        DiscontinuityKind::Signalled => {
                            // Explicit adaptation-field discontinuity_indicator
                            // (ISO/IEC 13818-1 §2.4.3.5): the source stream
                            // itself signalled a splice/encoder restart. Keep
                            // today's full reset — mark the segmenter
                            // discontinuous (HLS emits EXT-X-DISCONTINUITY)
                            // and flush the AC-3/E-AC-3 + MP2 decoders' IMDCT
                            // state, since audio either side of a genuine
                            // splice is not one coded sequence.
                            if let Some(ref mut seg) = self.segmenter {
                                seg.mark_discontinuity();
                            }
                            self.audio_decoder.reset();
                            self.mpa_decoder.reset();
                        }
                        DiscontinuityKind::TimelineReanchored => {
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
                            // still choose NOT to reset audio_decoder/
                            // mpa_decoder or mark_discontinuity() here — an
                            // accepted risk, not a misreading of the event:
                            // `DiscontinuityKind` carries no drift magnitude,
                            // so this call site cannot tell "just over the
                            // 20 ms line" from "genuinely spliced stream",
                            // and resetting the AC-3/E-AC-3/MP2 decoders'
                            // IMDCT overlap-add state plus forcing a CMAF/HLS
                            // discontinuity signal on every one of these on a
                            // long-running live feed would itself introduce
                            // audible clicks / unnecessary client-side resets
                            // far more often than a genuine splice occurs. A
                            // muxer restart that *does* set the adaptation-
                            // field discontinuity_indicator is already
                            // caught by `Signalled` above with a full reset.
                            //
                            // Accepted residual risk: on a live feed whose
                            // muxer does not set discontinuity_indicator
                            // across an encoder restart/PID reuse, we keep
                            // the AC-3/E-AC-3 decoders' IMDCT overlap-add
                            // window running across that seam (a possible
                            // audible glitch) and emit no
                            // #EXT-X-DISCONTINUITY / CMAF discontinuity
                            // signal for it. We accept that rather than
                            // reset-on-every-wobble because ordinary jittery
                            // live muxers cross the 20 ms line routinely,
                            // and a full reset there is a more frequent,
                            // certainly-audible regression traded for a rare,
                            // possibly-inaudible one. Each `Sample`'s dts/pts
                            // is already the corrected, absolute value
                            // (transmux's own IR, media plane step 2c) by
                            // the time it reaches the segmenter regardless.
                            self.timeline_reanchor_count += 1;
                            std::eprintln!(
                                "[skyfire-wasm] discontinuity: TimelineReanchored \
                                 (audio dts/pts re-anchored, >20ms drift; per \
                                 accepted risk, decoders not reset, segmenter \
                                 not marked discontinuous)"
                            );
                        }
                        DiscontinuityKind::BudgetExceeded { bytes } => {
                            // A per-PID PES buffer cap was tripped and
                            // in-flight payload was dropped (transmux's
                            // `MAX_PES_BUFFER_BYTES`) — real data loss, not
                            // just a timeline correction. The decoder's next
                            // input is missing bytes it has no way to know
                            // about, so its IMDCT state cannot be trusted to
                            // continue cleanly. Treat like `Signalled`: reset
                            // both audio decoders and mark the segmenter
                            // discontinuous.
                            std::eprintln!(
                                "[skyfire-wasm] discontinuity: PES budget exceeded, \
                                 {bytes} bytes dropped"
                            );
                            if let Some(ref mut seg) = self.segmenter {
                                seg.mark_discontinuity();
                            }
                            self.audio_decoder.reset();
                            self.mpa_decoder.reset();
                        }
                        // `DiscontinuityKind` is `#[non_exhaustive]` (a future
                        // discontinuity source, e.g. issue #778's
                        // continuity-counter gap, adds a variant without a
                        // breaking change). Default to the conservative,
                        // full-reset behaviour for any kind this bridge
                        // doesn't recognise yet — the same rationale as
                        // `Signalled`/`BudgetExceeded` above: an unknown cause
                        // gets the safe assumption (audio state may not be
                        // trustworthy), never the `TimelineReanchored`
                        // no-audible-reset exemption, which is earned per
                        // known-cause, not a default.
                        _ => {
                            if let Some(ref mut seg) = self.segmenter {
                                seg.mark_discontinuity();
                            }
                            self.audio_decoder.reset();
                            self.mpa_decoder.reset();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn on_track_added(&mut self, track: transmux::TrackSpec) {
        let meta = skyfire_ts::track_meta(&track);
        let track_id = track.track_id;

        if matches!(meta.kind, TrackKind::Video(_)) && self.video_track_id.is_none() {
            self.video_track_id = Some(track_id);

            if let transmux::CodecConfig::Avc { ref config, .. } = track.config {
                let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
                self.cached_video_config = Some(CachedVideoConfig { codec, description });
            }

            // #101 review, finding 2: this must be the *track's own*
            // timescale, not a hardcoded 90_000 — the mirror image of the
            // bug just fixed. Harmless today (only the video track is ever
            // pushed into this MSE-fallback segmenter, and video's timescale
            // is always 90 000), but `Sample`s pushed into `seg` below carry
            // ticks in `track.timescale` units, and this value becomes the
            // segmenter's per-track `TrackSpec::timescale`, which in turn
            // feeds `WasmMediaSegment.base_media_decode_time` — documented as
            // 90 kHz. The `Segmenter::new` second argument (2.0s target
            // duration's `90_000`) is that separate global fragmenting
            // timescale, not this per-track one, and is unaffected.
            let ts = transmux::TrackSpec::new(track_id, track.timescale, track.config.clone());
            if let Ok(seg) = transmux::Segmenter::new(vec![ts], 90_000, 2.0) {
                self.segmenter = Some(seg);
            }
        }

        if matches!(meta.kind, TrackKind::Audio(_))
            && self.selected_audio_pid.is_none()
            && let Some(pid) = meta.pid
        {
            self.selected_audio_pid = Some(pid);
        }

        self.tracks.insert(track_id, meta);
    }

    fn on_track_updated(&mut self, track: transmux::TrackSpec) {
        let meta = skyfire_ts::track_meta(&track);
        // If the video track's config changed (e.g. in-band SPS update), rebuild
        // cached_video_config — mirrors what skyfire-core's Engine already does.
        if Some(track.track_id) == self.video_track_id
            && let transmux::CodecConfig::Avc { ref config, .. } = track.config
        {
            let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
            self.cached_video_config = Some(CachedVideoConfig { codec, description });
        }
        self.tracks.insert(track.track_id, meta);
    }

    fn on_sample(&mut self, track_id: u32, sample: transmux::Sample) {
        let meta = match self.tracks.get(&track_id).cloned() {
            Some(m) => m,
            None => return,
        };

        match meta.kind {
            TrackKind::Video(_) if Some(track_id) == self.video_track_id => {
                // Seed latest_pcr from video PTS only before the first real
                // clock-reference event arrives. DemuxEvent::ClockReference is
                // the authoritative source and must not be overwritten by
                // every video sample. checked_ticks_90k rejects a negative
                // pts (never expected) rather than trusting it, and rescales
                // from this track's own timescale to 90 kHz (a no-op for
                // video, whose timescale is 90 000, but the same conversion
                // function as every other track — never a special case).
                if self.latest_pcr.is_none()
                    && let Some(pts) = skyfire_ts::checked_ticks_90k(sample.pts, meta.timescale)
                {
                    // Checked conversion (issue #101 review, item 5): a raw
                    // `pts as i64` cast is never reachable-safe on a
                    // timestamp. `pts` (u64, 90 kHz ticks) practically never
                    // exceeds i64::MAX, but a failed conversion here is
                    // simply "don't seed the fallback", not an error.
                    if let Ok(v) = i64::try_from(pts) {
                        self.latest_pcr = Some(v);
                    }
                }
                let pts = skyfire_ts::checked_ticks_90k(sample.pts, meta.timescale);
                let dts = skyfire_ts::checked_ticks_90k(sample.dts, meta.timescale);
                // transmux 0.14 (rust-broadcast#595) sets is_sync on open-GOP
                // random-access points (IDR / recovery-point SEI / SPS-led AU),
                // so no client-side keyframe re-derivation is needed.
                self.video_aus.push(WasmVideoAu {
                    pts_ticks: pts,
                    dts_ticks: dts,
                    is_keyframe: sample.flags.is_sync,
                    bytes: sample.data.to_vec(),
                });
                if let Some(ref mut seg) = self.segmenter
                    && let Err(e) = seg.push(track_id, sample)
                {
                    self.segmenter_error_count += 1;
                    std::eprintln!("[skyfire-wasm] segmenter push error: {e}");
                }
            }
            TrackKind::Audio(codec) if meta.pid == self.selected_audio_pid => {
                self.probe_channels(meta.pid, codec, &sample.data);
                // CRITICAL fix (issue #101 review): an audio track's own
                // timescale is its sample rate (e.g. 48 000), not 90 kHz
                // (transmux 0.20's absolute Sample::pts/dts are in the
                // owning track's TrackSpec::timescale) — must rescale via
                // `meta.timescale`, the same conversion every track uses.
                let pts_ticks = skyfire_ts::checked_ticks_90k(sample.pts, meta.timescale);
                self.decode_audio(codec, pts_ticks, &sample.data);
            }
            // Unselected audio PIDs are never decoded, but their frame headers
            // still tell us the channel layout — which the picker needs in
            // order to label them.
            TrackKind::Audio(codec) => {
                self.probe_channels(meta.pid, codec, &sample.data);
            }
            TrackKind::Subtitle(_) if meta.pid == self.selected_subtitle_pid => {
                let pid = meta.pid.unwrap_or(0);
                // Subtitle (Data) tracks already carry timescale == 90_000,
                // so this is a no-op rescale — but the same conversion
                // function as video/audio, never a special case.
                let pts_ticks = skyfire_ts::checked_ticks_90k(sample.pts, meta.timescale);
                if sample.data.first() == Some(&dvb_subtitle::DataIdentifier)
                    && let Ok(field) = dvb_subtitle::PesDataField::parse(&sample.data)
                {
                    self.subtitle_compositor.feed_pes(pid, pts_ticks, &field);
                }
            }
            _ => {}
        }
    }

    /// Records the channel count for `pid` from a frame header, once.
    fn probe_channels(&mut self, pid: Option<u16>, codec: AudioCodec, data: &[u8]) {
        let Some(pid) = pid else { return };
        if self.audio_channels.contains_key(&pid) {
            return;
        }
        let ch = match codec {
            AudioCodec::Mp2 => skyfire_ts::mp2_header::channels_from_header(data),
            // Explicit, not a catch-all: `AudioCodec` has exactly three
            // variants and adding a fourth must fail to compile here rather
            // than silently probe it as AC-3 (mirrors `audio_codec_str`).
            AudioCodec::Ac3 | AudioCodec::EAc3 => {
                skyfire_ac3::header::channels_from_syncframe(data)
            }
        };
        if let Some(ch) = ch {
            self.audio_channels.insert(pid, ch);
        }
    }

    fn decode_audio(&mut self, codec: AudioCodec, pts_ticks: Option<u64>, data: &[u8]) {
        match codec {
            AudioCodec::Mp2 => match self.mpa_decoder.decode_au(data) {
                Ok(Some(decoded)) if decoded.sample_rate > 0 && decoded.channels > 0 => {
                    // Mirror the AC-3 path: record the true source channel count and
                    // emit a CONSISTENT channel layout. Previously this passed the
                    // raw per-frame channel count and never set last_audio_channels,
                    // so the player locked its output to the first frame's count and
                    // dropped every frame that differed (stereo MP2 heard as mono /
                    // silent). Normalise ≤2ch to stereo (mono upmixed, stereo kept).
                    self.last_audio_channels = decoded.channels;
                    let (channels, samples_f32) = if self.downmix_audio || decoded.channels <= 2 {
                        (
                            2u16,
                            skyfire_ac3::downmix::downmix_s16le_to_stereo_f32(
                                &decoded.pcm_s16le,
                                decoded.channels,
                            ),
                        )
                    } else {
                        (
                            decoded.channels,
                            skyfire_ac3::downmix::s16le_slice_to_f32(&decoded.pcm_s16le),
                        )
                    };
                    self.audio_pcm_pending.push(WasmPcmChunk {
                        pts_ticks,
                        sample_rate: decoded.sample_rate,
                        channels,
                        samples: samples_f32,
                    });
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => {
                    self.audio_decode_error_count += 1;
                    std::eprintln!("[skyfire-wasm] mp2 decode error: {e}");
                }
            },
            _ => match self.audio_decoder.decode_au(data) {
                Ok(Some(decoded)) if decoded.sample_rate > 0 && decoded.channels > 0 => {
                    self.last_audio_channels = decoded.channels;
                    let (channels, samples_f32) = if self.downmix_audio || decoded.channels <= 2 {
                        (
                            2u16,
                            skyfire_ac3::downmix::downmix_s16le_to_stereo_f32(
                                &decoded.pcm_s16le,
                                decoded.channels,
                            ),
                        )
                    } else {
                        let native = skyfire_ac3::downmix::s16le_slice_to_f32(&decoded.pcm_s16le);
                        (decoded.channels, native)
                    };
                    self.audio_pcm_pending.push(WasmPcmChunk {
                        pts_ticks,
                        sample_rate: decoded.sample_rate,
                        channels,
                        samples: samples_f32,
                    });
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => {
                    self.audio_decode_error_count += 1;
                    std::eprintln!("[skyfire-wasm] ac3/eac3 decode error: {e}");
                }
            },
        }
    }
}

impl Default for SkyfireBridge {
    fn default() -> Self {
        Self::new()
    }
}
