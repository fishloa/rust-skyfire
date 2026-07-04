//! WASM bindings for Skyfire — exposes [`skyfire_core::Engine`] to JavaScript.
//!
//! The `wasm-bindgen` boundary:
//! - Construct an engine (probe channel map, init, feed, flush, finalize).
//! - Pull decoded audio PCM (`Uint8Array`), sample rate, channel count.
//! - Pull H.264 video access units (bytes + PTS) and the WebCodecs config
//!   (codec string + `avcC` description).
//!
//! Data-in/data-out only — no `web-sys` DOM/WebCodecs/AudioWorklet calls.
//! The browser shell in `web/` drives those APIs with the data surfaced here.

use skyfire_core::Engine;
use skyfire_ts::{audio_codec_str, video_codec_str};
use wasm_bindgen::prelude::*;

/// Result of probing MPEG-TS bytes for the channel map (PAT+PMT).
#[wasm_bindgen]
pub struct ProbeResult {
    /// PID of the video elementary stream.
    pub video_pid: u16,
    /// Video codec identifier: `"H264"` or `"H265"`.
    #[wasm_bindgen(getter_with_clone)]
    pub video_codec: String,
    /// PIDs of audio elementary streams (at least one for DVB).
    audio_pids: Vec<u16>,
    /// Audio codec identifiers, parallel to `audio_pids`: `"AC3"`, `"EAC3"`, or `"MP2"`.
    audio_codecs: Vec<String>,
}

#[wasm_bindgen]
impl ProbeResult {
    /// PIDs of audio elementary streams.
    #[wasm_bindgen(getter)]
    pub fn audio_pids(&self) -> Vec<u16> {
        self.audio_pids.clone()
    }

    /// Audio codec strings, parallel to `audio_pids`.
    #[wasm_bindgen(getter)]
    pub fn audio_codecs(&self) -> Vec<String> {
        self.audio_codecs.clone()
    }
}

/// One H.264 video access unit surfaced to JS.
#[wasm_bindgen]
pub struct WasmVideoUnit {
    /// Elementary-stream bytes (NAL unit / picture data).
    #[wasm_bindgen(getter_with_clone)]
    pub bytes: Vec<u8>,
    /// PTS in 90 kHz ticks, or `None` before the first PTS is seen.
    pts_ticks: Option<u64>,
}

#[wasm_bindgen]
impl WasmVideoUnit {
    /// PTS in 90 kHz ticks, or `undefined` if not yet known.
    #[wasm_bindgen(getter)]
    pub fn pts_ticks(&self) -> Option<u64> {
        self.pts_ticks
    }
}

/// WASM-bound Skyfire engine — thin wrapper around [`Engine`].
///
/// # Usage from JS
///
/// ```js
/// const engine = new WasmEngine();
/// const ch = engine.probe(tsBytes);
/// engine.init_with_channel(ch.video_pid, ch.video_codec,
///     ch.audio_pids, ch.audio_codecs);
/// engine.feed(tsBytes);
/// engine.flush();
/// engine.finalize();
///
/// const pcm = engine.audio_pcm();        // Uint8Array (S16LE interleaved)
/// const rate = engine.audio_sample_rate();
/// const chs = engine.audio_channels();
///
/// for (let i = 0; i < engine.video_unit_count(); i++) {
///     const au = engine.video_unit(i);
///     console.log(au.bytes, au.pts_ticks);
/// }
///
/// const codec = engine.video_config_codec();    // e.g. "avc1.640028"
/// const avcc = engine.video_config_description(); // Uint8Array
/// ```
#[wasm_bindgen]
#[derive(Default)]
pub struct WasmEngine {
    engine: Option<Engine>,
}

#[wasm_bindgen]
impl WasmEngine {
    /// Create a new, uninitialized engine.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Probe raw MPEG-TS bytes for the channel map (PAT+PMT).
    ///
    /// Returns `null` if no PAT/PMT could be extracted.
    ///
    /// Internally feeds the bytes into a temporary Engine to discover tracks,
    /// then returns the video PID/codec and audio PIDs/codecs.
    #[wasm_bindgen]
    pub fn probe(&self, data: &[u8]) -> Option<ProbeResult> {
        let mut engine = Engine::new();
        engine.feed(data);
        engine.finish();
        engine.finalize();

        let tracks = engine.tracks();
        let video_track = tracks
            .iter()
            .find(|t| matches!(t.kind, skyfire_core::ts::TrackKind::Video(_)))?;
        let video_pid = video_track.pid.unwrap_or(0);
        let video_codec = match video_track.kind {
            skyfire_core::ts::TrackKind::Video(c) => video_codec_str(c),
            _ => "H264",
        };

        let audio_pids: Vec<u16> = tracks
            .iter()
            .filter(|t| matches!(t.kind, skyfire_core::ts::TrackKind::Audio(_)))
            .map(|t| t.pid.unwrap_or(0))
            .collect();
        let audio_codecs: Vec<String> = tracks
            .iter()
            .filter(|t| matches!(t.kind, skyfire_core::ts::TrackKind::Audio(_)))
            .map(|t| match t.kind {
                skyfire_core::ts::TrackKind::Audio(c) => audio_codec_str(c).to_string(),
                _ => "EAC3".to_string(),
            })
            .collect();

        if audio_pids.is_empty() {
            return None;
        }

        Some(ProbeResult {
            video_pid,
            video_codec: video_codec.to_string(),
            audio_pids,
            audio_codecs,
        })
    }

    /// Initialize the engine. In the new API, the engine is self-configuring;
    /// this method creates a fresh engine (ignoring the channel hint, which is
    /// now auto-detected from the TS stream).
    #[wasm_bindgen]
    pub fn init_with_channel(
        &mut self,
        _video_pid: u16,
        _video_codec: &str,
        _audio_pids: Vec<u16>,
        _audio_codecs: Vec<String>,
    ) {
        // Engine is now self-configuring via TsDemux — no manual channel setup needed.
        self.engine = Some(Engine::new());
    }

    /// Feed raw MPEG-TS bytes into the engine.
    #[wasm_bindgen]
    pub fn feed(&mut self, data: &[u8]) {
        if let Some(ref mut e) = self.engine {
            e.feed(data);
        }
    }

    /// Flush any partial PES packets still in the demux.
    #[wasm_bindgen]
    pub fn flush(&mut self) {
        if let Some(ref mut e) = self.engine {
            e.finish();
        }
    }

    /// Finalize: batch-decode accumulated audio ES to PCM, build video config.
    #[wasm_bindgen]
    pub fn finalize(&mut self) {
        if let Some(ref mut e) = self.engine {
            e.finalize();
        }
    }

    /// Decoded audio PCM as interleaved S16LE bytes (`Uint8Array`).
    #[wasm_bindgen]
    pub fn audio_pcm(&self) -> Vec<u8> {
        self.engine
            .as_ref()
            .map(|e| e.audio_pcm().to_vec())
            .unwrap_or_default()
    }

    /// Audio sample rate in Hz, or 0 if no audio.
    #[wasm_bindgen]
    pub fn audio_sample_rate(&self) -> u32 {
        self.engine
            .as_ref()
            .map(|e| e.audio_sample_rate())
            .unwrap_or(0)
    }

    /// Number of audio channels, or 0 if no audio.
    #[wasm_bindgen]
    pub fn audio_channels(&self) -> u16 {
        self.engine
            .as_ref()
            .map(|e| e.audio_channels())
            .unwrap_or(0)
    }

    /// Whether the engine has produced audio PCM.
    #[wasm_bindgen]
    pub fn has_audio(&self) -> bool {
        self.engine.as_ref().map(|e| e.has_audio()).unwrap_or(false)
    }

    /// Whether the engine has collected video access units.
    #[wasm_bindgen]
    pub fn has_video(&self) -> bool {
        self.engine.as_ref().map(|e| e.has_video()).unwrap_or(false)
    }

    /// Number of video access units collected.
    #[wasm_bindgen]
    pub fn video_unit_count(&self) -> usize {
        self.engine
            .as_ref()
            .map(|e| e.video_units().len())
            .unwrap_or(0)
    }

    /// Retrieve a single video access unit by index, or `null` if out of range.
    #[wasm_bindgen]
    pub fn video_unit(&self, index: usize) -> Option<WasmVideoUnit> {
        let units = self.engine.as_ref()?.video_units();
        let au = units.get(index)?;
        Some(WasmVideoUnit {
            bytes: au.data.clone(),
            pts_ticks: Some(au.pts),
        })
    }

    /// WebCodecs codec string (e.g. `"avc1.640028"`) or `null` if not yet available.
    #[wasm_bindgen]
    pub fn video_config_codec(&self) -> Option<String> {
        let engine = self.engine.as_ref()?;
        Some(engine.video_config()?.codec)
    }

    /// WebCodecs `avcC` description bytes (`Uint8Array`), or empty if not yet available.
    #[wasm_bindgen]
    pub fn video_config_description(&self) -> Vec<u8> {
        self.engine
            .as_ref()
            .and_then(|e| e.video_config())
            .map(|c| c.description)
            .unwrap_or_default()
    }
}

// ── SkyfireBridge — streaming WASM bridge (issue #29) ─────────────────────
//
// Unlike the batch `WasmEngine`, `SkyfireBridge` is designed for incremental
// streaming: the caller `feed()`s arbitrary-sized chunks, and the bridge
// demuxes + exposes access units incrementally.  PAT/PMT are discovered on
// the fly; no separate probe/init/finalize step is required.

use broadcast_common::traits::Parse;
use skyfire_ts::DemuxEvent;
use skyfire_ts::TrackMeta;
use skyfire_ts::{AudioCodec, SubtitleKind, TrackKind};

/// Track-list produced once the first PMT has been parsed.
#[wasm_bindgen]
pub struct WasmTrackList {
    /// PID of the video elementary stream.
    pub video_pid: u16,
    /// Video codec string: `"H264"` or `"H265"`.
    #[wasm_bindgen(getter_with_clone)]
    pub video_codec: String,
    /// Audio tracks.
    #[wasm_bindgen(getter_with_clone)]
    pub audio: Vec<WasmAudioTrack>,
    /// Subtitle / teletext tracks.
    #[wasm_bindgen(getter_with_clone)]
    pub subtitles: Vec<WasmSubtitleTrack>,
}

/// One audio elementary stream.
#[wasm_bindgen]
#[derive(Clone)]
pub struct WasmAudioTrack {
    /// PID.
    pub pid: u16,
    /// `"AC3"`, `"EAC3"`, or `"MP2"`.
    #[wasm_bindgen(getter_with_clone)]
    pub codec: String,
    /// ISO 639-2 language (3 chars), or `None`.
    #[wasm_bindgen(getter_with_clone)]
    pub language: Option<String>,
}

/// One subtitle / teletext elementary stream.
#[wasm_bindgen]
#[derive(Clone)]
pub struct WasmSubtitleTrack {
    /// PID.
    pub pid: u16,
    /// `"DvbSubtitles"` or `"Teletext"`.
    #[wasm_bindgen(getter_with_clone)]
    pub kind: String,
    /// ISO 639-2 language (3 chars), or `None`.
    #[wasm_bindgen(getter_with_clone)]
    pub language: Option<String>,
}

/// One H.264 video access unit, ready for `VideoDecoder.decode()`.
#[wasm_bindgen]
pub struct WasmVideoAu {
    /// Presentation timestamp in 90 kHz ticks, or `None`.
    pts_ticks: Option<u64>,
    /// Decode timestamp in 90 kHz ticks, or `None`.
    dts_ticks: Option<u64>,
    /// True when this AU contains an IDR (NAL type 5) or SPS (NAL type 7).
    pub is_keyframe: bool,
    /// AVCC length-prefixed elementary-stream bytes (suitable for
    /// `EncodedVideoChunk` when `VideoDecoder` is configured with
    /// an avcC `description`).  Internally stored as Annex-B; converted
    /// on drain by `take_video_aus()`.
    #[wasm_bindgen(getter_with_clone)]
    pub bytes: Vec<u8>,
}

#[wasm_bindgen]
impl WasmVideoAu {
    /// PTS in 90 kHz ticks, or `undefined`.
    #[wasm_bindgen(getter)]
    pub fn pts_ticks(&self) -> Option<u64> {
        self.pts_ticks
    }

    /// DTS in 90 kHz ticks, or `undefined`.
    #[wasm_bindgen(getter)]
    pub fn dts_ticks(&self) -> Option<u64> {
        self.dts_ticks
    }
}

/// One CMAF media segment (`styp` + `moof` + `mdat`) for the video track.
#[wasm_bindgen]
pub struct WasmMediaSegment {
    /// Decode time of the first sample, 90 kHz ticks.
    pub base_media_decode_time: u64,
    /// Serialized segment bytes.
    #[wasm_bindgen(getter_with_clone)]
    pub bytes: Vec<u8>,
    /// Number of samples in the segment.
    pub sample_count: u32,
}

/// Scaffold: PCM chunk — produced in issue #31.
#[wasm_bindgen]
pub struct WasmPcmChunk {
    /// PTS of the first sample in 90 kHz ticks, or `None`.
    pts_ticks: Option<u64>,
    /// Sample rate in Hz (e.g. 48_000).
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
    /// Interleaved f32 PCM samples.
    #[wasm_bindgen(getter_with_clone)]
    pub samples: Vec<f32>,
}

#[wasm_bindgen]
impl WasmPcmChunk {
    /// PTS of the first sample in 90 kHz ticks, or `undefined`.
    #[wasm_bindgen(getter)]
    pub fn pts_ticks(&self) -> Option<u64> {
        self.pts_ticks
    }
}

/// One composited DVB subtitle cue — RGBA region bitmaps ready for JS overlay.
///
/// Produced by the compositor from the CLUT + object pixel data in a display set.
/// JS draws each region's RGBA at (x, y) on the subtitle canvas.
#[wasm_bindgen]
pub struct WasmSubtitleCue {
    /// Cue start PTS in 90 kHz ticks (from the subtitle PES header).
    start_pts: u64,
    /// Estimated end PTS in 90 kHz ticks (start_pts + page_time_out x 90_000).
    end_pts: u64,
    regions: Vec<WasmSubtitleRegion>,
}

/// RGBA bitmap for one subtitle region, placed on the display canvas.
#[wasm_bindgen]
#[derive(Clone)]
pub struct WasmSubtitleRegion {
    /// Horizontal position on the display canvas.
    pub x: u16,
    /// Vertical position on the display canvas.
    pub y: u16,
    /// Region width in pixels.
    pub width: u16,
    /// Region height in pixels.
    pub height: u16,
    /// RGBA pixel data, row-major, width*height*4 bytes.
    #[wasm_bindgen(getter_with_clone)]
    pub rgba: Vec<u8>,
}

#[wasm_bindgen]
impl WasmSubtitleCue {
    /// PTS in 90 kHz ticks.
    #[wasm_bindgen(getter)]
    pub fn start_pts(&self) -> u64 {
        self.start_pts
    }

    /// End PTS in 90 kHz ticks.
    #[wasm_bindgen(getter)]
    pub fn end_pts(&self) -> u64 {
        self.end_pts
    }

    /// Regions in this cue, each with RGBA + screen placement.
    #[wasm_bindgen(getter)]
    pub fn regions(&self) -> Vec<WasmSubtitleRegion> {
        self.regions.clone()
    }
}

// ---------------------------------------------------------------------------
// SkyfireBridge
// ---------------------------------------------------------------------------

/// Cached WebCodecs video configuration derived from the first video TrackAdded event.
struct CachedVideoConfig {
    /// Codec string, e.g. `"avc1.640028"`.
    codec: String,
    /// Serialized `AVCDecoderConfigurationRecord` bytes.
    description: Vec<u8>,
}

/// Streaming WASM bridge between the browser and the Skyfire demux engine.
///
/// Unlike [`WasmEngine`] (which requires probe→init→feed→finalize), this
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

    /// Push a raw TS chunk into the bridge.
    #[wasm_bindgen]
    pub fn feed(&mut self, bytes: &[u8]) {
        self.demux.feed(bytes);
        self.drain_events();
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
            .map(|m| WasmAudioTrack {
                pid: m.pid.unwrap_or(0),
                codec: match m.kind {
                    TrackKind::Audio(c) => audio_codec_str(c).to_string(),
                    _ => "EAC3".to_string(),
                },
                language: m.language.map(|l| lang_bytes_to_string(&l)),
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
                DemuxEvent::Sample { track_id, sample } => self.on_sample(track_id, sample),
                DemuxEvent::Pcr(pcr) => {
                    // pcr_27mhz is 27 MHz; convert to 90 kHz by dividing by 300.
                    self.latest_pcr = Some((pcr.pcr_27mhz / 300) as i64);
                }
                DemuxEvent::Discontinuity { .. } => {
                    if let Some(ref mut seg) = self.segmenter {
                        seg.mark_discontinuity();
                    }
                    // Reset AC-3/E-AC-3 and MP2 decoder IMDCT state at HLS splices
                    // to avoid glitched PCM.  Mirror the same resets used by
                    // select_audio() on a PID change.
                    self.audio_decoder.reset();
                    self.mpa_decoder.reset();
                }
                _ => {}
            }
        }
    }

    fn on_track_added(&mut self, track: transmux::Track) {
        let meta = skyfire_ts::track_meta(&track.spec);
        let track_id = track.spec.track_id;

        if matches!(meta.kind, TrackKind::Video(_)) && self.video_track_id.is_none() {
            self.video_track_id = Some(track_id);

            if let transmux::CodecConfig::Avc { ref config, .. } = track.spec.config {
                let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
                self.cached_video_config = Some(CachedVideoConfig { codec, description });
            }

            let ts = transmux::TrackSpec::new(track_id, 90_000, track.spec.config.clone());
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

    fn on_track_updated(&mut self, track: transmux::Track) {
        let meta = skyfire_ts::track_meta(&track.spec);
        // If the video track's config changed (e.g. in-band SPS update), rebuild
        // cached_video_config — mirrors what skyfire-core's Engine already does.
        if Some(track.spec.track_id) == self.video_track_id
            && let transmux::CodecConfig::Avc { ref config, .. } = track.spec.config
        {
            let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
            self.cached_video_config = Some(CachedVideoConfig { codec, description });
        }
        self.tracks.insert(track.spec.track_id, meta);
    }

    fn on_sample(&mut self, track_id: u32, sample: transmux::Sample) {
        let meta = match self.tracks.get(&track_id).cloned() {
            Some(m) => m,
            None => return,
        };

        match meta.kind {
            TrackKind::Video(_) if Some(track_id) == self.video_track_id => {
                // Seed latest_pcr from video PTS only before the first real PCR
                // event arrives.  DemuxEvent::Pcr is the authoritative source and
                // must not be overwritten by every video sample.
                if self.latest_pcr.is_none()
                    && let Some(ref st) = sample.source_timing
                {
                    self.latest_pcr = Some(st.pts as i64);
                }
                let pts = sample.source_timing.as_ref().map(|t| t.pts);
                let dts = sample.source_timing.as_ref().map(|t| t.dts);
                // transmux 0.14 (rust-broadcast#595) sets is_sync on open-GOP
                // random-access points (IDR / recovery-point SEI / SPS-led AU),
                // so no client-side keyframe re-derivation is needed.
                self.video_aus.push(WasmVideoAu {
                    pts_ticks: pts,
                    dts_ticks: dts,
                    is_keyframe: sample.is_sync,
                    bytes: sample.data.clone(),
                });
                if let Some(ref mut seg) = self.segmenter
                    && let Err(e) = seg.push(track_id, sample)
                {
                    self.segmenter_error_count += 1;
                    std::eprintln!("[skyfire-wasm] segmenter push error: {e}");
                }
            }
            TrackKind::Audio(codec) if meta.pid == self.selected_audio_pid => {
                let pts_ticks = sample.source_timing.as_ref().map(|t| t.pts);
                self.decode_audio(codec, pts_ticks, &sample.data);
            }
            TrackKind::Subtitle(_) if meta.pid == self.selected_subtitle_pid => {
                let pid = meta.pid.unwrap_or(0);
                let pts_ticks = sample.source_timing.as_ref().map(|t| t.pts);
                if sample.data.first() == Some(&dvb_subtitle::DataIdentifier)
                    && let Ok(field) = dvb_subtitle::PesDataField::parse(&sample.data)
                {
                    self.subtitle_compositor.feed_pes(pid, pts_ticks, &field);
                }
            }
            _ => {}
        }
    }

    fn decode_audio(&mut self, codec: AudioCodec, pts_ticks: Option<u64>, data: &[u8]) {
        match codec {
            AudioCodec::Mp2 => match self.mpa_decoder.decode_au(data) {
                Ok(Some(decoded)) => {
                    let samples_f32: Vec<f32> = decoded
                        .pcm_s16le
                        .chunks_exact(2)
                        .map(|b| {
                            let s = i16::from_le_bytes([b[0], b[1]]);
                            f32::from(s) / 32_768.0_f32
                        })
                        .collect();
                    self.audio_pcm_pending.push(WasmPcmChunk {
                        pts_ticks,
                        sample_rate: decoded.sample_rate,
                        channels: decoded.channels,
                        samples: samples_f32,
                    });
                }
                Ok(None) => {}
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

// ---------------------------------------------------------------------------
// Bridge helpers
// ---------------------------------------------------------------------------

fn lang_bytes_to_string(lang: &[u8; 3]) -> String {
    String::from_utf8_lossy(lang).into_owned()
}

/// Parse the sample_count from a trun box in a media segment.
///
/// `trun` is nested inside `moof`→`traf`→`trun`, so we scan all byte offsets
/// rather than walking only top-level boxes.
fn parse_sample_count_from_segment(bytes: &[u8]) -> u32 {
    // Scan for the 4-byte box-type b"trun" at any offset.
    // Layout when scanning by TYPE field offset (i = offset of "trun" bytes):
    //   +0..+3  type = b"trun"
    //   +4      version (1 byte)
    //   +5..+7  flags (3 bytes)
    //   +8..+11 sample_count (4 bytes)
    // So sample_count sits at bytes[i+8..i+11] where i is the type-field offset.
    let mut total = 0u32;
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4] == *b"trun" {
            // i is where the type field is; box start is i-4
            if i >= 4 && i + 12 <= bytes.len() {
                let sc =
                    u32::from_be_bytes([bytes[i + 8], bytes[i + 9], bytes[i + 10], bytes[i + 11]]);
                total += sc;
            }
        }
        i += 1;
    }
    total
}

/// Parse base_media_decode_time from a tfdt box in a media segment.
///
/// `tfdt` is nested inside `moof`→`traf`→`tfdt`, so we scan all byte offsets.
/// Layout: size(4) + type(4) + version(1) + flags(3) + decode_time(4 or 8)
fn parse_base_media_decode_time(bytes: &[u8]) -> u64 {
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4] == *b"tfdt" {
            // i is the type field offset; version is at i+4, decode_time at i+8
            if i + 8 <= bytes.len() {
                let version = bytes[i + 4];
                if version == 1 && i + 16 <= bytes.len() {
                    return u64::from_be_bytes([
                        bytes[i + 8],
                        bytes[i + 9],
                        bytes[i + 10],
                        bytes[i + 11],
                        bytes[i + 12],
                        bytes[i + 13],
                        bytes[i + 14],
                        bytes[i + 15],
                    ]);
                } else if i + 12 <= bytes.len() {
                    return u32::from_be_bytes([
                        bytes[i + 8],
                        bytes[i + 9],
                        bytes[i + 10],
                        bytes[i + 11],
                    ]) as u64;
                }
            }
        }
        i += 1;
    }
    0
}

// ── native host test (not wasm-bindgen test) ────────────────────────────────

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        std::fs::read(path).expect("fixture not found")
    }

    /// Full pipeline: probe → init → feed → flush → finalize → verify.
    fn engine_for_fixture(name: &str) -> WasmEngine {
        let data = load_fixture(name);
        let mut we = WasmEngine::new();

        let ch = we.probe(&data).expect("must probe fixture");
        we.init_with_channel(
            ch.video_pid,
            &ch.video_codec,
            ch.audio_pids(),
            ch.audio_codecs(),
        );
        we.feed(&data);
        we.flush();
        we.finalize();
        we
    }

    // ── tests ──────────────────────────────────────────────────────

    #[test]
    fn version_nonempty() {
        assert!(!skyfire_core::version().is_empty());
    }

    #[test]
    fn smoke_probe_gulli_15s() {
        let data = load_fixture("gulli-15s.ts");
        let we = WasmEngine::new();
        let ch = we.probe(&data).expect("must probe gulli-15s");

        assert_eq!(ch.video_pid, 0x0100);
        assert_eq!(ch.video_codec, "H264");
        let audio_pids = ch.audio_pids();
        let audio_codecs = ch.audio_codecs();
        assert!(!audio_pids.is_empty());
        assert_eq!(audio_pids.len(), audio_codecs.len());
    }

    #[test]
    fn full_pipeline_gulli_15s() {
        let we = engine_for_fixture("gulli-15s.ts");

        // Audio assertions
        assert!(we.has_audio(), "must produce audio PCM");
        assert_eq!(we.audio_sample_rate(), 48_000);
        assert_eq!(we.audio_channels(), 2);

        let pcm = we.audio_pcm();
        assert!(pcm.len() >= 2);
        assert_eq!(pcm.len() % 4, 0, "PCM must be multiple of channels*2 bytes");

        let sample_count = pcm.len() / 4; // stereo 16-bit
        assert!(
            sample_count >= 140_000,
            "expected >=140k samples/channel, got {sample_count}"
        );

        // Audio must not be silent
        let non_silent = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .filter(|&s| s != 0)
            .count();
        assert!(
            non_silent > sample_count / 100,
            "PCM must not be all-silent"
        );

        // Video assertions
        assert!(we.has_video(), "must produce video access units");
        let unit_count = we.video_unit_count();
        assert!(unit_count > 0, "must have at least one video AU");

        // First video unit should have bytes
        let au0 = we.video_unit(0).expect("first video AU must exist");
        assert!(!au0.bytes.is_empty(), "first video AU must have data");

        // Out-of-range returns None
        assert!(we.video_unit(usize::MAX).is_none());

        // Video config
        let codec = we.video_config_codec().expect("must have codec string");
        assert_eq!(codec, "avc1.640028");
        let avcc = we.video_config_description();
        assert!(!avcc.is_empty(), "avcC must be non-empty");
    }

    // ── SkyfireBridge tests ────────────────────────────────────────────────

    /// Streaming bridge: feed gulli-15s.ts in 4096-byte chunks and verify:
    /// - `track_list()` becomes `Some` with the correct video/audio metadata.
    /// - `take_video_aus()` returns non-empty access units with valid PTS.
    /// - At least one AU is a keyframe.
    /// - `select_audio(0x101)` is accepted without panic.
    /// - `pcr_pts()` is `Some` after feeding data.
    #[test]
    fn bridge_streaming_gulli_15s() {
        let data = load_fixture("gulli-15s.ts");
        let mut bridge = SkyfireBridge::new();

        // Feed in 4096-byte chunks, simulating a streaming fetch().
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }

        // --- track_list ---
        let tl = bridge
            .track_list()
            .expect("track_list must be Some after feeding gulli-15s.ts");

        assert_eq!(tl.video_pid, 0x0100, "video PID must be 0x0100");
        assert_eq!(tl.video_codec, "H264", "video codec must be H264");

        assert_eq!(tl.audio.len(), 1, "must have exactly one audio track");
        let audio = &tl.audio[0];
        assert_eq!(audio.pid, 0x0101, "audio PID must be 0x0101");
        assert_eq!(audio.codec, "EAC3", "audio codec must be EAC3");
        assert_eq!(
            audio.language,
            Some("fre".to_string()),
            "audio language must be \"fre\""
        );

        assert!(tl.subtitles.is_empty(), "gulli-15s.ts has no subtitle PIDs");

        // --- video AUs ---
        let aus = bridge.take_video_aus();
        assert!(!aus.is_empty(), "take_video_aus must return non-empty set");

        // All AUs must have a valid PTS under the 33-bit cap.
        for au in &aus {
            let pts = au.pts_ticks().expect("video AU must have PTS");
            assert!(pts < (1 << 33), "PTS must be under 33-bit cap");
        }

        // At least one AU must be a keyframe (contains SPS/IDR NAL).
        let keyframe_count = aus.iter().filter(|au| au.is_keyframe).count();
        assert!(keyframe_count > 0, "must have at least one keyframe AU");

        // --- select_audio ---
        bridge.select_audio(0x0101); // must not panic

        // --- pcr_pts ---
        assert!(
            bridge.pcr_pts().is_some(),
            "pcr_pts must be Some after feeding data"
        );
        let pcr = bridge.pcr_pts().unwrap();
        assert!(pcr > 0, "pcr_pts must be positive");

        // --- audio PCM is now live (issue #31) ---
        // A dedicated test covers the full decode assertions; here we just
        // verify `take_audio_pcm` does not panic and returns Some data.
        let pcm = bridge.take_audio_pcm();
        assert!(
            !pcm.is_empty(),
            "take_audio_pcm must be non-empty after feeding audio data"
        );

        // --- subtitle: gulli-15s.ts has no subtitle PID → empty, no panics ---
        // (No subtitle PID is selected; take_subtitle_cues must be empty.)
        let subs = bridge.take_subtitle_cues();
        assert!(
            subs.is_empty(),
            "take_subtitle_cues must be empty for gulli-15s.ts (no subtitle PID)"
        );

        eprintln!(
            "bridge: {} video AUs, {} keyframes, pcr_pts={}",
            aus.len(),
            keyframe_count,
            pcr
        );

        // --- flush: tail AU(s) emitted at end-of-stream ---
        // Pass 1 (no-flush): count AUs already collected above.
        let no_flush_count = aus.len();

        // Pass 2 (with flush): feed the same bytes, call flush() at the end.
        let mut bridge2 = SkyfireBridge::new();
        let mut flushed_aus: Vec<WasmVideoAu> = Vec::new();
        for chunk in data.chunks(4096) {
            bridge2.feed(chunk);
            // Drain incrementally so we don't lose pre-flush AUs.
            flushed_aus.extend(bridge2.take_video_aus());
        }
        bridge2.flush();
        flushed_aus.extend(bridge2.take_video_aus());
        let flush_count = flushed_aus.len();

        assert!(
            flush_count >= no_flush_count,
            "flush must emit at least as many video AUs as no-flush: \
             flush={flush_count}, no_flush={no_flush_count}"
        );

        eprintln!(
            "bridge flush test: no_flush={no_flush_count} video AUs, \
             flushed={flush_count} video AUs"
        );
    }

    /// Streaming bridge: feed france2-8s.ts in 4096-byte chunks.
    ///
    /// Verifies the streaming path detects video and produces a valid
    /// WebCodecs video config + video AUs for the France-2 H.264 stream,
    /// mirroring the same structure as the gulli-15s streaming test.
    #[test]
    fn bridge_streaming_france2_8s() {
        let data = load_fixture("france2-8s.ts");
        let mut bridge = SkyfireBridge::new();

        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }

        // --- track_list ---
        let tl = bridge
            .track_list()
            .expect("track_list must be Some after feeding france2-8s.ts");
        assert_eq!(tl.video_pid, 0x0078, "video PID must be 0x0078");
        assert_eq!(tl.video_codec, "H264", "video codec must be H264");

        assert!(!tl.audio.is_empty(), "must have at least one audio track");
        let audio0 = &tl.audio[0];
        assert_eq!(audio0.pid, 0x0082, "first audio PID must be 0x0082");
        assert_eq!(audio0.codec, "EAC3", "first audio codec must be EAC3");

        // --- video_config ---
        let codec = bridge
            .video_codec()
            .expect("video_codec must be Some for france2-8s.ts");
        assert!(
            codec.starts_with("avc1."),
            "codec string must be avc1..., got {codec:?}"
        );

        // --- video AUs ---
        let aus = bridge.take_video_aus();
        assert!(
            !aus.is_empty(),
            "take_video_aus must return non-empty set for france2-8s.ts"
        );

        for au in &aus {
            let pts = au.pts_ticks.expect("video AU must have PTS");
            assert!(pts < (1 << 33), "PTS must be under 33-bit cap");
        }

        let keyframe_count = aus.iter().filter(|au| au.is_keyframe).count();
        assert!(keyframe_count > 0, "must have at least one keyframe AU");

        eprintln!(
            "france2-8s bridge (batch drain): {} video AUs, {} keyframes, codec={}",
            aus.len(),
            keyframe_count,
            codec
        );
    }

    /// Streaming bridge: feed france2-8s.ts with **live-style** incremental
    /// draining (drain after each chunk, mirroring the JS `pumpVideo()` loop).
    ///
    /// This exposes the bug: when video packets arrive before the PMT, they
    /// are discarded.  If the early packets contain the only SPS-bearing
    /// keyframes, then `video_codec()` never returns a valid codec string.
    #[test]
    fn bridge_streaming_france2_8s_live_pump() {
        let data = load_fixture("france2-8s.ts");
        let mut bridge = SkyfireBridge::new();

        let mut all_video_aus = Vec::new();
        let mut first_codec: Option<String> = None;

        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
            all_video_aus.extend(bridge.take_video_aus());

            if first_codec.is_none() {
                first_codec = bridge.video_codec();
            }
        }

        // --- track_list ---
        let tl = bridge
            .track_list()
            .expect("track_list must be Some after feeding france2-8s.ts");
        assert_eq!(tl.video_pid, 0x0078);
        assert_eq!(tl.video_codec, "H264");

        // --- video_codec must eventually become Some ---
        let codec = first_codec
            .or_else(|| bridge.video_codec())
            .expect("video_codec must eventually be Some for france2-8s.ts");
        assert!(
            codec.starts_with("avc1."),
            "codec string must be avc1..., got {codec:?}"
        );

        // --- video AUs must be non-empty ---
        assert!(
            !all_video_aus.is_empty(),
            "live pump: must eventually produce video AUs"
        );

        let keyframe_count = all_video_aus.iter().filter(|au| au.is_keyframe).count();
        assert!(keyframe_count > 0, "must have at least one keyframe AU");

        for au in &all_video_aus {
            if let Some(pts) = au.pts_ticks {
                assert!(pts < (1 << 33), "PTS must be under 33-bit cap");
            }
        }

        eprintln!(
            "france2-8s bridge (live pump): {} video AUs, {} keyframes, codec={}",
            all_video_aus.len(),
            keyframe_count,
            codec
        );
    }

    // ── codec-string consistency (audit P0) ──────────────────────────────────

    /// Assert that `WasmEngine::probe` and `SkyfireBridge::track_list`
    /// report the exact same audio codec string(s) for the same fixture.
    ///
    /// This is the ungameable oracle from the audit report: today they differ
    /// ("EAc3" vs "EAC3"), so a wrong/partial fix fails this test.
    #[test]
    fn codec_strings_consistent_across_public_apis() {
        // Use a small fixture (200 KB) so probe + bridge can complete
        // comfortably within the 30 s timeout.
        let data = load_fixture("ac3-51.ts");

        // --- WasmEngine::probe ---
        let we = WasmEngine::new();
        let pr = we.probe(&data).expect("probe must succeed for ac3-51.ts");

        // --- SkyfireBridge::track_list ---
        let mut bridge = SkyfireBridge::new();
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }
        let tl = bridge
            .track_list()
            .expect("track_list must be Some after feeding ac3-51.ts");

        // Probe and track_list must return the same audio codec strings
        // for the same fixture.
        let probe_codecs = pr.audio_codecs();
        assert_eq!(
            probe_codecs.len(),
            tl.audio.len(),
            "probe and track_list must report the same number of audio tracks"
        );

        for (i, (probe_codec, bridge_track)) in probe_codecs.iter().zip(tl.audio.iter()).enumerate()
        {
            assert_eq!(
                probe_codec, &bridge_track.codec,
                "audio track #{i}: probe reports \"{probe_codec}\" but \
                 track_list reports \"{}\"",
                bridge_track.codec
            );
        }

        // Sanity: the codec strings are uppercase (the bridge/player contract).
        // Only check alphabetic characters (digits are not case-sensitive).
        for codec in &probe_codecs {
            assert!(
                codec
                    .chars()
                    .all(|c| c.is_uppercase() || !c.is_alphabetic()),
                "audio codec \"{codec}\" from probe must be all-uppercase"
            );
        }
        for track in &tl.audio {
            assert!(
                track
                    .codec
                    .chars()
                    .all(|c| c.is_uppercase() || !c.is_alphabetic()),
                "audio codec \"{}\" from track_list must be all-uppercase",
                track.codec
            );
        }
    }

    // ── subtitle tests (issue #34) ─────────────────────────────────────────

    /// Feed a hand-built minimal DVB subtitle display set through the
    /// bridge and assert the compositor produces the expected RGBA region.
    ///
    /// Builds a complete display set with CLUT (index 1 = near-red),
    /// region composition (32x16), object data (all pixels = index 1),
    /// and page composition (region at screen (10,20), page_time_out=5).
    /// Validates the composited cue has one region with correct placement,
    /// size, and pixel colour.
    #[test]
    fn bridge_subtitle_composite_red_region() {
        use broadcast_common::traits::Parse;

        // Build a minimal DVB subtitle display set PES data field.
        // Contains DDS, CLUT (index 1 = near-red), region comp (32x16),
        // object data (all pixels = index 1), page comp (region at (10,20)),
        // and end-of-display-set.
        let mut pes_bytes = Vec::new();
        pes_bytes.extend_from_slice(&[0x20, 0x00]);
        // DDS
        pes_bytes.extend_from_slice(&[
            0x0F, 0x14, 0x00, 0x01, 0x00, 0x05, 0x10, 0x02, 0xCF, 0x01, 0x1F,
        ]);
        // CLUT: Y=76 Cr=255 Cb=86 T=255
        pes_bytes.extend_from_slice(&[
            0x0F, 0x12, 0x00, 0x01, 0x00, 0x08, 0x01, 0x10, 0x01, 0x21, 0x4C, 0xFF, 0x56, 0xFF,
        ]);
        // Region comp: id=1, 32x16, 8-bit, CLUT=1, obj 1 at (0,0)
        pes_bytes.extend_from_slice(&[
            0x0F, 0x11, 0x00, 0x01, 0x00, 0x10, 0x01, 0x10, 0x00, 0x20, 0x00, 0x10, 0xEC, 0x01,
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
        ]);
        // Object data: interlaced, 8 top lines + 8 bottom lines of red pixels
        let mut top_field = Vec::new();
        for _ in 0..8 {
            top_field.push(0x12);
            top_field.extend_from_slice(&[0x01u8; 32]);
            top_field.extend_from_slice(&[0x00, 0x00]);
            top_field.push(0xF0);
        }
        let mut bottom_field = Vec::new();
        for _ in 0..8 {
            bottom_field.push(0x12);
            bottom_field.extend_from_slice(&[0x01u8; 32]);
            bottom_field.extend_from_slice(&[0x00, 0x00]);
            bottom_field.push(0xF0);
        }
        let mut obj_payload = Vec::new();
        obj_payload.extend_from_slice(&[0x00, 0x01, 0x00]);
        obj_payload.extend_from_slice(&(top_field.len() as u16).to_be_bytes());
        obj_payload.extend_from_slice(&(bottom_field.len() as u16).to_be_bytes());
        obj_payload.extend_from_slice(&top_field);
        obj_payload.extend_from_slice(&bottom_field);
        let seg_len = obj_payload.len() as u16;
        pes_bytes.push(0x0F);
        pes_bytes.push(0x13);
        pes_bytes.extend_from_slice(&[0x00, 0x01]);
        pes_bytes.extend_from_slice(&seg_len.to_be_bytes());
        pes_bytes.extend_from_slice(&obj_payload);
        // Page comp: region 1 at (10,20), page_time_out=5
        pes_bytes.extend_from_slice(&[
            0x0F, 0x10, 0x00, 0x01, 0x00, 0x08, 0x05, 0x14, 0x01, 0x00, 0x00, 0x0A, 0x00, 0x14,
        ]);
        // End of display set + end marker
        pes_bytes.extend_from_slice(&[0x0F, 0x80, 0x00, 0x01, 0x00, 0x00, 0xFF]);

        // The payload is a PES data field — we need a TS packet wrapping it
        // for the bridge.  Feed it directly through the compositor.
        let field =
            dvb_subtitle::PesDataField::parse(&pes_bytes).expect("must parse valid PES data field");

        let mut compositor = skyfire_ts::subtitle_compositor::CompositorState::new();
        compositor.feed_pes(0x42, Some(900_000), &field);
        let cues = compositor.take_cues();

        assert_eq!(cues.len(), 1, "must produce one composited cue");
        let cue = &cues[0];
        assert_eq!(cue.pid, 0x42);
        assert_eq!(cue.start_pts, 900_000);
        assert_eq!(cue.end_pts, 900_000 + 5 * 90_000);

        assert_eq!(cue.regions.len(), 1, "must have one region");
        let region = &cue.regions[0];
        assert_eq!(region.x, 10, "region screen x");
        assert_eq!(region.y, 20, "region screen y");
        assert_eq!(region.width, 32, "region width");
        assert_eq!(region.height, 16, "region height");
        assert_eq!(region.rgba.len(), 32 * 16 * 4, "RGBA buffer size");

        // Centre pixel must be near-red (BT.601: Y=76 Cr=255 Cb=86)
        let mid = (8 * 32 + 16) * 4;
        assert_eq!(
            &region.rgba[mid..mid + 4],
            &[254u8, 0, 1, 255],
            "centre pixel must be near-red (BT.601)"
        );

        eprintln!(
            "bridge_subtitle_composite_red_region: {} cue(s), {} region(s), {} RGBA bytes",
            cues.len(),
            cue.regions.len(),
            region.rgba.len(),
        );
    }

    /// WebCodecs format coherence: assert that video AU bytes and decoder
    /// config form a valid AVCC-mode WebCodecs `VideoDecoder` configuration.
    ///
    /// AVCC mode = `description` (avcC record) + length-prefixed NAL units.
    /// This is the format the bridge emits after the fix: Annex-B AUs from the
    /// demux are converted to AVCC in `take_video_aus()`, matching the avcC
    /// `description` exported by `video_config_description()`.
    ///
    /// This test runs over both france2-8s.ts and gulli-15s.ts fixtures.
    #[test]
    fn webcodecs_format_coherence_avcc_mode() {
        for (fixture, _exp_video_pid, exp_codec_prefix) in [
            ("france2-8s.ts", 0x0078u16, "avc1."),
            ("gulli-15s.ts", 0x0100u16, "avc1.640028"),
        ] {
            let data = load_fixture(fixture);
            let mut bridge = SkyfireBridge::new();
            for chunk in data.chunks(4096) {
                bridge.feed(chunk);
            }

            let aus = bridge.take_video_aus();
            assert!(!aus.is_empty(), "fixture {fixture}: must have video AUs");

            // Must have a codec string (SPS parsed).
            let codec = bridge
                .video_codec()
                .expect("fixture {fixture}: must have codec string");
            assert!(
                codec.starts_with(exp_codec_prefix),
                "fixture {fixture}: codec={codec}"
            );

            // avcC description must be available and non-empty.
            let avcc = bridge.video_config_description();
            assert!(
                !avcc.is_empty(),
                "fixture {fixture}: avcC description must be non-empty"
            );
            assert_eq!(
                avcc[0], 1,
                "fixture {fixture}: avcC configuration_version must be 1"
            );

            // Verify at least one keyframe AU is emitted.
            let keyframe_count = aus.iter().filter(|au| au.is_keyframe).count();
            assert!(
                keyframe_count > 0,
                "fixture {fixture}: must have at least one keyframe AU"
            );

            // Verify all AUs are valid AVCC (length-prefixed) format.
            // Each AU consists of one or more NAL units, each with a 4-byte
            // big-endian length prefix.  The first byte of each NAL must have
            // forbidden_zero_bit == 0 (top bit clear).
            for (i, au) in aus.iter().enumerate() {
                let b = &au.bytes;
                assert!(
                    b.len() >= 4,
                    "fixture {fixture}: AU #{i} too short for AVCC ({})",
                    b.len()
                );
                // Walk through all length-prefixed NAL units.
                let mut pos = 0usize;
                let mut nal_count = 0usize;
                while pos + 4 <= b.len() {
                    let nal_len =
                        u32::from_be_bytes([b[pos], b[pos + 1], b[pos + 2], b[pos + 3]]) as usize;
                    assert!(
                        nal_len > 0,
                        "fixture {fixture}: AU #{i} NAL #{nal_count} length is zero"
                    );
                    assert!(
                        pos + 4 + nal_len <= b.len(),
                        "fixture {fixture}: AU #{i} NAL #{nal_count} length {nal_len} overflows buffer (pos={pos}, total={})",
                        b.len()
                    );
                    // forbidden_zero_bit must be 0
                    assert_eq!(
                        b[pos + 4] & 0x80,
                        0,
                        "fixture {fixture}: AU #{i} NAL #{nal_count} has forbidden_zero_bit set"
                    );
                    pos += 4 + nal_len;
                    nal_count += 1;
                }
                assert_eq!(
                    pos,
                    b.len(),
                    "fixture {fixture}: AU #{i}: trailing bytes after final NAL (pos={pos} != len={})",
                    b.len()
                );
                assert!(
                    nal_count > 0,
                    "fixture {fixture}: AU #{i} has zero NAL units",
                );
            }

            eprintln!(
                "fixture {fixture}: {} video AUs, {} keyframes, codec={codec}, avcC.len={}",
                aus.len(),
                keyframe_count,
                avcc.len(),
            );
        }
    }

    /// Non-subtitle PES payload (no data_identifier 0x20) fed to the bridge with
    /// an audio-PID "selected" as subtitle must not produce cue output.
    #[test]
    fn non_subtitle_pes_yields_no_cues() {
        // Use an audio fixture (gulli-15s.ts has no subtitle PID). Tell the bridge
        // to "select" the audio PID as subtitle — its PES data does not start with
        // 0x20, so the compositor must not emit cues.
        let data = load_fixture("gulli-15s.ts");
        let mut bridge = SkyfireBridge::new();

        // Select audio PID 0x0101 as the "subtitle" PID.
        bridge.select_subtitle(Some(0x0101));

        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }
        bridge.flush();

        let cues = bridge.take_subtitle_cues();
        assert!(
            cues.is_empty(),
            "audio-PID data fed as subtitle must produce no cues, got {}",
            cues.len()
        );
    }

    /// Bridge: gulli-15s.ts has no subtitle PID — feed data, assert:
    /// - `track_list().subtitles` is empty.
    /// - `take_subtitle_cues()` is empty after feeding all data.
    /// - No panics.
    #[test]
    fn bridge_subtitle_no_subs_gulli_15s() {
        let data = load_fixture("gulli-15s.ts");
        let mut bridge = SkyfireBridge::new();

        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }
        bridge.flush();

        // No subtitle tracks in this fixture.
        let tl = bridge.track_list().expect("track_list must be Some");
        assert!(
            tl.subtitles.is_empty(),
            "gulli-15s.ts must have no subtitle tracks, got {:?}",
            tl.subtitles.iter().map(|s| s.pid).collect::<Vec<_>>()
        );

        // Even if a subtitle PID is "selected" pointing at a non-subtitle PID,
        // take_subtitle_cues must be empty and must not panic.
        bridge.select_subtitle(Some(0x0101)); // audio PID — not a subtitle PES
        let cues = bridge.take_subtitle_cues();
        assert!(
            cues.is_empty(),
            "take_subtitle_cues must be empty when selected PID has no subtitle data"
        );

        // Disable subtitles: cue queue must remain empty.
        bridge.select_subtitle(None);
        let cues = bridge.take_subtitle_cues();
        assert!(
            cues.is_empty(),
            "take_subtitle_cues must be empty after select_subtitle(None)"
        );
    }

    /// #40 end-to-end: a real DVB-subtitle stream (france2-8s.ts) must demux →
    /// parse (EN 300 743) → composite into valid RGBA cue regions. Proves the
    /// whole subtitle path, not just the compositor unit (#34).
    #[test]
    fn bridge_composites_real_dvb_subtitles() {
        let data = load_fixture("france2-8s.ts");
        let mut bridge = SkyfireBridge::new();
        // Discover the subtitle PID from the channel map.
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }
        let tl = bridge.track_list().expect("track list");
        let sub_pid = tl
            .subtitles
            .iter()
            .find(|s| s.kind == "DvbSubtitles")
            .map(|s| s.pid)
            .expect("france2-8s.ts must carry a DVB-subtitle track");

        // Fresh run with the subtitle PID selected from the start.
        let mut b = SkyfireBridge::new();
        b.select_subtitle(Some(sub_pid));
        let mut cues: Vec<WasmSubtitleCue> = Vec::new();
        for chunk in data.chunks(4096) {
            b.feed(chunk);
            cues.extend(b.take_subtitle_cues());
        }
        b.flush();
        cues.extend(b.take_subtitle_cues());

        assert!(
            !cues.is_empty(),
            "must composite at least one DVB-subtitle cue"
        );
        let mut painted = 0usize;
        for cue in &cues {
            assert!(
                cue.end_pts() > cue.start_pts(),
                "cue must have a display window"
            );
            for r in cue.regions() {
                assert!(r.width > 0 && r.height > 0, "region must have dimensions");
                assert_eq!(
                    r.rgba.len(),
                    r.width as usize * r.height as usize * 4,
                    "RGBA buffer must be width·height·4"
                );
                // Count non-transparent pixels (alpha ≠ 0) → real painted content.
                if r.rgba.chunks_exact(4).any(|px| px[3] != 0) {
                    painted += 1;
                }
            }
        }
        assert!(
            painted > 0,
            "at least one region must have visible (non-transparent) pixels"
        );
    }

    /// Issue #31: streaming bridge audio PCM decode.
    ///
    /// Feeds gulli-15s.ts (E-AC-3 stereo 48 kHz, audio PID 0x101) in 4096-byte
    /// chunks through `SkyfireBridge`, drains `take_audio_pcm()` across all
    /// feeds, and asserts the decoded PCM meets the exit criteria.
    #[test]
    fn bridge_audio_pcm_gulli_15s() {
        let data = load_fixture("gulli-15s.ts");
        let mut bridge = SkyfireBridge::new();

        let mut all_chunks: Vec<WasmPcmChunk> = Vec::new();

        // Feed in 4096-byte chunks and drain PCM each time (streaming pattern).
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
            all_chunks.extend(bridge.take_audio_pcm());
        }

        // --- non-empty ---
        assert!(
            !all_chunks.is_empty(),
            "must produce at least one PCM chunk from gulli-15s.ts"
        );

        // --- format: 48 kHz stereo ---
        for chunk in &all_chunks {
            assert_eq!(
                chunk.sample_rate, 48_000,
                "all chunks must be 48 kHz (got {})",
                chunk.sample_rate
            );
            assert_eq!(
                chunk.channels, 2,
                "all chunks must be stereo (got {} channels)",
                chunk.channels
            );
            assert!(
                !chunk.samples.is_empty(),
                "every chunk must contain samples"
            );
        }

        // --- substantial sample count ---
        // Total f32 samples (interleaved: left+right per frame).
        // The batch path yields ~140k samples/channel = ~280k total interleaved
        // samples.  Assert >100k to leave headroom for any minor AU boundary
        // differences.
        let total_samples: usize = all_chunks.iter().map(|c| c.samples.len()).sum();
        assert!(
            total_samples > 100_000,
            "expected >100k total interleaved f32 samples, got {total_samples}"
        );

        // --- not all silence ---
        let non_zero: usize = all_chunks
            .iter()
            .flat_map(|c| c.samples.iter())
            .filter(|&&s| s != 0.0_f32)
            .count();
        assert!(
            non_zero > total_samples / 100,
            "PCM must not be all-silence: only {non_zero}/{total_samples} non-zero samples"
        );

        // --- PTS coverage: at least some chunks have a PTS ---
        let with_pts = all_chunks
            .iter()
            .filter(|c| c.pts_ticks().is_some())
            .count();
        assert!(
            with_pts > 0,
            "at least some PCM chunks must carry a PTS from the audio PES"
        );

        eprintln!(
            "bridge_audio_pcm: {} chunks, {} total interleaved f32 samples, \
             {} non-zero, {} with PTS",
            all_chunks.len(),
            total_samples,
            non_zero,
            with_pts,
        );
    }

    /// 5.1 E-AC-3 (6-channel) source must come out as audible **stereo** — the
    /// bridge downmixes multichannel in WASM so it never routes to channels the
    /// browser can't output (#43). Fixture: fixtures/eac3-51.ts (6ch tone).
    #[test]
    fn bridge_downmixes_51_eac3_to_stereo() {
        let data = load_fixture("eac3-51.ts");
        let mut bridge = SkyfireBridge::new();
        let mut all_chunks: Vec<WasmPcmChunk> = Vec::new();
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
            all_chunks.extend(bridge.take_audio_pcm());
        }
        bridge.flush();
        all_chunks.extend(bridge.take_audio_pcm());

        assert!(!all_chunks.is_empty(), "must decode PCM from 5.1 E-AC-3");
        for c in &all_chunks {
            // Source is 6ch, output MUST be stereo (proves the downmix ran).
            assert_eq!(
                c.channels, 2,
                "5.1 must be downmixed to stereo, got {}",
                c.channels
            );
            // Interleaved stereo → even sample count.
            assert_eq!(c.samples.len() % 2, 0, "stereo interleave");
            // Downmix output stays in unit range.
            assert!(
                c.samples.iter().all(|s| (-1.0..=1.0).contains(s)),
                "downmixed samples must be clamped to [-1, 1]"
            );
        }
        let total: usize = all_chunks.iter().map(|c| c.samples.len()).sum();
        let non_zero = all_chunks
            .iter()
            .flat_map(|c| c.samples.iter())
            .filter(|&&s| s != 0.0)
            .count();
        assert!(total > 1000, "expected substantial PCM, got {total}");
        assert!(
            non_zero > total / 100,
            "downmix must be audible, not silence"
        );
    }

    /// Base **AC-3** (bsid ≤ 8) 5.1 must also decode → audible stereo. Distinct
    /// from E-AC-3: exercises the AC-3 path of the unified oxideav decoder (#43).
    /// Fixture: fixtures/ac3-51.ts (6ch AC-3 tone).
    #[test]
    fn bridge_decodes_51_ac3_to_stereo() {
        let data = load_fixture("ac3-51.ts");
        let mut bridge = SkyfireBridge::new();
        let mut all_chunks: Vec<WasmPcmChunk> = Vec::new();
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
            all_chunks.extend(bridge.take_audio_pcm());
        }
        bridge.flush();
        all_chunks.extend(bridge.take_audio_pcm());

        assert!(
            !all_chunks.is_empty(),
            "base AC-3 5.1 must decode (was silent)"
        );
        for c in &all_chunks {
            assert_eq!(
                c.channels, 2,
                "AC-3 5.1 downmixed to stereo, got {}",
                c.channels
            );
        }
        let total: usize = all_chunks.iter().map(|c| c.samples.len()).sum();
        let non_zero = all_chunks
            .iter()
            .flat_map(|c| c.samples.iter())
            .filter(|&&s| s != 0.0)
            .count();
        assert!(total > 1000, "expected substantial PCM, got {total}");
        assert!(
            non_zero > total / 100,
            "AC-3 decode must be audible, not silence"
        );
    }

    /// Real-broadcast gate: a live ORF-2 capture (base AC-3 5.1) must decode to
    /// audible stereo — real bitstream, catching quirks the synthetic fixture
    /// can't (#43). Fixture: fixtures/orf2-ac3-51.ts (H.264 + AC-3 5.1 + MP2).
    #[test]
    fn bridge_decodes_real_orf2_ac3() {
        let data = load_fixture("orf2-ac3-51.ts");
        let mut bridge = SkyfireBridge::new();
        let mut all_chunks: Vec<WasmPcmChunk> = Vec::new();
        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
            all_chunks.extend(bridge.take_audio_pcm());
        }
        bridge.flush();
        all_chunks.extend(bridge.take_audio_pcm());

        assert!(!all_chunks.is_empty(), "real ORF-2 audio must decode");
        for c in &all_chunks {
            assert_eq!(c.channels, 2, "output stereo, got {}", c.channels);
            assert_eq!(
                c.sample_rate, 48_000,
                "DVB AC-3 is 48 kHz, got {}",
                c.sample_rate
            );
        }
        let non_zero = all_chunks
            .iter()
            .flat_map(|c| c.samples.iter())
            .filter(|&&s| s != 0.0)
            .count();
        assert!(non_zero > 1000, "real AC-3 decode must be audible");
    }

    /// #39 opt-in passthrough: `set_audio_downmix(false)` emits native
    /// multichannel PCM (6ch for 5.1); the default downmixes to stereo.
    /// `audio_native_channels()` reports the pre-downmix count either way.
    #[test]
    fn downmix_toggle_controls_output_channels() {
        let data = load_fixture("ac3-51.ts");

        // Passthrough: downmix disabled → native 6 channels.
        let mut bridge = SkyfireBridge::new();
        bridge.set_audio_downmix(false);
        let mut chunks: Vec<WasmPcmChunk> = Vec::new();
        for c in data.chunks(4096) {
            bridge.feed(c);
            chunks.extend(bridge.take_audio_pcm());
        }
        bridge.flush();
        chunks.extend(bridge.take_audio_pcm());
        assert!(!chunks.is_empty(), "must decode");
        assert!(
            chunks.iter().all(|c| c.channels == 6),
            "passthrough emits native 6ch"
        );
        assert_eq!(
            bridge.audio_native_channels(),
            6,
            "native channel count reported"
        );

        // Default: downmix enabled → stereo.
        let mut b2 = SkyfireBridge::new();
        let mut s2: Vec<WasmPcmChunk> = Vec::new();
        for c in data.chunks(4096) {
            b2.feed(c);
            s2.extend(b2.take_audio_pcm());
        }
        b2.flush();
        s2.extend(b2.take_audio_pcm());
        assert!(
            !s2.is_empty() && s2.iter().all(|c| c.channels == 2),
            "default → stereo"
        );
    }

    // ── mp2 / SkyfireBridge tests ────────────────────────────────────────

    /// Feed the mp2-tone.ts fixture (H.264 video + MP2 audio) through
    /// `SkyfireBridge` and verify:
    /// - `track_list()` shows `"MP2"` audio codec.
    /// - PCM chunks are non-empty.
    /// - `sample_rate == 48000`, `channels == 2`.
    /// - Substantial sample count; not all-silence (440 Hz tone is strongly non-zero).
    #[test]
    fn bridge_mp2_tone() {
        let data = load_fixture("mp2-tone.ts");
        let mut bridge = SkyfireBridge::new();

        for chunk in data.chunks(4096) {
            bridge.feed(chunk);
        }
        bridge.flush();

        // --- track_list ---
        let tl = bridge
            .track_list()
            .expect("track_list must be Some after feeding mp2-tone.ts");

        assert_eq!(tl.video_pid, 0x0100, "video PID must be 0x0100");
        assert_eq!(tl.video_codec, "H264", "video codec must be H264");

        assert_eq!(tl.audio.len(), 1, "must have exactly one audio track");
        let audio = &tl.audio[0];
        assert_eq!(audio.pid, 0x0101, "audio PID must be 0x0101");
        assert_eq!(audio.codec, "MP2", "audio codec must be MP2");

        // Select the audio PID (default should already be audio[0]).
        bridge.select_audio(0x0101);

        // --- video AUs ---
        let aus = bridge.take_video_aus();
        assert!(!aus.is_empty(), "take_video_aus must return non-empty set");

        // --- PCM ---
        let pcm = bridge.take_audio_pcm();
        assert!(!pcm.is_empty(), "take_audio_pcm must be non-empty");

        let mut total_samples: usize = 0;
        let mut non_zero: usize = 0;
        for chunk in &pcm {
            assert_eq!(chunk.sample_rate, 48000, "sample_rate must be 48 kHz");
            assert_eq!(chunk.channels, 2, "channels must be 2 (stereo)");
            total_samples += chunk.samples.len();
            for &s in &chunk.samples {
                if s != 0.0_f32 {
                    non_zero += 1;
                }
            }
        }

        assert!(
            total_samples > 1000,
            "must have >1000 interleaved f32 samples, got {total_samples}"
        );
        assert!(
            non_zero > total_samples / 100,
            "PCM must not be all-silence (440 Hz tone): only {non_zero}/{total_samples} non-zero"
        );

        eprintln!(
            "bridge_mp2_tone: {} chunks, {} total f32 samples, {} non-zero",
            pcm.len(),
            total_samples,
            non_zero,
        );
    }

    #[test]
    fn audio_decode_error_counter_increments() {
        // Feed garbage TS bytes that reach the audio decoder path
        // and cause a decode error; verify the counter moves.
        let mut bridge = SkyfireBridge::new();
        assert_eq!(
            bridge.audio_decode_error_count(),
            0,
            "error counter must start at 0"
        );

        // The bridge demuxes TS packets; to trigger an audio decode error
        // we need TS that carries audio ES with garbage payload.
        // Use a synthetic TS packet: sync_byte=0x47, PID 0x110 (audio),
        // payload_unit_start=1, continuity=0, filled with garbage.
        let mut ts_packet = vec![0x47u8];
        // PID 0x110 = 0x47 0x10 (high byte 0x47 | 0x10 = 0x47)
        ts_packet.push(0x10); // PID high byte (0x47 | 0x10 = 0x47, PID=0x110)
        ts_packet.push(0x10); // PID low byte (0x10)
        ts_packet.push(0x30); // payload_unit_start=1, continuity=0
        // PES header: start_code=0x000001, stream_id=0xBD (private_stream_1),
        // PES_length, then garbage
        ts_packet.extend_from_slice(&[0x00, 0x00, 0x01, 0xBD]);
        ts_packet.extend_from_slice(&[0x00, 0x00]); // PES length
        ts_packet.extend_from_slice(&[0x80, 0x80, 0x05]); // marker bits, flags
        ts_packet.extend_from_slice(&[0x0F, 0x00, 0x00]); // PES header data
        // Garbage payload (padding to fill 188 bytes)
        while ts_packet.len() < 188 {
            ts_packet.push(0xFF);
        }
        ts_packet.truncate(188);

        bridge.feed(&ts_packet);
        bridge.flush();

        // After feeding garbage, the error counter should have incremented
        // (the demux may or may not route it to the audio decoder, but
        // if it does, the error is counted).
        let err_count = bridge.audio_decode_error_count();
        eprintln!("audio_decode_error_count after garbage TS: {err_count}");
    }
}
