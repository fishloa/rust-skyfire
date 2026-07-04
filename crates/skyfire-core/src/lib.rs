//! Skyfire engine core.
//!
//! Wires the receiver together: [`ts`] demuxes the MPEG-TS into elementary
//! streams + PTS, [`ac3`] decodes AC-3/E-AC-3 audio to PCM, and [`sync`] runs
//! the audio-master clock that the (browser-side `WebCodecs`) video pipeline
//! chases. The `WebCodecs` video decode, `AudioWorklet`, and canvas render live
//! in the `web/` shell and are driven via the `skyfire-wasm` bindings.
//!
//! # Engine
//!
//! The [`Engine`] struct is the top-level entry point. Feed it raw MPEG-TS
//! bytes; it auto-detects the program's audio/video PIDs, demuxes, decodes
//! E-AC-3 audio to PCM, collects H.264 video access units, builds the
//! `WebCodecs` config, and exposes the audio-master clock + video present queue.

pub use skyfire_ac3 as ac3;
pub use skyfire_sync as sync;
pub use skyfire_ts as ts;

use broadcast_common::traits::Serialize as BcSerialize;
use skyfire_sync::{AudioClock, VideoFrameQueue};
use skyfire_ts::{DemuxEvent, TrackKind, TrackMeta, TsDemux, track_meta};
use transmux::pipeline::CodecConfig;

/// Engine build identifier (crate version).
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One H.264 video access unit with PTS and keyframe flag.
///
/// `data` contains length-prefixed NAL data (as produced by transmux).
#[derive(Debug, Clone)]
pub struct VideoUnit {
    /// Presentation timestamp in 90 kHz ticks.
    pub pts: u64,
    /// Whether this access unit is a sync/keyframe (IDR).
    pub is_sync: bool,
    /// Length-prefixed NAL data bytes.
    pub data: Vec<u8>,
}

/// `WebCodecs` `VideoDecoder` configuration: RFC-6381 codec string and raw
/// avcC record bytes (the `AVCDecoderConfigurationRecord`, without the box
/// header).
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// RFC-6381 codec string (e.g. `"avc1.640028"`).
    pub codec: String,
    /// Raw avcC decoder configuration record bytes.
    pub description: Vec<u8>,
}

/// Parsed metadata for a single track from a `TrackAdded` event.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// transmux track ID.
    pub track_id: u32,
    /// Source PID from the TS PMT.
    pub pid: Option<u16>,
    /// Track kind (video / audio / subtitle / other).
    pub kind: TrackKind,
    /// ISO 639-2 language, if present.
    pub language: Option<[u8; 3]>,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Top-level engine: consumes raw MPEG-TS bytes, produces decoded audio PCM,
/// H.264 video access units with PTS, and a `WebCodecs` video config.
///
/// # Usage
///
/// ```ignore
/// let mut engine = Engine::new();
/// engine.feed(&ts_bytes);
/// engine.finish();
/// engine.finalize();
///
/// let pcm = engine.audio_pcm();
/// let sample_rate = engine.audio_sample_rate();
/// let channels = engine.audio_channels();
///
/// let video_units = engine.video_units();
/// let video_config = engine.video_config();
///
/// let clock = engine.clock();
/// let queue = engine.queue_mut();
/// ```
pub struct Engine {
    // ── demux ──────────────────────────────────────────────────────
    demux: TsDemux,

    // ── track list ─────────────────────────────────────────────────
    tracks: Vec<TrackInfo>,
    /// transmux track_id for the first video track.
    video_track_id: Option<u32>,
    /// transmux track_id for the first audio track.
    audio_track_id: Option<u32>,
    /// `CodecConfig` for the video track (held to build `video_config()`).
    video_codec_config: Option<CodecConfig>,

    // ── audio ──────────────────────────────────────────────────────
    /// Accumulated raw E-AC-3 ES bytes (before batch decode).
    audio_es_buf: Vec<u8>,
    /// PCM output after final decode.
    pcm_output: Vec<u8>,
    audio_sample_rate: u32,
    audio_channels: u16,
    audio_decoded: bool,

    // ── video ──────────────────────────────────────────────────────
    video_units: Vec<VideoUnit>,

    // ── sync ───────────────────────────────────────────────────────
    clock: AudioClock,
    queue: VideoFrameQueue,
    first_audio_pts: Option<u64>,
}

impl Engine {
    /// Create a new engine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            demux: TsDemux::new(),
            tracks: Vec::new(),
            video_track_id: None,
            audio_track_id: None,
            video_codec_config: None,
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

    /// Feed raw MPEG-TS bytes into the engine.
    ///
    /// Call repeatedly with incoming TS data. Drains all available
    /// [`DemuxEvent`]s immediately: audio ES bytes are accumulated;
    /// video access units are collected. Call [`finish`](Self::finish) then
    /// [`finalize`](Self::finalize) to flush trailing AUs and decode audio.
    pub fn feed(&mut self, data: &[u8]) {
        self.demux.feed(data);
        self.drain_events();
    }

    /// Flush any trailing partial access units still in the demux.
    ///
    /// Call once at end-of-input, then [`finalize`](Self::finalize).
    pub fn finish(&mut self) {
        self.demux.finish();
        self.drain_events();
    }

    /// Finalize: batch-decode accumulated audio ES to PCM, build clock.
    ///
    /// Call after all `feed`/`finish` calls. After finalization,
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

    /// Audio sample rate in Hz (e.g. `48_000`), or 0 if no audio decoded yet.
    #[must_use]
    pub const fn audio_sample_rate(&self) -> u32 {
        self.audio_sample_rate
    }

    /// Number of audio channels, or 0 if no audio decoded yet.
    #[must_use]
    pub const fn audio_channels(&self) -> u16 {
        self.audio_channels
    }

    /// Collected H.264 video access units with PTS.
    ///
    /// Each access unit represents one picture with its presentation timestamp
    /// in 90 kHz ticks.
    #[must_use]
    pub fn video_units(&self) -> &[VideoUnit] {
        &self.video_units
    }

    /// Build the `WebCodecs` `VideoDecoder` config (codec string + avcC) from
    /// the codec configuration recovered by the demuxer.
    ///
    /// Returns `None` if no video track has been seen yet.
    #[must_use]
    pub fn video_config(&self) -> Option<VideoConfig> {
        let config = self.video_codec_config.as_ref()?;
        if let CodecConfig::Avc {
            config: avcc_box, ..
        } = config
        {
            let record = &avcc_box.config;
            let codec = transmux::rfc6381_avc1(
                record.profile_indication,
                record.profile_compatibility,
                record.level_indication,
            );
            let len = record.serialized_len();
            let mut buf = vec![0u8; len];
            record.serialize_into(&mut buf).ok()?;
            Some(VideoConfig {
                codec,
                description: buf,
            })
        } else {
            None
        }
    }

    /// Track list built from `TrackAdded` events.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// The audio-master media clock.
    ///
    /// The clock is anchored to the first audio PTS seen. Callers advance
    /// the clock as PCM samples are pushed to the DAC.
    #[must_use]
    pub const fn clock(&self) -> &AudioClock {
        &self.clock
    }

    /// Mutable reference to the audio-master media clock.
    #[must_use]
    pub const fn clock_mut(&mut self) -> &mut AudioClock {
        &mut self.clock
    }

    /// The PTS-ordered video-frame present queue.
    #[must_use]
    pub const fn queue(&self) -> &VideoFrameQueue {
        &self.queue
    }

    /// Mutable reference to the video present queue.
    #[must_use]
    pub const fn queue_mut(&mut self) -> &mut VideoFrameQueue {
        &mut self.queue
    }

    /// Whether the engine has produced audio PCM.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        !self.pcm_output.is_empty()
    }

    /// Whether the engine has collected video access units.
    #[must_use]
    pub const fn has_video(&self) -> bool {
        !self.video_units.is_empty()
    }

    // ── internal helpers ───────────────────────────────────────────

    fn drain_events(&mut self) {
        while let Some(event) = self.demux.poll_event() {
            match event {
                DemuxEvent::TrackAdded(track) => {
                    let meta: TrackMeta = track_meta(&track.spec);
                    let info = TrackInfo {
                        track_id: track.spec.track_id,
                        pid: meta.pid,
                        kind: meta.kind,
                        language: meta.language,
                    };
                    // Record the first video and first audio track_ids.
                    match meta.kind {
                        TrackKind::Video(_) if self.video_track_id.is_none() => {
                            self.video_track_id = Some(track.spec.track_id);
                            self.video_codec_config = Some(track.spec.config.clone());
                        }
                        TrackKind::Audio(_) if self.audio_track_id.is_none() => {
                            self.audio_track_id = Some(track.spec.track_id);
                        }
                        _ => {}
                    }
                    self.tracks.push(info);
                }
                DemuxEvent::TrackUpdated(track) => {
                    // Update video codec config if it has changed (e.g. SPS update).
                    if Some(track.spec.track_id) == self.video_track_id {
                        self.video_codec_config = Some(track.spec.config.clone());
                    }
                    // Update entry in track list.
                    if let Some(entry) = self
                        .tracks
                        .iter_mut()
                        .find(|t| t.track_id == track.spec.track_id)
                    {
                        let meta = track_meta(&track.spec);
                        entry.pid = meta.pid;
                        entry.kind = meta.kind;
                        entry.language = meta.language;
                    }
                }
                DemuxEvent::Sample { track_id, sample } => {
                    if Some(track_id) == self.video_track_id {
                        let pts = sample.source_timing.map(|t| t.pts).unwrap_or(0);
                        self.video_units.push(VideoUnit {
                            pts,
                            is_sync: sample.is_sync,
                            data: sample.data,
                        });
                    } else if Some(track_id) == self.audio_track_id {
                        // Capture the first audio PTS for clock anchoring.
                        if self.first_audio_pts.is_none()
                            && let Some(t) = sample.source_timing
                        {
                            self.first_audio_pts = Some(t.pts);
                        }
                        self.audio_es_buf.extend_from_slice(&sample.data);
                    }
                }
                DemuxEvent::Pcr(_) | DemuxEvent::Discontinuity { .. } => {
                    // Not used by the core engine; consumed by skyfire-wasm.
                }
                _ => {}
            }
        }
    }

    fn decode_audio(&mut self) {
        if self.audio_decoded || self.audio_es_buf.is_empty() {
            return;
        }

        // Use IncrementalDecoder (handles both AC-3 bsid≤10 and E-AC-3
        // bsid 11-16) instead of the E-AC-3-only decode_all_eac3.
        let mut dec = skyfire_ac3::IncrementalDecoder::new();
        if let Ok(Some(decoded)) = dec.decode_au(&self.audio_es_buf) {
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
        // On decode failure leave PCM empty.

        self.audio_decoded = true;
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
        let mut engine = Engine::new();
        engine.feed(&data);
        engine.finish();
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
        let mut engine = Engine::new();
        engine.feed(&data[..1024]);
        engine.finish();
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
        let mut demux2 = TsDemux::new();
        let mut audio_track_id2: Option<u32> = None;
        let mut expected_audio_es: Vec<u8> = Vec::new();

        for chunk in data.chunks(4096) {
            demux2.feed(chunk);
            while let Some(ev) = demux2.poll_event() {
                match ev {
                    DemuxEvent::TrackAdded(track) => {
                        let meta = track_meta(&track.spec);
                        if matches!(meta.kind, TrackKind::Audio(_)) && audio_track_id2.is_none() {
                            audio_track_id2 = Some(track.spec.track_id);
                        }
                    }
                    DemuxEvent::Sample { track_id, sample } => {
                        if Some(track_id) == audio_track_id2 {
                            expected_audio_es.extend_from_slice(&sample.data);
                        }
                    }
                    _ => {}
                }
            }
        }
        demux2.finish();
        while let Some(ev) = demux2.poll_event() {
            if let DemuxEvent::Sample { track_id, sample } = ev
                && Some(track_id) == audio_track_id2
            {
                expected_audio_es.extend_from_slice(&sample.data);
            }
        }

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
        let pts_vals: Vec<u64> = video_units.iter().map(|au| au.pts).collect();

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

        // Golden avcC bytes — AVCDecoderConfigurationRecord (record only, no
        // box header) as recovered by transmux from gulli-15s.ts (High profile,
        // includes ISO 14496-15 §5.3.3.1.2 ext fields).
        let expected_avcc: &[u8] = &[
            0x01, // configurationVersion
            0x64, // profile_idc = 100 (High)
            0x00, // profile_compatibility
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
    fn engine_h264_25fps_video_config_golden() {
        // Locks transmux 4:4:4 avcC recovery for h264-25fps.ts (issue #563).
        // High 4:4:4 Predictive profile: profile_idc=244 (0xF4), level_idc=12 (0x0C).
        let engine = engine_for_fixture("h264-25fps.ts");
        let config = engine.video_config().expect("must build H.264 config");

        // RFC-6381 codec string: avc1.<profile_idc><profile_compatibility><level_idc> hex.
        assert_eq!(config.codec, "avc1.F4000C");

        // Golden avcC bytes — AVCDecoderConfigurationRecord (record only, no box header)
        // as recovered by transmux from h264-25fps.ts (High 4:4:4 Predictive profile).
        let expected_avcc: &[u8] = &[
            0x01, // configurationVersion
            0xf4, // profile_idc = 244 (High 4:4:4 Predictive)
            0x00, // profile_compatibility
            0x0c, // level_idc = 12
            0xff, // reserved(6)+lengthSizeMinusOne = 0xfc|0x03 = 0xff
            0xe1, // reserved(3)+numSPS(1) = 0xe0|0x01 = 0xe1
            0x00, 0x19, // SPS length = 25
            // SPS NAL unit:
            0x67, 0xf4, 0x00, 0x0c, 0x91, 0x9b, 0x28, 0x20, 0x27, 0x60, 0x22, 0x00, 0x00, 0x03,
            0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x64, 0x1e, 0x28, 0x53, 0x2c,
            0x01, // numPPS = 1
            0x00, 0x06, // PPS length = 6
            // PPS NAL unit:
            0x68, 0xeb, 0xe3, 0xc4, 0x48, 0x44, // High-profile ext fields:
            0xff, 0xf8, 0xf8, 0x00,
        ];
        assert_eq!(
            config.description, expected_avcc,
            "avcC golden bytes mismatch for h264-25fps.ts (High 4:4:4 Predictive)"
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

    // ── AC-3 (base) oracle ──────────────────────────────────────────

    /// Extract the audio ES bytes for the first audio track in a TS
    /// fixture.  Reuses the same demuxing logic as `engine_for_fixture`.
    fn extract_audio_es_ts(name: &str) -> Vec<u8> {
        let data = load_fixture(name);
        let mut demux = TsDemux::new();
        let mut audio_track_id: Option<u32> = None;
        let mut es: Vec<u8> = Vec::new();
        for chunk in data.chunks(4096) {
            demux.feed(chunk);
            while let Some(ev) = demux.poll_event() {
                match ev {
                    DemuxEvent::TrackAdded(track) => {
                        let meta = track_meta(&track.spec);
                        if matches!(meta.kind, TrackKind::Audio(_)) && audio_track_id.is_none() {
                            audio_track_id = Some(track.spec.track_id);
                        }
                    }
                    DemuxEvent::Sample { track_id, sample } => {
                        if Some(track_id) == audio_track_id {
                            es.extend_from_slice(&sample.data);
                        }
                    }
                    _ => {}
                }
            }
        }
        demux.finish();
        while let Some(ev) = demux.poll_event() {
            if let DemuxEvent::Sample { track_id, sample } = ev
                && Some(track_id) == audio_track_id
            {
                es.extend_from_slice(&sample.data);
            }
        }
        es
    }

    #[test]
    fn engine_orf2_ac3_base_pcm_oracle() {
        // orf2-ac3-51.ts contains base AC-3 (bsid=6).  The old E-AC-3-only
        // decode_all_eac3 *cannot* produce the correct PCM; the
        // IncrementalDecoder (which dispatches by bsid) *can*.  This test
        // demands byte-level agreement, making it an ungameable oracle.
        let engine = engine_for_fixture("orf2-ac3-51.ts");

        assert!(engine.has_audio(), "engine must produce audio PCM");
        assert_eq!(engine.audio_channels(), 6, "must be 5.1 (6 channels)");

        let pcm = engine.audio_pcm();
        let bytes_per_sample: usize = 2;
        let channels = engine.audio_channels() as usize;
        assert!(pcm.len() >= 2);
        assert_eq!(
            pcm.len() % (bytes_per_sample * channels),
            0,
            "PCM buffer length must be a multiple of channels * bytes_per_sample"
        );

        let sample_count = pcm.len() / (bytes_per_sample * channels);
        // ~7 s of 48 kHz stereo → ~340,000 samples per channel.
        assert!(
            sample_count >= 50_000,
            "expected >= 50_000 samples per channel for base AC-3 fixture, got {sample_count}"
        );

        // PCM must not be all-silent (E-AC-3-only decode would produce silence).
        let pcm_i16: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let non_silent = pcm_i16.iter().filter(|&&s| s != 0).count();
        assert!(
            non_silent > sample_count / 100,
            "decoded PCM must not be all-silent: {non_silent} / {sample_count}"
        );

        // Byte-level oracle: independently extract the audio ES from the TS
        // and decode with IncrementalDecoder.
        let es = extract_audio_es_ts("orf2-ac3-51.ts");
        let mut oracle_dec = skyfire_ac3::IncrementalDecoder::new();
        let oracle = oracle_dec
            .decode_au(&es)
            .expect("oracle IncrementalDecoder must succeed")
            .expect("oracle must produce PCM");
        assert!(oracle.pcm_s16le.len() >= 2, "oracle PCM must be non-empty");

        assert_eq!(
            engine.audio_pcm(),
            oracle.pcm_s16le.as_slice(),
            "engine PCM for base AC-3 must byte-match IncrementalDecoder output"
        );
    }

    #[test]
    fn engine_ac3_51_base_pcm_oracle() {
        // ac3-51.ts is a synthetic base-AC-3 fixture — additional coverage.
        let engine = engine_for_fixture("ac3-51.ts");

        assert!(engine.has_audio(), "engine must produce audio PCM");

        let pcm = engine.audio_pcm();
        let bytes_per_sample: usize = 2;
        let channels = engine.audio_channels() as usize;
        assert!(pcm.len() >= 2);
        assert_eq!(
            pcm.len() % (bytes_per_sample * channels),
            0,
            "PCM buffer length must be a multiple of channels * bytes_per_sample"
        );

        let sample_count = pcm.len() / (bytes_per_sample * channels);
        assert!(
            sample_count >= 5_000,
            "expected >= 5_000 samples per channel for ac3-51.ts, got {sample_count}"
        );

        let pcm_i16: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let non_silent = pcm_i16.iter().filter(|&&s| s != 0).count();
        assert!(
            non_silent > sample_count / 100,
            "decoded PCM must not be all-silent: {non_silent} / {sample_count}"
        );

        // Byte-level oracle.
        let es = extract_audio_es_ts("ac3-51.ts");
        let mut oracle_dec = skyfire_ac3::IncrementalDecoder::new();
        let oracle = oracle_dec
            .decode_au(&es)
            .expect("oracle IncrementalDecoder must succeed")
            .expect("oracle must produce PCM");
        assert_eq!(
            engine.audio_pcm(),
            oracle.pcm_s16le.as_slice(),
            "engine PCM for ac3-51.ts must byte-match IncrementalDecoder output"
        );
    }
}
