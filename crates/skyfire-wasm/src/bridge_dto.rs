use wasm_bindgen::prelude::*;

// ── SkyfireBridge DTO structs (issue #29) ───────────────────────────────────

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
    /// Channel count, read from the first frame header seen on this PID, or
    /// `None` when no frame has been observed yet. Never a guess — the UI
    /// must degrade rather than invent a value.
    pub channels: Option<u8>,
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
    pub(crate) pts_ticks: Option<u64>,
    /// Decode timestamp in 90 kHz ticks, or `None`.
    pub(crate) dts_ticks: Option<u64>,
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
    pub(crate) pts_ticks: Option<u64>,
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
    pub(crate) start_pts: u64,
    /// Estimated end PTS in 90 kHz ticks (start_pts + page_time_out x 90_000).
    pub(crate) end_pts: u64,
    pub(crate) regions: Vec<WasmSubtitleRegion>,
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

// ── Internal DTOs (not exposed to JS directly) ──────────────────────────────

/// Cached WebCodecs video configuration derived from the first video TrackAdded event.
pub(crate) struct CachedVideoConfig {
    /// Codec string, e.g. `"avc1.640028"`.
    pub codec: String,
    /// Serialized `AVCDecoderConfigurationRecord` bytes.
    pub description: Vec<u8>,
}
