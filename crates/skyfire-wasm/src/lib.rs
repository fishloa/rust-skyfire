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

use oxideav_core::{CodecId, Decoder as _, Frame, Packet, TimeBase};
use oxideav_h264::h264_decoder::H264CodecDecoder;
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
            streams.push(skyfire_core::ts::AudioStream { pid, codec: ac });
        }
        let channel = skyfire_core::ts::ChannelMap {
            video_pid,
            video_codec: vc,
            audio_streams: streams,
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
    /// == 0`). WebCodecs cannot decode such streams; the shell must route
    /// them through the [`WasmVideoDecoder`] software path.
    #[wasm_bindgen]
    pub fn video_is_interlaced(&self) -> bool {
        self.engine
            .as_ref()
            .and_then(|e| e.video_config())
            .map(|c| c.interlaced)
            .unwrap_or(false)
    }
}

// ── software H.264 video decoder (interlaced / 1080i path) ───────────────────

/// A decoded video frame as I420 (planar YUV 4:2:0): the Y plane
/// (`width * height` bytes) followed by the U and V planes
/// (`(width/2) * (height/2)` bytes each), tightly packed.
#[wasm_bindgen]
pub struct WasmVideoFrame {
    /// Luma width in samples.
    pub width: u32,
    /// Luma height in samples.
    pub height: u32,
    /// I420 planar bytes (Y, then U, then V), tightly packed.
    #[wasm_bindgen(getter_with_clone)]
    pub data: Vec<u8>,
}

/// Software H.264 decoder exposed to JS for content WebCodecs cannot
/// handle — chiefly **1080i / PAFF interlaced** broadcast H.264. Feed
/// Annex-B access units with [`send`](WasmVideoDecoder::send), then drain
/// decoded frames with [`receive`](WasmVideoDecoder::receive) until it
/// returns `null`. The output is progressive (fields already woven).
#[wasm_bindgen]
pub struct WasmVideoDecoder {
    dec: H264CodecDecoder,
}

impl Default for WasmVideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmVideoDecoder {
    /// Create a new software H.264 decoder.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            dec: H264CodecDecoder::new(CodecId::new("h264")),
        }
    }

    /// Feed one Annex-B access unit. `pts_ticks` is the 90 kHz PTS.
    #[wasm_bindgen]
    pub fn send(&mut self, au: &[u8], pts_ticks: f64) {
        let pkt = Packet::new(0, TimeBase::new(1, 90_000), au.to_vec()).with_pts(pts_ticks as i64);
        let _ = self.dec.send_packet(&pkt);
    }

    /// Signal end-of-stream so buffered (reordered) frames can drain.
    #[wasm_bindgen]
    pub fn flush(&mut self) {
        let _ = self.dec.flush();
    }

    /// Pull the next decoded frame as I420, or `null` if none is ready.
    /// Call repeatedly until it returns `null`.
    #[wasm_bindgen]
    pub fn receive(&mut self) -> Option<WasmVideoFrame> {
        match self.dec.receive_frame() {
            Ok(Frame::Video(vf)) => Some(video_frame_to_i420(&vf)),
            _ => None,
        }
    }
}

/// Pack an `oxideav_core::VideoFrame` (whose planes may be strided) into
/// tightly-packed I420 bytes.
fn video_frame_to_i420(vf: &oxideav_core::VideoFrame) -> WasmVideoFrame {
    let y = &vf.planes[0];
    let w = y.stride;
    let h = if w == 0 { 0 } else { y.data.len() / w };
    let cw = w / 2;
    let ch = h / 2;
    let mut data = Vec::with_capacity(w * h + 2 * cw * ch);
    pack_plane(&mut data, &y.data, y.stride, w, h);
    if vf.planes.len() >= 3 {
        pack_plane(&mut data, &vf.planes[1].data, vf.planes[1].stride, cw, ch);
        pack_plane(&mut data, &vf.planes[2].data, vf.planes[2].stride, cw, ch);
    } else {
        // Monochrome fallback: neutral chroma.
        data.resize(w * h + 2 * cw * ch, 128);
    }
    WasmVideoFrame {
        width: w as u32,
        height: h as u32,
        data,
    }
}

/// Copy `rows` rows of `cols` bytes out of a strided plane into `dst`.
fn pack_plane(dst: &mut Vec<u8>, src: &[u8], stride: usize, cols: usize, rows: usize) {
    if stride == cols {
        dst.extend_from_slice(&src[..(cols * rows).min(src.len())]);
    } else {
        for r in 0..rows {
            let start = r * stride;
            let end = start + cols;
            if end <= src.len() {
                dst.extend_from_slice(&src[start..end]);
            }
        }
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

    /// Software H.264 path: feed gulli-15s.ts video AUs through the
    /// `WasmVideoDecoder` and confirm it produces non-black I420 frames
    /// of the expected dimensions. This is the 1080i/PAFF path that
    /// WebCodecs cannot handle.
    #[test]
    fn software_decode_gulli_15s() {
        let we = engine_for_fixture("gulli-15s.ts");
        eprintln!(
            "gulli-15s.ts interlaced={} units={}",
            we.video_is_interlaced(),
            we.video_unit_count()
        );

        let mut dec = WasmVideoDecoder::new();
        let n = we.video_unit_count();
        let mut frames = 0usize;
        let mut first_dims = None;
        let mut any_non_black = false;

        let drain = |dec: &mut WasmVideoDecoder,
                     frames: &mut usize,
                     first_dims: &mut Option<(u32, u32)>,
                     any_non_black: &mut bool| {
            while let Some(f) = dec.receive() {
                assert!(f.width >= 320 && f.height >= 240, "frame too small");
                assert_eq!(
                    f.data.len(),
                    (f.width as usize * f.height as usize) * 3 / 2,
                    "I420 byte length mismatch"
                );
                if first_dims.is_none() {
                    *first_dims = Some((f.width, f.height));
                }
                // Luma plane not all one value ⇒ real picture.
                let y = &f.data[..(f.width as usize * f.height as usize)];
                if y.iter().any(|&p| p != y[0]) {
                    *any_non_black = true;
                }
                *frames += 1;
            }
        };

        // Cap at the first ~40 access units — enough to clear the open-GOP
        // leading pictures and decode several real frames without making
        // the unit test decode all 672 1080i pictures in software.
        for i in 0..n.min(40) {
            let au = we.video_unit(i).expect("au");
            dec.send(&au.bytes, au.pts_ticks().map(|p| p as f64).unwrap_or(0.0));
            drain(&mut dec, &mut frames, &mut first_dims, &mut any_non_black);
        }
        dec.flush();
        drain(&mut dec, &mut frames, &mut first_dims, &mut any_non_black);

        eprintln!("software-decoded {frames} frames, first_dims={first_dims:?}");
        assert!(frames > 0, "software decoder must produce frames");
        assert!(any_non_black, "decoded frames must not be uniform/black");
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
}
