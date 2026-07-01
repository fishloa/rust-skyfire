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
    /// Audio codec identifiers, parallel to `audio_pids`: `"EAc3"`, `"Ac3"`, or `"Mp2"`.
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
                "Mp2" => skyfire_core::ts::AudioCodec::Mp2,
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
        skyfire_core::ts::AudioCodec::Mp2 => "Mp2",
    }
}

// ── SkyfireBridge — streaming WASM bridge (issue #29) ─────────────────────
//
// Unlike the batch `WasmEngine`, `SkyfireBridge` is designed for incremental
// streaming: the caller `feed()`s arbitrary-sized chunks, and the bridge
// demuxes + exposes access units incrementally.  PAT/PMT are discovered on
// the fly; no separate probe/init/finalize step is required.

use broadcast_common::traits::Parse;
use dvb_si::resync::TsResync as BridgeTsResync;
use skyfire_ac3::IncrementalDecoder;
use skyfire_mpa::IncrementalMpaDecoder;
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

    // Cached WebCodecs video config, built once from the first keyframe
    // AUs.  Persists across `take_video_aus()` drains.
    cached_video_config: Option<skyfire_ts::h264_config::VideoConfig>,

    // Incremental E-AC-3/AC-3 decoder — holds IMDCT state across AU boundaries.
    audio_decoder: IncrementalDecoder,

    // Incremental MPEG-1/2 Layer II decoder.
    mpa_decoder: IncrementalMpaDecoder,

    // Decoded PCM chunks pending drain by `take_audio_pcm`.
    audio_pcm_pending: Vec<WasmPcmChunk>,

    // Subtitle compositor (accumulates segments, composites display sets).
    subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState,

    // Composited subtitle cues pending drain by `take_subtitle_cues`.
    subtitle_cues_pending: Vec<WasmSubtitleCue>,

    // Latest PCR / PTS seen (90 kHz ticks).
    latest_pts: Option<i64>,

    // Access units drained before the PMT was parsed (channel is None).
    // Replayed once the channel map becomes known.
    pre_channel_aus: Vec<skyfire_ts::AccessUnit>,

    // CMAF media segment sequence number (starts at 1, increments per segment).
    media_seq: u32,
    // Set to true when flush() has been called (end of stream).
    ended: bool,
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
            cached_video_config: None,
            audio_decoder: IncrementalDecoder::new(),
            mpa_decoder: IncrementalMpaDecoder::new(),
            audio_pcm_pending: Vec::new(),
            subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState::new(),
            subtitle_cues_pending: Vec::new(),
            latest_pts: None,
            pre_channel_aus: Vec::new(),
            media_seq: 1,
            ended: false,
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

        // If the channel map was just discovered, replay any access units
        // that were drained before we knew the video/audio PIDs.
        self.replay_pre_channel_aus();
    }

    /// Select which audio PID to route and decode.
    ///
    /// If the PID changes, the AC-3/E-AC-3 and MPEG audio decoder states are
    /// reset so the new stream decodes cleanly (PTS continuity is handled in
    /// issue #33).
    #[wasm_bindgen]
    pub fn select_audio(&mut self, pid: u16) {
        if self.selected_audio_pid != Some(pid) {
            self.audio_decoder.reset();
            self.mpa_decoder.reset();
        }
        self.selected_audio_pid = Some(pid);
    }

    /// Select a subtitle PID, or `None` to disable subtitles.
    ///
    /// Calling this clears any buffered subtitle cues from the previously
    /// selected PID (or disables subtitle output when `pid` is `None`).
    #[wasm_bindgen]
    pub fn select_subtitle(&mut self, pid: Option<u16>) {
        if self.selected_subtitle_pid != pid {
            self.subtitle_cues_pending.clear();
        }
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

    /// CMAF initialization segment (`ftyp` + fragmented-init `moov`) for
    /// the video track, for MSE playback. Empty until SPS/PPS have been seen.
    /// Track id = 1, timescale = 90_000 (matches TS PTS).
    #[wasm_bindgen]
    pub fn video_init_segment(&self) -> Vec<u8> {
        let Some(cfg) = self.cached_video_config.as_ref() else {
            return Vec::new();
        };
        let track = transmux::TrackSpec {
            track_id: 1,
            timescale: 90_000,
            config: transmux::CodecConfig::Avc {
                config: cfg.avcc_box.clone(),
                width: cfg.width,
                height: cfg.height,
            },
        };
        transmux::build_init_segment(&[track], 90_000).unwrap_or_default()
    }

    /// WebCodecs codec string (e.g. `"avc1.640028"`) once SPS has been seen.
    ///
    /// Returns `None` until sufficient video AUs have been fed to extract an SPS.
    /// Once extracted, the config is cached and survives `take_video_aus()` drains.
    #[wasm_bindgen]
    pub fn video_codec(&self) -> Option<String> {
        self.cached_video_config.as_ref().map(|c| c.codec.clone())
    }

    /// WebCodecs `avcC` description bytes (`Uint8Array`), or empty if not yet available.
    ///
    /// This is the `AVCDecoderConfigurationRecord` per ISO/IEC 14496-15.
    /// When present, the player should configure `VideoDecoder` with `description`
    /// (AVCC mode); when absent, Annex-B mode requires SPS+PPS in-band.
    #[wasm_bindgen]
    pub fn video_config_description(&self) -> Vec<u8> {
        self.cached_video_config
            .as_ref()
            .map(|c| c.description.clone())
            .unwrap_or_default()
    }

    /// Drain all completed video access units since the last call.
    ///
    /// Returns AVCC length-prefixed bytes with PTS/DTS and a keyframe flag.
    /// Configure the `VideoDecoder` with the avcC `description` from
    /// `video_config_description()` for AVCC mode.
    #[wasm_bindgen]
    pub fn take_video_aus(&mut self) -> Vec<WasmVideoAu> {
        let mut aus = std::mem::take(&mut self.video_aus);
        for au in &mut aus {
            au.bytes = skyfire_ts::h264_config::annexb_to_avcc(&au.bytes);
        }
        aus
    }

    /// Drain the next complete GOP (keyframe → just before the next keyframe)
    /// as a CMAF media segment. Returns `None` until a full GOP is buffered.
    /// Sample durations are the DTS deltas (90 kHz); composition offset =
    /// pts − dts.
    #[wasm_bindgen]
    pub fn take_video_media_segment(&mut self) -> Option<WasmMediaSegment> {
        // Drop leading non-keyframe AUs (cannot start a segment mid-GOP).
        while self.video_aus.first().is_some_and(|a| !a.is_keyframe) {
            self.video_aus.remove(0);
        }
        if self.video_aus.is_empty() {
            return None;
        }
        // Find the end of the GOP: next keyframe (exclusive).
        let gop_end = self.video_aus.iter().skip(1).position(|a| a.is_keyframe);
        let end = match gop_end {
            Some(pos) => pos + 1, // +1 because .skip(1) shifts positions
            None => {
                // No following keyframe: emit only if the stream has ended.
                if self.ended {
                    self.video_aus.len()
                } else {
                    return None;
                }
            }
        };

        let gop: Vec<_> = self.video_aus.drain(0..end).collect();
        let dts_vec: Vec<u64> = gop
            .iter()
            .map(|a| a.dts_ticks.or(a.pts_ticks).unwrap_or(0))
            .collect();
        let base_media_decode_time = dts_vec[0];

        let mut samples = Vec::with_capacity(gop.len());
        for (i, au) in gop.iter().enumerate() {
            let duration = if i + 1 < dts_vec.len() {
                (dts_vec[i + 1].saturating_sub(dts_vec[i])) as u32
            } else {
                // Last sample: reuse previous delta, else a 25 fps default.
                if i > 0 {
                    (dts_vec[i].saturating_sub(dts_vec[i - 1])) as u32
                } else {
                    3600
                }
            };
            let pts = au.pts_ticks.unwrap_or(dts_vec[i]);
            let composition_offset = (pts as i64 - dts_vec[i] as i64) as i32;
            samples.push(transmux::Sample::from_annexb(
                &au.bytes,
                duration,
                au.is_keyframe,
                composition_offset,
            ));
        }

        let sample_count = samples.len() as u32;
        let seq = self.media_seq;
        self.media_seq += 1;
        let bytes = transmux::build_media_segment(
            seq,
            &[transmux::FragmentTrackData {
                track_id: 1,
                base_media_decode_time,
                samples: &samples,
            }],
        )
        .unwrap_or_default();

        Some(WasmMediaSegment {
            base_media_decode_time,
            bytes,
            sample_count,
        })
    }

    /// Drain all decoded PCM chunks produced since the last call.
    ///
    /// Each chunk corresponds to one audio access unit decoded from the
    /// selected audio PID.  Samples are interleaved f32 (WebAudio-ready).
    #[wasm_bindgen]
    pub fn take_audio_pcm(&mut self) -> Vec<WasmPcmChunk> {
        std::mem::take(&mut self.audio_pcm_pending)
    }

    /// Drain all composited subtitle cues since the last call.
    ///
    /// Each cue corresponds to one DVB subtitle display-set from the selected
    /// subtitle PID.  Each cue contains RGBA region bitmaps ready for the
    /// JS overlay (no further parsing needed).
    ///
    /// Returns an empty `Vec` when no subtitle PID is selected
    /// (`select_subtitle(None)`) or when the selected PID carries no subtitle
    /// PES packets in the fed data (e.g. a fixture without subtitle tracks).
    #[wasm_bindgen]
    pub fn take_subtitle_cues(&mut self) -> Vec<WasmSubtitleCue> {
        // Drain the compositor, converting to WasmSubtitleCue.
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

    /// Signal end-of-stream: flush any partial PES packets held in the
    /// PES assemblers, then run the same access-unit processing as `feed()`.
    ///
    /// After calling `flush()`, a subsequent `take_video_aus()` /
    /// `take_audio_pcm()` will return any tail access units that were
    /// held back because the final PES end had not yet been signalled by
    /// a downstream PUSI packet.  Safe to call once at stream end;
    /// idempotent — calling it more than once does nothing harmful.
    #[wasm_bindgen]
    pub fn flush(&mut self) {
        self.es_demux.flush();
        self.drain_access_units();
        self.ended = true;
    }

    // ── internal ────────────────────────────────────────────────────────────

    fn drain_access_units(&mut self) {
        let units = self.es_demux.drain();
        if units.is_empty() {
            return;
        }

        // When the channel map isn't known yet, buffer all AUs for later replay.
        if self.channel.is_none() {
            self.pre_channel_aus.extend(units);
            return;
        }

        self.route_access_units(units.into_iter());
    }

    /// Process a stream of access units through the per-PID routing logic.
    fn route_access_units(&mut self, units: impl Iterator<Item = skyfire_ts::AccessUnit>) {
        let video_pid = self.channel.as_ref().map(|ch| ch.video_pid);
        let audio_pid = self.selected_audio_pid;
        let subtitle_pid = self.selected_subtitle_pid;

        // Look up the codec for the selected audio PID.
        let audio_codec = audio_pid.and_then(|pid| {
            self.channel
                .as_ref()
                .and_then(|ch| ch.audio_streams.iter().find(|s| s.pid == pid))
                .map(|s| s.codec)
        });

        for au in units {
            // Update latest PTS clock from video or selected-audio PID.
            if Some(au.pid) == video_pid || Some(au.pid) == audio_pid {
                if let Some(pts) = au.pts_ticks {
                    self.latest_pts = Some(pts as i64);
                }
            }

            if Some(au.pid) == video_pid {
                let is_keyframe = annexb_has_idr_or_sps(&au.es_bytes);
                // Store Annex-B bytes internally; conversion to AVCC happens
                // in `take_video_aus()` once the avcC description is available.
                self.video_aus.push(WasmVideoAu {
                    pts_ticks: au.pts_ticks,
                    dts_ticks: au.dts_ticks,
                    is_keyframe,
                    bytes: au.es_bytes,
                });
                // Try to build the video config once we have enough data.
                if self.cached_video_config.is_none() {
                    let video_pid_val = video_pid.unwrap_or(0);
                    let aus: Vec<skyfire_ts::AccessUnit> = self
                        .video_aus
                        .iter()
                        .map(|a| skyfire_ts::AccessUnit {
                            pid: video_pid_val,
                            pts_ticks: a.pts_ticks,
                            dts_ticks: a.dts_ticks,
                            es_bytes: a.bytes.clone(),
                        })
                        .collect();
                    self.cached_video_config = skyfire_ts::h264_config::h264_decoder_config(&aus);
                }
            } else if Some(au.pid) == audio_pid {
                let pts_ticks = au.pts_ticks;
                // Route to the appropriate decoder based on codec.
                match audio_codec {
                    Some(TsAudioCodec::Mp2) => {
                        let _ = self.mpa_decoder.decode_au(&au.es_bytes).map(|opt| {
                            if let Some(decoded) = opt {
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
                        });
                    }
                    _ => {
                        // Default to AC-3/E-AC-3 decoder.
                        let _ = self.audio_decoder.decode_au(&au.es_bytes).map(|opt| {
                            if let Some(decoded) = opt {
                                if decoded.sample_rate > 0 && decoded.channels > 0 {
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
                            }
                        });
                    }
                }
            } else if Some(au.pid) == subtitle_pid {
                // DVB subtitle PES: parse with dvb-subtitle (ETSI EN 300 743),
                // then feed through the compositor.
                // Non-subtitle PES on the same PID (e.g. padding_stream) are
                // silently dropped when data_identifier ≠ 0x20.
                if au.es_bytes.first() == Some(&dvb_subtitle::DataIdentifier) {
                    if let Ok(field) = dvb_subtitle::PesDataField::parse(&au.es_bytes) {
                        self.subtitle_compositor
                            .feed_pes(au.pid, au.pts_ticks, &field);
                    }
                }
            }
        }
    }

    /// Replay any access units that were buffered before the channel map was
    /// known.  Called after `self.channel` transitions from `None` to `Some`.
    fn replay_pre_channel_aus(&mut self) {
        if self.channel.is_none() || self.pre_channel_aus.is_empty() {
            return;
        }
        let units: Vec<skyfire_ts::AccessUnit> = std::mem::take(&mut self.pre_channel_aus);
        self.route_access_units(units.into_iter());
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
        TsAudioCodec::Mp2 => "MP2",
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

    /// Non-subtitle PES payload (no data_identifier 0x20) must return None.
    #[test]
    fn parse_subtitle_cue_non_subtitle_pes_returns_none() {
        use skyfire_ts::parse_subtitle_pes;
        // A minimal PES payload that starts with 0x00 (not 0x20) — e.g. a
        // padding_stream PES multiplexed on the same PID as a subtitle PID.
        let non_subtitle_payload: &[u8] = &[0x00, 0xBE, 0x01, 0x02, 0x03];
        let result = parse_subtitle_pes(0x0042, Some(100_000), non_subtitle_payload);
        assert!(result.is_none(), "non-subtitle PES must return None");
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
}
