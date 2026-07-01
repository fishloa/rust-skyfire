//! Skyfire engine core.
//!
//! Wires the receiver together: [`ts`] demuxes the MPEG-TS into elementary
//! streams + PTS, [`ac3`] decodes AC-3/E-AC-3 audio to PCM, and [`sync`] runs
//! the audio-master clock that the (browser-side WebCodecs) video pipeline
//! chases. The WebCodecs video decode, `AudioWorklet`, and canvas render live
//! in the `web/` shell and are driven via the `skyfire-wasm` bindings.
//!
//! # Engine
//!
//! The [`Engine`] struct is the top-level entry point. Feed it raw MPEG-TS
//! bytes; it auto-detects the program's audio/video PIDs, demuxes, decodes
//! E-AC-3 audio to PCM, collects H.264 video access units, builds the
//! WebCodecs config, and exposes the audio-master clock + video present queue.

pub use skyfire_ac3 as ac3;
pub use skyfire_sync as sync;
pub use skyfire_ts as ts;

use dvb_si::resync::TsResync;
use skyfire_sync::{AudioClock, VideoFrameQueue};
use skyfire_ts::{h264_config, AccessUnit, ChannelMap, EsDemux};

/// Engine build identifier (crate version).
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Top-level engine: consumes raw MPEG-TS bytes, produces decoded audio PCM,
/// H.264 video access units with PTS, and a WebCodecs video config.
///
/// # Usage
///
/// ```ignore
/// // Probe the channel map first, then feed everything.
/// let channel = Engine::probe(&ts_bytes).expect("no PAT/PMT found");
/// let mut engine = Engine::with_channel(channel);
/// engine.feed(&ts_bytes);
/// engine.flush();
///
/// let pcm = engine.audio_pcm();
/// let sample_rate = engine.audio_sample_rate();
/// let channels = engine.audio_channels();
///
/// let video_units = engine.video_units();
/// let video_config = engine.video_config();
///
/// // The caller (skyfire-wasm) advances the audio-clock as PCM is
/// // played out, and pushes decoded video frames into the present queue.
/// let clock = engine.clock();
/// let queue = engine.queue_mut();
/// ```
pub struct Engine {
    // ── demux ──────────────────────────────────────────────────────
    resync: TsResync,
    demux: EsDemux,

    // ── channel map ────────────────────────────────────────────────
    channel: ChannelMap,

    // ── audio ──────────────────────────────────────────────────────
    /// Accumulated raw E-AC-3 ES bytes (before batch decode).
    audio_es_buf: Vec<u8>,
    /// PCM output after final decode.
    pcm_output: Vec<u8>,
    audio_sample_rate: u32,
    audio_channels: u16,
    audio_decoded: bool,

    // ── video ──────────────────────────────────────────────────────
    video_units: Vec<AccessUnit>,

    // ── sync ───────────────────────────────────────────────────────
    clock: AudioClock,
    queue: VideoFrameQueue,
    first_audio_pts: Option<u64>,
}

impl Engine {
    /// Create a new engine with a pre-probed channel map.
    ///
    /// Use [`Engine::probe`] to obtain the channel map from raw TS bytes.
    #[must_use]
    pub fn with_channel(channel: ChannelMap) -> Self {
        Self {
            resync: TsResync::new(),
            demux: EsDemux::new(),
            channel,
            audio_es_buf: Vec::new(),
            pcm_output: Vec::new(),
            audio_sample_rate: 0,
            audio_channels: 0,
            audio_decoded: false,
            video_units: Vec::new(),
            clock: AudioClock::default(),
            queue: VideoFrameQueue::new(32, 20_000, 100_000),
            first_audio_pts: None,
        }
    }

    /// Probe raw MPEG-TS bytes for the channel map (PAT+PMT).
    ///
    /// Convenience wrapper around [`skyfire_ts::probe`].
    #[must_use]
    pub fn probe(data: &[u8]) -> Option<ChannelMap> {
        skyfire_ts::probe(data)
    }

    /// Feed raw MPEG-TS bytes into the engine.
    ///
    /// Call repeatedly with incoming TS data. Audio ES bytes are accumulated;
    /// video access units are collected. Call [`flush`](Self::flush) then
    /// [`finalize`](Self::finalize) to decode audio and build the video config.
    pub fn feed(&mut self, data: &[u8]) {
        for chunk in data.chunks(4096) {
            for pkt in self.resync.feed(chunk) {
                self.demux.feed_packet(&pkt[..]);
            }
        }
        self.drain_units();
    }

    /// Flush any partial PES packets still in the demux.
    pub fn flush(&mut self) {
        self.demux.flush();
        self.drain_units();
    }

    /// Finalize: batch-decode accumulated audio ES to PCM, build clock.
    ///
    /// Call this after all `feed`/`flush` calls. After finalization,
    /// `audio_pcm()`, `audio_sample_rate()`, `audio_channels()`, and
    /// `clock` are populated.
    pub fn finalize(&mut self) {
        self.decode_audio();
    }

    /// Decoded audio PCM (interleaved S16LE bytes).
    ///
    /// Length = `samples × channels × 2` bytes. Empty if no audio decoded.
    #[must_use]
    pub fn audio_pcm(&self) -> &[u8] {
        &self.pcm_output
    }

    /// Audio sample rate in Hz (e.g. 48_000), or 0 if no audio decoded yet.
    #[must_use]
    pub fn audio_sample_rate(&self) -> u32 {
        self.audio_sample_rate
    }

    /// Number of audio channels, or 0 if no audio decoded yet.
    #[must_use]
    pub fn audio_channels(&self) -> u16 {
        self.audio_channels
    }

    /// Collected H.264 video access units with PTS.
    ///
    /// Each access unit represents one picture (frame or field) with its
    /// presentation timestamp in 90 kHz ticks.
    #[must_use]
    pub fn video_units(&self) -> &[AccessUnit] {
        &self.video_units
    }

    /// Build the WebCodecs `VideoDecoder` config (codec string + avcC) from
    /// the accumulated video access units.
    ///
    /// Returns `None` if no SPS/PPS have been extracted yet.
    #[must_use]
    pub fn video_config(&self) -> Option<h264_config::VideoConfig> {
        h264_config::h264_decoder_config(&self.video_units)
    }

    /// The audio-master media clock.
    ///
    /// The clock is anchored to the first audio PTS seen. Callers advance
    /// the clock as PCM samples are pushed to the DAC.
    #[must_use]
    pub fn clock(&self) -> &AudioClock {
        &self.clock
    }

    /// Mutable reference to the audio-master media clock.
    #[must_use]
    pub fn clock_mut(&mut self) -> &mut AudioClock {
        &mut self.clock
    }

    /// The PTS-ordered video-frame present queue.
    #[must_use]
    pub fn queue(&self) -> &VideoFrameQueue {
        &self.queue
    }

    /// Mutable reference to the video present queue.
    #[must_use]
    pub fn queue_mut(&mut self) -> &mut VideoFrameQueue {
        &mut self.queue
    }

    /// Whether the engine has produced audio PCM.
    #[must_use]
    pub fn has_audio(&self) -> bool {
        !self.pcm_output.is_empty()
    }

    /// Whether the engine has collected video access units.
    #[must_use]
    pub fn has_video(&self) -> bool {
        !self.video_units.is_empty()
    }

    /// The current channel map.
    #[must_use]
    pub fn channel(&self) -> &ChannelMap {
        &self.channel
    }

    // ── internal helpers ───────────────────────────────────────────

    fn drain_units(&mut self) {
        let units = self.demux.drain();
        if units.is_empty() {
            return;
        }

        let audio_pid = self.channel.audio_streams.first().map(|s| s.pid);
        let video_pid = self.channel.video_pid;

        for au in units {
            if Some(au.pid) == audio_pid {
                // Capture the first audio PTS for clock anchoring.
                if self.first_audio_pts.is_none() {
                    if let Some(pts) = au.pts_ticks {
                        self.first_audio_pts = Some(pts);
                    }
                }
                self.audio_es_buf.extend_from_slice(&au.es_bytes);
            } else if au.pid == video_pid {
                self.video_units.push(au);
            }
        }
    }

    fn decode_audio(&mut self) {
        if self.audio_decoded || self.audio_es_buf.is_empty() {
            return;
        }

        match skyfire_ac3::decode_all_eac3(&self.audio_es_buf) {
            Ok(decoded) => {
                if decoded.sample_rate == 0 || decoded.channels == 0 {
                    return;
                }
                self.audio_sample_rate = decoded.sample_rate;
                self.audio_channels = decoded.channels;
                self.pcm_output = decoded.pcm_s16le;

                // Set up the audio clock.
                if let Some(pts) = self.first_audio_pts {
                    self.clock = AudioClock::new(pts, decoded.sample_rate);
                    // Advance the clock by all decoded samples.
                    let sample_frames = self.pcm_output.len() / (decoded.channels as usize * 2);
                    let _ = self.clock.advance(sample_frames as u64);
                }
            }
            Err(_) => {
                // Decode failed — leave PCM empty.
            }
        }

        self.audio_decoded = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        std::fs::read(path).expect("fixture not found")
    }

    fn engine_for_fixture(name: &str) -> Engine {
        let data = load_fixture(name);
        let channel = Engine::probe(&data).expect("must probe fixture");
        let mut engine = Engine::with_channel(channel);
        engine.feed(&data);
        engine.flush();
        engine.finalize();
        engine
    }

    #[test]
    fn reexports_present() {
        assert_eq!(super::ts::TS_PACKET_LEN, 188);
        assert_eq!(super::ac3::AC3_SYNCWORD, 0x0B77);
    }

    // ── Engine tests ────────────────────────────────────────────────

    #[test]
    fn engine_truncated_input_no_panic() {
        let data = load_fixture("gulli-15s.ts");
        let channel = Engine::probe(&data).expect("must probe gulli-15s");
        let mut engine = Engine::with_channel(channel);
        engine.feed(&data[..1024]);
        engine.flush();
        engine.finalize();
        // Must not panic.
    }

    #[test]
    fn engine_gulli_15s_audio_pcm_oracle() {
        let engine = engine_for_fixture("gulli-15s.ts");

        assert!(engine.has_audio(), "engine must produce audio PCM");
        assert_eq!(engine.audio_sample_rate(), 48_000);
        assert_eq!(engine.audio_channels(), 2);

        let pcm = engine.audio_pcm();
        let bytes_per_sample: usize = 2;
        let channels = engine.audio_channels() as usize;
        assert!(pcm.len() >= 2);
        assert_eq!(
            pcm.len() % (bytes_per_sample * channels),
            0,
            "PCM buffer length must be a multiple of channels × bytes_per_sample"
        );

        let sample_count = pcm.len() / (bytes_per_sample * channels);

        // ~15 s of 48 kHz stereo → ~700,000 samples per channel.
        assert!(
            sample_count >= 140_000,
            "expected >= 140_000 samples per channel for ~15 s, got {sample_count}"
        );

        // PCM must not be all-silent.
        let pcm_i16: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        let non_silent = pcm_i16.iter().filter(|&&s| s != 0).count();
        assert!(
            non_silent > sample_count / 100,
            "decoded PCM must not be all-silent: {non_silent} / {sample_count}"
        );

        // Verify consistency: byte-level match against independently-demuxed
        // audio ES bytes decoded the same way.
        let data = load_fixture("gulli-15s.ts");
        let channel = Engine::probe(&data).unwrap();
        let audio_pid = channel.audio_streams.first().map(|s| s.pid).unwrap();

        let mut demux2 = EsDemux::new();
        let mut resync2 = TsResync::new();
        for chunk in data.chunks(4096) {
            for pkt in resync2.feed(chunk) {
                demux2.feed_packet(&pkt[..]);
            }
        }
        demux2.flush();
        let expected_audio_es: Vec<u8> = demux2
            .drain()
            .into_iter()
            .filter(|au| au.pid == audio_pid)
            .flat_map(|au| au.es_bytes)
            .collect();

        let decoded_expected =
            skyfire_ac3::decode_all_eac3(&expected_audio_es).expect("decode extracted audio");

        assert_eq!(
            engine.audio_pcm(),
            decoded_expected.pcm_s16le.as_slice(),
            "engine PCM must match independently decoded audio ES"
        );
    }

    #[test]
    fn engine_gulli_15s_video_access_units() {
        let engine = engine_for_fixture("gulli-15s.ts");

        assert!(engine.has_video(), "engine must produce video access units");

        let video_units = engine.video_units();
        assert!(!video_units.is_empty());

        // Every video AU must have a finite PTS under the 33-bit cap.
        let pts_vals: Vec<u64> = video_units
            .iter()
            .map(|au| au.pts_ticks.expect("video AU must have PTS"))
            .collect();

        let max_pts = pts_vals.iter().max().copied().unwrap();
        let min_pts = pts_vals.iter().min().copied().unwrap();

        assert!(max_pts < (1 << 33), "max PTS must be under 33-bit cap");
        assert!(
            max_pts - min_pts < 2_000_000,
            "PTS spread must be consistent with a ~15 s clip, got {}",
            max_pts - min_pts
        );
    }

    #[test]
    fn engine_gulli_15s_video_config_golden() {
        let engine = engine_for_fixture("gulli-15s.ts");

        let config = engine.video_config().expect("must build H.264 config");

        assert_eq!(config.codec, "avc1.640028");

        // Golden avcC bytes from skyfire-ts h264_config golden test (High profile
        // includes ISO 14496-15 §5.3.3.1.2 ext fields).
        let expected_avcc: &[u8] = &[
            0x01, // version
            0x64, // profile_idc = 100 (High)
            0x00, // constraint_flags
            0x28, // level_idc = 40 (4.0)
            0xff, // reserved(6)+lengthSizeMinusOne(3) = 0xfc|0x03 = 0xff
            0xe1, // reserved(3)+numSPS(1) = 0xe0|0x01 = 0xe1
            0x00, 0x1c, // SPS length = 28
            // SPS NAL unit:
            0x67, 0x64, 0x00, 0x28, 0xac, 0x34, 0xa5, 0x01, 0xe0, 0x11, 0x1f, 0x78, 0x0a, 0x10,
            0x10, 0x10, 0x14, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xca, 0x50,
            0x01, // numPPS = 1
            0x00, 0x05, // PPS length = 5
            // PPS NAL unit:
            0x68, 0xea, 0x57, 0x52, 0x50,
            // High-profile ext (chroma=YUV420, 8-bit, no sps_ext):
            0xfd, 0xf8, 0xf8, 0x00,
        ];
        assert_eq!(
            config.description, expected_avcc,
            "avcC golden bytes mismatch"
        );
    }

    #[test]
    fn engine_audio_clock_anchored_on_first_pts() {
        let engine = engine_for_fixture("gulli-15s.ts");

        let clock = engine.clock();
        assert!(
            clock.anchor_pts_raw > 0,
            "clock must be anchored on first audio PTS"
        );
        assert_eq!(clock.sample_rate, 48_000);
        assert!(
            clock.samples_played > 0,
            "clock must have advanced with decoded samples"
        );
    }

    #[test]
    fn engine_video_queue_accessible() {
        let engine = engine_for_fixture("gulli-15s.ts");

        let queue = engine.queue();
        assert!(queue.is_empty(), "queue starts empty");
        assert_eq!(queue.len(), 0);
    }
}
