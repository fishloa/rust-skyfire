use broadcast_common::traits::Parse;
use skyfire_ts::{AudioCodec, DemuxEvent, SubtitleKind, TrackKind, TrackMeta};
use skyfire_ts::{audio_codec_str, video_codec_str};
use transmux::{DiscontinuityKind, InputDegradation};
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
    /// The audio PID that most recently produced decoded PCM. Updated every time
    /// `decode_audio` succeeds. Unlike `selected_audio_pid` (always the *requested*
    /// PID), this reflects what is genuinely being decoded — the test's primary oracle.
    decoded_audio_pid: Option<u16>,
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
    /// Number of `DemuxEvent::InputDegraded { kind: TransportError }` events
    /// observed since construction (JS-observable) — issue #106. A single
    /// corrupt TS packet (transport_error_indicator set). Counted + logged,
    /// never a decoder reset (see `degradation_action`).
    transport_error_count: u64,
    /// Number of `DemuxEvent::InputDegraded { kind: ContinuityGap { .. } }`
    /// events observed since construction (JS-observable) — issue #106. Lost
    /// packets; triggers an audio-decoder reset (see `degradation_action`).
    continuity_gap_count: u64,
    /// Number of `DemuxEvent` variants this bridge did not recognise —
    /// genuinely new `#[non_exhaustive]` variants from a future transmux.
    /// Counted and logged here (issue #103) instead of silently dropped, so a
    /// new upstream variant fails loud rather than vanishing.
    unknown_event_count: u64,
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
            decoded_audio_pid: None,
            last_audio_channels: 0,
            audio_decode_error_count: 0,
            segmenter_error_count: 0,
            timeline_reanchor_count: 0,
            transport_error_count: 0,
            continuity_gap_count: 0,
            unknown_event_count: 0,
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

    /// Number of `InputDegradation::TransportError` events (single corrupt
    /// TS packets) observed since construction. These are counted and logged
    /// but deliberately do not reset the audio decoders (see
    /// `degradation_action`).
    #[wasm_bindgen]
    #[must_use]
    pub fn transport_error_count(&self) -> u64 {
        self.transport_error_count
    }

    /// Number of `InputDegradation::ContinuityGap` events (lost packets)
    /// observed since construction. Each triggers an audio-decoder reset
    /// (see `degradation_action`).
    #[wasm_bindgen]
    #[must_use]
    pub fn continuity_gap_count(&self) -> u64 {
        self.continuity_gap_count
    }

    /// Number of unrecognised `DemuxEvent` variants (future transmux
    /// `#[non_exhaustive]` additions) observed since construction. New
    /// upstream variants are counted and logged here instead of being
    /// silently dropped (issue #103).
    #[wasm_bindgen]
    #[must_use]
    pub fn unknown_event_count(&self) -> u64 {
        self.unknown_event_count
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

    /// The audio PID that most recently produced decoded PCM, or `None`
    /// before any audio has been decoded.
    ///
    /// Written inside `decode_audio`, in the branch where a decode actually
    /// succeeded, from the PID passed in by the caller — never in `on_sample`
    /// and never from `selected_audio_pid`. That distinction is the point of
    /// the field (issue #89): inside the selected-audio match arm `meta.pid`
    /// is equal to `selected_audio_pid` by construction, so assigning from
    /// there would make this a copy of the request and report a switch that
    /// had not happened. A decode failure therefore leaves the previous value
    /// in place, which is correct — nothing new has been decoded.
    #[wasm_bindgen]
    pub fn current_decoded_pid(&self) -> Option<u16> {
        self.decoded_audio_pid
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
                DemuxEvent::Discontinuity { kind, .. } => self.apply_discontinuity(kind),
                DemuxEvent::InputDegraded { kind, .. } => self.apply_input_degraded(kind),
                DemuxEvent::TrackRemoved { track_id, .. } => {
                    // Intentionally a no-op, but explicit + logged, not
                    // silently swallowed (issue #103): this bridge holds a
                    // *monotonic* record of what was declared and chosen —
                    // `self.tracks`, `video_track_id`, `selected_audio_pid`,
                    // `selected_subtitle_pid` — and never re-segments
                    // arbitrarily (the MSE segmenter is built once per
                    // track's `TrackSpec` and the browser keeps the
                    // WebCodecs/WebAudio pipeline alive regardless). A
                    // removed PMT PID simply stops producing `Sample`s, so no
                    // stale entry can corrupt behaviour. Logging (not
                    // silence) makes a PMT churn visible in diagnostics.
                    std::eprintln!(
                        "[skyfire-wasm] track removed: track_id={track_id} \
                         (no-op: bridge tracks are a monotonic declared-set \
                         record; a removed PID simply stops emitting samples)"
                    );
                }
                DemuxEvent::TrackAbandoned {
                    track_id, reason, ..
                } => {
                    // No-op like `TrackRemoved`, made explicit + logged
                    // (#103): an abandoned track never fired `TrackAdded` and
                    // never produced a track_id, so nothing in this bridge
                    // ever referenced it. Logging keeps a config-recovery
                    // failure diagnosable instead of vanishing.
                    std::eprintln!(
                        "[skyfire-wasm] track abandoned: track_id={track_id:?} \
                         reason={reason} (no-op: never promoted, nothing to tear down)"
                    );
                }
                DemuxEvent::TracksResolved { generation, .. } => {
                    // No state change needed (#103): `SkyfireBridge` builds
                    // its segmenter incrementally on each video `TrackAdded`
                    // and drains `Sample`s as they arrive, so it has no
                    // track-set gate (`TracksResolved`'s purpose is
                    // multi-track segmenter construction that waits on a
                    // stable track set). Logged once for visibility rather
                    // than silently ignored.
                    std::eprintln!(
                        "[skyfire-wasm] tracks resolved: generation={generation} \
                         (no-op: bridge segments incrementally, no track-set gate \
                         to clear)"
                    );
                }
                // A genuinely new `#[non_exhaustive]` `DemuxEvent` variant
                // from a future transmux release. Counted and logged here
                // (#103) so it fails loud (count + log) rather than vanishing
                // the way `InputDegraded` used to.
                _ => {
                    self.unknown_event_count += 1;
                    std::eprintln!(
                        "[skyfire-wasm] unrecognised DemuxEvent variant (future \
                         transmux #[non_exhaustive] addition), n={}",
                        self.unknown_event_count
                    );
                }
            }
        }
    }

    // ── pure-decision application (#103 / #106) ──────────────────────────

    /// Apply an [`EventAction`] to the bridge's mutable state. Shared by all
    /// the pure decision call sites so the event loop never re-decides — it
    /// only ever applies what [`discontinuity_action`] /
    /// [`degradation_action`] returned.
    fn apply_action(&mut self, action: EventAction) {
        if action.mark_segmenter_discontinuity
            && let Some(ref mut seg) = self.segmenter
        {
            seg.mark_discontinuity();
        }
        if action.reset_decoders {
            self.audio_decoder.reset();
            self.mpa_decoder.reset();
        }
    }

    /// Handle a `DemuxEvent::Discontinuity`. The *decision* is the pure
    /// [`discontinuity_action`]; this method only adds the non-decision
    /// side-effects (per-kind logging / counting) and applies the returned
    /// action.
    fn apply_discontinuity(&mut self, kind: DiscontinuityKind) {
        match kind {
            DiscontinuityKind::Signalled => {}
            DiscontinuityKind::TimelineReanchored => {
                self.timeline_reanchor_count += 1;
                std::eprintln!(
                    "[skyfire-wasm] discontinuity: TimelineReanchored \
                     (audio dts/pts re-anchored, >20ms drift; per accepted \
                     risk, decoders not reset, segmenter not marked discontinuous)"
                );
            }
            DiscontinuityKind::BudgetExceeded { bytes } => {
                std::eprintln!(
                    "[skyfire-wasm] discontinuity: PES budget exceeded, \
                     {bytes} bytes dropped"
                );
            }
            // Unknown future `DiscontinuityKind` (#[non_exhaustive]) —
            // `discontinuity_action` decides it (conservative full reset);
            // nothing else to log here.
            _ => {}
        }
        self.apply_action(discontinuity_action(kind));
    }

    /// Handle a `DemuxEvent::InputDegraded` (#106). Counts and logs each
    /// degradation, then applies the pure [`degradation_action`] decision.
    fn apply_input_degraded(&mut self, kind: InputDegradation) {
        match kind {
            InputDegradation::TransportError => {
                self.transport_error_count += 1;
                std::eprintln!(
                    "[skyfire-wasm] input degraded: transport error (single \
                     corrupt packet; counted, decoders not reset)"
                );
            }
            InputDegradation::ContinuityGap { expected, got } => {
                self.continuity_gap_count += 1;
                std::eprintln!(
                    "[skyfire-wasm] input degraded: continuity-counter gap \
                     (expected CC {expected}, got {got}; resetting audio decoders)"
                );
            }
            _ => {
                self.unknown_event_count += 1;
                std::eprintln!(
                    "[skyfire-wasm] input degraded: unrecognised InputDegradation \
                     kind (future #[non_exhaustive])"
                );
            }
        }
        self.apply_action(degradation_action(kind));
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
                self.decode_audio(codec, pts_ticks, &sample.data, meta.pid);
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

    fn decode_audio(
        &mut self,
        codec: AudioCodec,
        pts_ticks: Option<u64>,
        data: &[u8],
        pid: Option<u16>,
    ) {
        match codec {
            AudioCodec::Mp2 => match self.mpa_decoder.decode_au(data) {
                Ok(Some(decoded)) if decoded.sample_rate > 0 && decoded.channels > 0 => {
                    // Record which PID actually produced this decoded PCM
                    // so the oracle sees a genuinely decoded PID, not the
                    // request-only echo.
                    self.decoded_audio_pid = pid;
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
                    self.decoded_audio_pid = pid;
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

// ── pure per-variant decisions (#103) ───────────────────────────────────

/// What a `DemuxEvent::Discontinuity` kind or `DemuxEvent::InputDegraded`
/// `kind` should trigger on the bridge's mutable state. Pure: derived from a
/// `match` on the kind value alone, with no `self`. Kept out of the event
/// loop so every per-kind decision is unit-testable without a fixture that
/// emits the event (that is the point of #103 — the decisions were baked
/// into the match arms and thus untestable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventAction {
    /// Reset the AC-3/E-AC-3 + MP2 decoders' IMDCT overlap-add state.
    pub reset_decoders: bool,
    /// Mark the MSE segmenter discontinuous (HLS/CMAF `EXT-X-DISCONTINUITY`).
    pub mark_segmenter_discontinuity: bool,
}

impl EventAction {
    const NONE: EventAction = EventAction {
        reset_decoders: false,
        mark_segmenter_discontinuity: false,
    };
    const FULL_RESET: EventAction = EventAction {
        reset_decoders: true,
        mark_segmenter_discontinuity: true,
    };
}

/// Pure decision for a `DemuxEvent::Discontinuity` `kind`. The event loop
/// applies the returned action (via [`SkyfireBridge::apply_action`]); this
/// function is the single, testable source of truth for *what* a kind
/// triggers. Reasoning per kind:
///
/// - `Signalled` — explicit adaptation-field `discontinuity_indicator`
///   (ISO/IEC 13818-1 §2.4.3.5): a genuine splice/encoder restart the source
///   flagged itself. Full reset is correct: audio either side of a genuine
///   splice is not one coded sequence.
/// - `TimelineReanchored` — the audio anchor drifted past the 20 ms
///   re-anchor threshold and was re-anchored. Upstream it is *also* a genuine
///   gap/splice signal, but `DiscontinuityKind` carries no drift magnitude,
///   so the call site cannot tell "just over the 20 ms line" from "spliced".
///   Resetting on every of these on a long-running jittery live feed would
///   introduce audible clicks / needless client resets far more often than a
///   real splice occurs, and a muxer restart that does set the
///   discontinuity_indicator is already caught by `Signalled`. So we
///   deliberately do **not** reset. (#101 review, item 1.)
/// - `BudgetExceeded` — a per-PID PES cap tripped and in-flight payload was
///   dropped (`MAX_PES_BUFFER_BYTES`): real data loss the decoder cannot know
///   about, so its IMDCT state cannot be trusted. Full reset, same as
///   `Signalled`.
/// - Unknown future kind — `#[non_exhaustive]`: conservative full reset. The
///   safe assumption is "audio state may not be trustworthy", never the
///   earned-per-known-cause `TimelineReanchored` exemption.
pub(crate) fn discontinuity_action(kind: DiscontinuityKind) -> EventAction {
    match kind {
        DiscontinuityKind::Signalled => EventAction::FULL_RESET,
        DiscontinuityKind::TimelineReanchored => EventAction::NONE,
        DiscontinuityKind::BudgetExceeded { .. } => EventAction::FULL_RESET,
        _ => EventAction::FULL_RESET,
    }
}

/// Pure decision for a `DemuxEvent::InputDegraded` `kind` (#106). The event
/// loop counts/logs the observation and applies the returned action (via
/// [`SkyfireBridge::apply_action`]). Reasoning per kind:
///
/// - `TransportError` — the `transport_error_indicator` bit was set
///   (ISO/IEC 13818-1 §2.4.3.2): *one* packet the demodulator could not
///   correct. Deliberately **no reset**: a single corrupt packet can at most
///   corrupt one frame (the demux drops its payload so no garbage bytes reach
///   the decoder), and broadcast UDP feeds hit occasional single-TEI packets
///   routinely. Resetting both decoders' IMDCT overlap-add state on every one
///   would inject audible clicks far more often than it fixes anything. Count
///   + log, nothing more.
/// - `ContinuityGap` — whole packets were lost (a continuity-counter gap,
///   excluding live duplicates and signalled discontinuities). This is *data
///   loss the decoder cannot know about*: its next input is missing bytes, so
///   its IMDCT overlap-add state cannot be trusted to continue cleanly. The
///   same rationale already applied to `BudgetExceeded` argues for a decoder
///   reset here, so we reset. We deliberately do **not** also mark the
///   segmenter discontinuous: surviving `Sample` dts/pts are still the
///   corrected absolute values (media plane step 2c), so an
///   `EXT-X-DISCONTINUITY` (which forces every client through a heavy reset
///   and breaks timeline-joining) is more than the recovery needs.
/// - Unknown future kind — `#[non_exhaustive]`: a missing-data degradation,
///   so default to the continuity-gap conservative reset (no segmenter
///   discontinuity, same reasoning).
pub(crate) fn degradation_action(kind: InputDegradation) -> EventAction {
    match kind {
        InputDegradation::TransportError => EventAction::NONE,
        InputDegradation::ContinuityGap { .. } => EventAction {
            reset_decoders: true,
            mark_segmenter_discontinuity: false,
        },
        _ => EventAction {
            reset_decoders: true,
            mark_segmenter_discontinuity: false,
        },
    }
}

impl Default for SkyfireBridge {
    fn default() -> Self {
        Self::new()
    }
}
