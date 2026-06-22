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
    /// Audio codec identifiers, parallel to `audio_pids`: `"EAc3"` or `"Ac3"`.
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
    #[wasm_bindgen]
    pub fn probe(&self, data: &[u8]) -> Option<ProbeResult> {
        let channel = Engine::probe(data)?;
        let audio_pids: Vec<u16> = channel.audio_streams.iter().map(|s| s.pid).collect();
        let audio_codecs: Vec<String> = channel
            .audio_streams
            .iter()
            .map(|s| audio_codec_str(s.codec).to_string())
            .collect();
        Some(ProbeResult {
            video_pid: channel.video_pid,
            video_codec: video_codec_str(channel.video_codec).to_string(),
            audio_pids,
            audio_codecs,
        })
    }

    /// Initialize the engine from a channel map (typically obtained via `probe()`).
    #[wasm_bindgen]
    pub fn init_with_channel(
        &mut self,
        video_pid: u16,
        video_codec: &str,
        audio_pids: Vec<u16>,
        audio_codecs: Vec<String>,
    ) {
        let vc = match video_codec {
            "H264" => skyfire_core::ts::VideoCodec::H264,
            "H265" => skyfire_core::ts::VideoCodec::H265,
            _ => return,
        };
        let mut streams = Vec::with_capacity(audio_pids.len().min(audio_codecs.len()));
        for (pid, codec_str) in audio_pids.into_iter().zip(audio_codecs.iter()) {
            let ac = match codec_str.as_str() {
                "EAc3" | "AC3" => skyfire_core::ts::AudioCodec::EAc3,
                "Ac3" => skyfire_core::ts::AudioCodec::Ac3,
                _ => skyfire_core::ts::AudioCodec::EAc3,
            };
            streams.push(skyfire_core::ts::AudioStream {
                pid,
                codec: ac,
                language: None,
            });
        }
        let channel = skyfire_core::ts::ChannelMap {
            video_pid,
            video_codec: vc,
            audio_streams: streams,
            subtitle_streams: Vec::new(),
            pcr_pid: video_pid,
        };
        self.engine = Some(Engine::with_channel(channel));
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
            e.flush();
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
            bytes: au.es_bytes.clone(),
            pts_ticks: au.pts_ticks,
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

    /// True when the video stream is interlaced (SPS `frame_mbs_only_flag
    /// == 0`). WebCodecs cannot decode such streams — under ADR 0008 the
    /// server (zenith) deinterlaces to progressive before the browser sees
    /// it, so this should report `false` on a `/skyfire/<slug>` stream;
    /// kept as a diagnostic.
    #[wasm_bindgen]
    pub fn video_is_interlaced(&self) -> bool {
        self.engine
            .as_ref()
            .and_then(|e| e.video_config())
            .map(|c| c.interlaced)
            .unwrap_or(false)
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

fn video_codec_str(c: skyfire_core::ts::VideoCodec) -> &'static str {
    match c {
        skyfire_core::ts::VideoCodec::H264 => "H264",
        skyfire_core::ts::VideoCodec::H265 => "H265",
    }
}

fn audio_codec_str(c: skyfire_core::ts::AudioCodec) -> &'static str {
    match c {
        skyfire_core::ts::AudioCodec::Ac3 => "Ac3",
        skyfire_core::ts::AudioCodec::EAc3 => "EAc3",
    }
}

// ── SkyfireBridge — streaming WASM bridge (issue #29) ─────────────────────
//
// Unlike the batch `WasmEngine`, `SkyfireBridge` is designed for incremental
// streaming: the caller `feed()`s arbitrary-sized chunks, and the bridge
// demuxes + exposes access units incrementally.  PAT/PMT are discovered on
// the fly; no separate probe/init/finalize step is required.

use dvb_si::resync::TsResync as BridgeTsResync;
use skyfire_ts::{
    AudioCodec as TsAudioCodec, ChannelMap, EsDemux, SubtitleKind as TsSubtitleKind,
    VideoCodec as TsVideoCodec,
};

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
    /// `"AC3"` or `"EAC3"`.
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
    /// Annex-B elementary-stream bytes.
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

/// Scaffold: subtitle cue — produced in issue #34.
#[wasm_bindgen]
pub struct WasmSubtitleCue {
    /// Cue start PTS in 90 kHz ticks.
    pub start_pts: u64,
    /// Cue end PTS in 90 kHz ticks.
    pub end_pts: u64,
    /// PID this cue came from.
    pub pid: u16,
    /// Raw subtitle payload bytes.
    #[wasm_bindgen(getter_with_clone)]
    pub bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// SkyfireBridge
// ---------------------------------------------------------------------------

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
    resync: BridgeTsResync,
    es_demux: EsDemux,

    // PSI path: reuse the probe machinery incrementally.
    si_demux: dvb_si::demux::SiDemux,
    pmt_pids: Option<Vec<u16>>,
    channel: Option<ChannelMap>,

    // User selections.
    selected_audio_pid: Option<u16>,
    selected_subtitle_pid: Option<u16>,
    playing: bool,

    // Accumulated video AUs (drained by `take_video_aus`).
    video_aus: Vec<WasmVideoAu>,

    // Audio ES bytes for the selected audio PID (routing only; #31 decodes).
    // We hold them so issue #31 can plug in decode without structural changes.
    #[allow(dead_code)]
    audio_es_pending: Vec<u8>,

    // Latest PCR / PTS seen (90 kHz ticks).
    latest_pts: Option<i64>,
}

#[wasm_bindgen]
impl SkyfireBridge {
    /// Create a new, empty bridge.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            resync: BridgeTsResync::new(),
            es_demux: EsDemux::new(),
            si_demux: dvb_si::demux::SiDemux::builder().follow_pat(true).build(),
            pmt_pids: None,
            channel: None,
            selected_audio_pid: None,
            selected_subtitle_pid: None,
            playing: false,
            video_aus: Vec::new(),
            audio_es_pending: Vec::new(),
            latest_pts: None,
        }
    }

    /// Push a raw TS chunk into the bridge.
    ///
    /// Demuxes PAT/PMT on the fly and accumulates video AUs.
    #[wasm_bindgen]
    pub fn feed(&mut self, bytes: &[u8]) {
        use dvb_si::tables::any::AnyTableSection;

        for chunk in bytes.chunks(4096) {
            for pkt in self.resync.feed(chunk) {
                // PSI path: discover PAT/PMT.
                if self.channel.is_none() {
                    for event in self.si_demux.feed(&pkt) {
                        match event.table_section() {
                            Ok(AnyTableSection::PatSection(pat)) => {
                                let pids: Vec<u16> = pat.programmes().map(|e| e.pid).collect();
                                self.pmt_pids = Some(pids);
                            }
                            Ok(AnyTableSection::PmtSection(pmt)) => {
                                if let Some(ref pids) = self.pmt_pids {
                                    let event_pid: u16 = event.pid().into();
                                    if pids.contains(&event_pid) {
                                        if let Some(ch) = build_channel_map_bridge(&pmt) {
                                            // Default selected audio to first audio stream.
                                            if self.selected_audio_pid.is_none() {
                                                self.selected_audio_pid =
                                                    ch.audio_streams.first().map(|s| s.pid);
                                            }
                                            self.channel = Some(ch);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // ES path: feed packet to PES assembler.
                self.es_demux.feed_packet(&pkt);
            }
        }

        // Drain completed access units.
        self.drain_access_units();
    }

    /// Select which audio PID to route (and later decode in #31).
    #[wasm_bindgen]
    pub fn select_audio(&mut self, pid: u16) {
        self.selected_audio_pid = Some(pid);
    }

    /// Select a subtitle PID, or `None` to disable subtitles.
    #[wasm_bindgen]
    pub fn select_subtitle(&mut self, pid: Option<u16>) {
        self.selected_subtitle_pid = pid;
    }

    /// Set the play/pause state (stored; gates nothing critical yet).
    #[wasm_bindgen]
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Returns the track list once a PMT has been parsed, or `None`.
    #[wasm_bindgen]
    pub fn track_list(&self) -> Option<WasmTrackList> {
        let ch = self.channel.as_ref()?;
        let audio: Vec<WasmAudioTrack> = ch
            .audio_streams
            .iter()
            .map(|s| WasmAudioTrack {
                pid: s.pid,
                codec: bridge_audio_codec_str(s.codec).to_string(),
                language: s.language.map(|l| lang_bytes_to_string(&l)),
            })
            .collect();
        let subtitles: Vec<WasmSubtitleTrack> = ch
            .subtitle_streams
            .iter()
            .map(|s| WasmSubtitleTrack {
                pid: s.pid,
                kind: bridge_subtitle_kind_str(s.kind).to_string(),
                language: s.language.map(|l| lang_bytes_to_string(&l)),
            })
            .collect();
        Some(WasmTrackList {
            video_pid: ch.video_pid,
            video_codec: bridge_video_codec_str(ch.video_codec).to_string(),
            audio,
            subtitles,
        })
    }

    /// WebCodecs codec string (e.g. `"avc1.640028"`) once SPS has been seen.
    ///
    /// Returns `None` until sufficient video AUs have been fed to extract an SPS.
    #[wasm_bindgen]
    pub fn video_codec(&self) -> Option<String> {
        let ch = self.channel.as_ref()?;
        // Collect the video AUs we've buffered so far (plus any in the pending queue).
        let video_pid = ch.video_pid;
        // Build a slice of AccessUnit refs from our buffered WasmVideoAu to feed
        // h264_decoder_config.  We reconstruct minimal AccessUnit structs from the bytes.
        let aus: Vec<skyfire_ts::AccessUnit> = self
            .video_aus
            .iter()
            .map(|au| skyfire_ts::AccessUnit {
                pid: video_pid,
                pts_ticks: au.pts_ticks,
                dts_ticks: au.dts_ticks,
                es_bytes: au.bytes.clone(),
            })
            .collect();
        let config = skyfire_ts::h264_config::h264_decoder_config(&aus)?;
        Some(config.codec)
    }

    /// Drain all completed video access units since the last call.
    ///
    /// Returns Annex-B bytes with PTS/DTS and a keyframe flag.
    #[wasm_bindgen]
    pub fn take_video_aus(&mut self) -> Vec<WasmVideoAu> {
        std::mem::take(&mut self.video_aus)
    }

    /// Scaffold: returns an empty `Vec` until issue #31 implements AC-3 decode.
    ///
    /// The selected audio PID is already routed internally via
    /// `audio_es_pending`; issue #31 will hook decode into `drain_access_units`.
    #[wasm_bindgen]
    pub fn take_audio_pcm(&mut self) -> Vec<WasmPcmChunk> {
        Vec::new()
    }

    /// Scaffold: returns an empty `Vec` until issue #34 implements subtitle parsing.
    #[wasm_bindgen]
    pub fn take_subtitle_cues(&mut self) -> Vec<WasmSubtitleCue> {
        Vec::new()
    }

    /// Latest PCR-derived clock value in 90 kHz ticks.
    ///
    /// The `EsDemux` / `SiDemux` layer does not separately surface PCR values;
    /// we derive this from the most recently seen video or selected-audio PTS,
    /// which is within one PCR interval (~40 ms for DVB) of the true PCR.
    /// A future issue can replace this with raw PCR extraction if sub-millisecond
    /// accuracy is required (verified 2026-06-22).
    #[wasm_bindgen]
    pub fn pcr_pts(&self) -> Option<i64> {
        self.latest_pts
    }

    // ── internal ────────────────────────────────────────────────────────────

    fn drain_access_units(&mut self) {
        let units = self.es_demux.drain();
        if units.is_empty() {
            return;
        }

        let video_pid = self.channel.as_ref().map(|ch| ch.video_pid);
        let audio_pid = self.selected_audio_pid;

        for au in units {
            // Update latest PTS clock from video or selected-audio PID.
            if Some(au.pid) == video_pid || Some(au.pid) == audio_pid {
                if let Some(pts) = au.pts_ticks {
                    self.latest_pts = Some(pts as i64);
                }
            }

            if Some(au.pid) == video_pid {
                let is_keyframe = annexb_has_idr_or_sps(&au.es_bytes);
                self.video_aus.push(WasmVideoAu {
                    pts_ticks: au.pts_ticks,
                    dts_ticks: au.dts_ticks,
                    is_keyframe,
                    bytes: au.es_bytes,
                });
            } else if Some(au.pid) == audio_pid {
                // Route audio ES bytes for #31 to decode.
                self.audio_es_pending.extend_from_slice(&au.es_bytes);
            }
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

/// Scan Annex-B bytes for NAL type 5 (IDR) or 7 (SPS).
/// A start-code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01) followed by a NAL
/// header byte whose lower 5 bits are 5 or 7 marks a keyframe.
fn annexb_has_idr_or_sps(bytes: &[u8]) -> bool {
    let len = bytes.len();
    let mut i = 0usize;
    while i + 3 < len {
        // Match 3-byte or 4-byte start code.
        let (sc3, sc4) = (
            bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1,
            i + 3 < len
                && bytes[i] == 0
                && bytes[i + 1] == 0
                && bytes[i + 2] == 0
                && bytes[i + 3] == 1,
        );
        if sc4 {
            let nal_offset = i + 4;
            if nal_offset < len {
                let nal_type = bytes[nal_offset] & 0x1f;
                if nal_type == 5 || nal_type == 7 {
                    return true;
                }
            }
            i += 4;
        } else if sc3 {
            let nal_offset = i + 3;
            if nal_offset < len {
                let nal_type = bytes[nal_offset] & 0x1f;
                if nal_type == 5 || nal_type == 7 {
                    return true;
                }
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

/// Build a `ChannelMap` from a PMT section via the public helper in skyfire-ts.
fn build_channel_map_bridge(pmt: &dvb_si::tables::pmt::PmtSection<'_>) -> Option<ChannelMap> {
    skyfire_ts::build_channel_map_from_pmt(pmt)
}

fn bridge_video_codec_str(c: TsVideoCodec) -> &'static str {
    match c {
        TsVideoCodec::H264 => "H264",
        TsVideoCodec::H265 => "H265",
    }
}

fn bridge_audio_codec_str(c: TsAudioCodec) -> &'static str {
    match c {
        TsAudioCodec::Ac3 => "AC3",
        TsAudioCodec::EAc3 => "EAC3",
    }
}

fn bridge_subtitle_kind_str(k: TsSubtitleKind) -> &'static str {
    match k {
        TsSubtitleKind::DvbSubtitles => "DvbSubtitles",
        TsSubtitleKind::Teletext => "Teletext",
    }
}

fn lang_bytes_to_string(lang: &[u8; 3]) -> String {
    // ISO 639-2 codes are ASCII — lossless conversion.
    String::from_utf8_lossy(lang).into_owned()
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

        // --- scaffolds ---
        let pcm = bridge.take_audio_pcm();
        assert!(pcm.is_empty(), "take_audio_pcm must be empty (scaffold)");
        let subs = bridge.take_subtitle_cues();
        assert!(
            subs.is_empty(),
            "take_subtitle_cues must be empty (scaffold)"
        );

        eprintln!(
            "bridge: {} video AUs, {} keyframes, pcr_pts={}",
            aus.len(),
            keyframe_count,
            pcr
        );
    }
}
