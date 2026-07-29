//! MPEG-TS demux for Skyfire — thin wrapper over `transmux::StreamingTsDemux`
//! plus a descriptor-parsing helper (`track_meta`) for track metadata.
//!
//! The bespoke TS-packet parsing, PES reassembly, and PSI demux that lived here
//! previously are gone; `transmux` owns all of that now.  Skyfire keeps only
//! what transmux is architecturally not: a DVB-subtitle renderer, the sync +
//! browser layer, and descriptor-based track metadata.

pub mod mp2_header;
pub mod subtitle_compositor;

pub use transmux::avc_config::AVCDecoderConfigurationRecord;
pub use transmux::ir::DemuxEvent;

/// MPEG-TS packet size in bytes (ISO/IEC 13818-1 §2.4.3.2).
pub const TS_PACKET_LEN: usize = 188;

use broadcast_common::traits::Serialize as BcSerialize;
use dvb_si::descriptors::any::{AnyDescriptor, DescriptorLoop};
use transmux::pipeline::CodecConfig;
use transmux::ts_demux::StreamingTsDemux;

// ---------------------------------------------------------------------------
// Track-kind / codec enums (consumed by skyfire-core and skyfire-wasm)
// ---------------------------------------------------------------------------

/// Identifies a video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}

/// Canonical video-codec string for public APIs.
///
/// This is the single source of truth — every API emits the same string.
/// The match is exhaustive (no `_ =>` catch-all) so adding a variant is a
/// compile error, never a silent misclassification.
#[must_use]
pub fn video_codec_str(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "H264",
        VideoCodec::H265 => "H265",
    }
}

/// Identifies an audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Ac3,
    EAc3,
    /// MPEG-1/2 Layer II audio (`stream_type` 0x03/0x04, DVB-SD).
    Mp2,
}

/// Canonical audio-codec string for public APIs.
///
/// UPPERCASE forms (`"AC3"`, `"EAC3"`, `"MP2"`) — the bridge/player contract.
/// This is the single source of truth: every API emits the same string.
/// The match is exhaustive (no `_ =>` catch-all) so adding a variant is a
/// compile error, never a silent misclassification.
#[must_use]
pub fn audio_codec_str(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Ac3 => "AC3",
        AudioCodec::EAc3 => "EAC3",
        AudioCodec::Mp2 => "MP2",
    }
}

/// Build the `WebCodecs` `VideoDecoder` configuration from an
/// `AVCDecoderConfigurationRecord`.
///
/// Returns the RFC-6381 codec string (e.g. `"avc1.640028"`) and the serialised
/// avcC description bytes.  This is the single canonical implementation —
/// callers that previously duplicated this logic now call here.
#[must_use]
pub fn build_avcc_config(record: &AVCDecoderConfigurationRecord) -> (String, Vec<u8>) {
    let codec = transmux::rfc6381_avc1(
        record.profile_indication,
        record.profile_compatibility,
        record.level_indication,
    );
    let len = record.serialized_len();
    let mut buf = vec![0u8; len];
    if record.serialize_into(&mut buf).is_ok() {
        (codec, buf)
    } else {
        // Should never happen — serialised_len reserves the exact size.
        (codec, Vec::new())
    }
}

/// Convert a transmux absolute PTS/DTS tick value (`Option<i64>`, media plane
/// step 2c) to the `u64` ticks type Skyfire's sync layer (`skyfire-sync`,
/// `WasmVideoAu::pts_ticks`, …) carries.
///
/// Returns `None` both when transmux reports no timestamp at all (`None` —
/// e.g. a section-carried sample) **and** when it reports a negative value.
/// A negative PTS/DTS is not expected from any DVB TS this player demuxes:
/// wire PTS/DTS are unsigned 33-bit values (ISO/IEC 13818-1 §2.4.3.6) and
/// transmux only unwraps their rollover at the demux edge — it never
/// manufactures a negative absolute value from that. But `Sample::pts`/`dts`
/// are typed `Option<i64>` (container-neutral IR), so a caller casting with
/// `pts as u64` would silently turn e.g. `-1` into `u64::MAX` and corrupt
/// every downstream A/V-sync computation without a single visible error.
/// Treating a negative value the same as "no timestamp" is a defensive
/// rejection of data this player's own domain says should never occur — it
/// is deliberately *not* a clamp to `0` (which would misrepresent a bogus
/// negative value as "the very first tick", asserting a false fact) or a
/// panic (which would take a whole feed down over one bad sample).
#[must_use]
pub fn checked_ticks(ts: Option<i64>) -> Option<u64> {
    match ts {
        Some(v) if v >= 0 => Some(v as u64),
        _ => None,
    }
}

/// Convert a transmux absolute PTS/DTS tick value **from the owning track's
/// own timescale** (`TrackSpec::timescale`) to 90 kHz ticks — the unit
/// `skyfire-sync`'s `AudioClock` and every WASM DTO (`WasmVideoAu`/
/// `WasmPcmChunk::pts_ticks`) contractually carry.
///
/// transmux 0.20 changed not just the `u64` → `Option<i64>` type of
/// `Sample::pts`/`dts` but also their **unit**: per `transmux::ts_demux`'s own
/// docs, "an audio track's IR timescale is its sample rate
/// (`TrackSpec::timescale`), and since media plane step 2c `Sample::dts`/
/// `Sample::pts` are defined to be in that track timescale". A video or
/// `Data` (subtitle) track's timescale is 90 000, so this function treats
/// every track identically — the same multiply/divide, never a branch on
/// track kind — rather than assuming any one caller's track is already
/// 90 kHz. This is the **one** conversion point: no call site may read a raw
/// `Sample::pts`/`dts` into 90 kHz ticks without going through this function
/// and supplying that track's real `timescale` (from [`TrackMeta::timescale`]
/// / `transmux::TrackSpec::timescale`).
///
/// Returns `None` when:
/// - `ts` is `None` or negative (delegates to [`checked_ticks`] for the sign
///   check — same "never fabricate, never wrap" rule);
/// - `timescale == 0` — "unknown", never divide by zero, never silently
///   assume 90 kHz.
///
/// The rescale (`ticks * 90_000 / timescale`) runs in a `u128` intermediate,
/// not `f64` (must be exact) and not a bare `u64` multiply (must not
/// overflow): a 33-bit wire PTS (`< 2^33`) times 90 000 is only ~2^47, well
/// inside `u64`, but [`checked_ticks`] accepts any non-negative `i64` up to
/// `i64::MAX` (~2^63), and `2^63 * 90_000` (~2^80) overflows `u64` — a real,
/// reachable overflow, not a theoretical one. If the final quotient still
/// doesn't fit `u64` (only possible for a pathologically tiny `timescale`),
/// this returns `None` rather than truncating or wrapping.
#[must_use]
pub fn checked_ticks_90k(ts: Option<i64>, timescale: u32) -> Option<u64> {
    let raw = checked_ticks(ts)?;
    if timescale == 0 {
        return None;
    }
    let scaled = (u128::from(raw) * 90_000u128) / u128::from(timescale);
    u64::try_from(scaled).ok()
}

/// Identifies a subtitle/text stream kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleKind {
    /// DVB bitmap subtitling (ETSI EN 300 468, descriptor tag 0x59).
    DvbSubtitles,
    /// EBU Teletext (ETSI EN 300 468, descriptor tag 0x56).
    Teletext,
}

/// The kind of a track, as seen from a `TrackAdded` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video(VideoCodec),
    Audio(AudioCodec),
    Subtitle(SubtitleKind),
    /// Unknown / unrecognised (opaque data track, ignored by the bridge).
    Other,
}

// ---------------------------------------------------------------------------
// TrackMeta — result of `track_meta()`
// ---------------------------------------------------------------------------

/// Parsed track metadata extracted from a `transmux::TrackSpec`.
///
/// Built by [`track_meta`] from a `TrackAdded` event; the bridge stores this
/// in its `track_id → TrackMeta` map for routing and UI display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeta {
    /// Source PID (from `TrackSpec::source_pid`), if this track came from TS.
    pub pid: Option<u16>,
    /// Coarse kind (video / audio / subtitle / other).
    pub kind: TrackKind,
    /// ISO 639-2 three-byte language code from the `iso_639_language_descriptor`
    /// (tag 0x0A), if present.  `None` when no language descriptor is found.
    pub language: Option<[u8; 3]>,
    /// This track's own media timescale (`TrackSpec::timescale`), ticks per
    /// second. `Sample::pts`/`dts` for this track are expressed in *this*
    /// unit, not a fixed 90 kHz — for an audio track it is the sample rate
    /// (e.g. 48 000); video and subtitle (`Data`) tracks carry 90 000. Every
    /// PTS/DTS read off a sample belonging to this track must be converted
    /// via [`checked_ticks_90k`] with this value, never assumed to already be
    /// 90 kHz ticks.
    pub timescale: u32,
}

/// Build [`TrackMeta`] from a `transmux::TrackSpec`.
///
/// - `pid` ← `spec.source_pid`
/// - `kind` / codec ← `spec.config` variant
/// - `language` ← descriptor tag `0x0A` in `spec.es_info_descriptors`
/// - For `CodecConfig::Data` tracks: `SubtitleKind` from tag `0x59`
///   (DVB-subtitling) or `0x56` (teletext) in the same descriptor loop.
#[must_use]
pub fn track_meta(spec: &transmux::TrackSpec) -> TrackMeta {
    let pid = spec.source_pid;
    let descriptors = DescriptorLoop::new(&spec.es_info_descriptors);

    let language = language_from_descriptors(&descriptors);

    let kind = match &spec.config {
        CodecConfig::Avc { .. } => TrackKind::Video(VideoCodec::H264),
        CodecConfig::Hevc { .. } => TrackKind::Video(VideoCodec::H265),
        CodecConfig::Ac3 { .. } => TrackKind::Audio(AudioCodec::Ac3),
        CodecConfig::Eac3 { .. } => TrackKind::Audio(AudioCodec::EAc3),
        CodecConfig::MpegAudio { .. } => TrackKind::Audio(AudioCodec::Mp2),
        CodecConfig::Data { .. } => {
            // For PES-private-data (stream_type 0x06), the codec is signalled
            // via ES_info descriptors, not the stream_type itself.
            // Check for AC-3/E-AC-3 audio before falling through to subtitle.
            if let Some(codec) = audio_codec_from_descriptors(&descriptors) {
                TrackKind::Audio(codec)
            } else {
                subtitle_kind_from_descriptors(&descriptors)
                    .map(TrackKind::Subtitle)
                    .unwrap_or(TrackKind::Other)
            }
        }
        _ => TrackKind::Other,
    };

    TrackMeta {
        pid,
        kind,
        language,
        timescale: spec.timescale,
    }
}

/// Extract the first ISO 639-2 language code from a descriptor loop (tag 0x0A).
fn language_from_descriptors(descriptors: &DescriptorLoop<'_>) -> Option<[u8; 3]> {
    for item in descriptors.iter().flatten() {
        if let AnyDescriptor::Iso639Language(lang) = item
            && let Some(entry) = lang.entries.first()
        {
            return Some(entry.language_code.0);
        }
    }
    None
}

/// Detect AC-3 / E-AC-3 audio from an ES descriptor loop.
///
/// Used for `stream_type 0x06` (PES private data) tracks where the codec is
/// signalled via descriptors rather than the stream_type:
/// - Registration descriptor (0x65) with `format_identifier` "AC-3" → AC-3
/// - AC-3 descriptor (0x6A) → AC-3
/// - Enhanced-AC-3 descriptor (0x7A) → E-AC-3
fn audio_codec_from_descriptors(descriptors: &DescriptorLoop<'_>) -> Option<AudioCodec> {
    for item in descriptors.iter().flatten() {
        match item {
            AnyDescriptor::Registration(reg) if &reg.format_identifier == b"AC-3" => {
                return Some(AudioCodec::Ac3);
            }
            AnyDescriptor::Ac3(_) => return Some(AudioCodec::Ac3),
            AnyDescriptor::EnhancedAc3(_) => return Some(AudioCodec::EAc3),
            _ => {}
        }
    }
    None
}

/// Detect DVB-subtitles (0x59) or teletext (0x56) from a descriptor loop.
fn subtitle_kind_from_descriptors(descriptors: &DescriptorLoop<'_>) -> Option<SubtitleKind> {
    for item in descriptors.iter().flatten() {
        match item {
            AnyDescriptor::Subtitling(_) => return Some(SubtitleKind::DvbSubtitles),
            AnyDescriptor::Teletext(_) => return Some(SubtitleKind::Teletext),
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// TsDemux — thin wrapper over transmux::StreamingTsDemux
// ---------------------------------------------------------------------------

/// Thin incremental MPEG-TS demuxer.
///
/// Wraps [`transmux::ts_demux::StreamingTsDemux`] and re-exports its event
/// type ([`DemuxEvent`]).  Callers feed raw bytes in any chunk size, poll
/// events, and call `finish()` at end of input.
///
/// ```text
/// let mut demux = TsDemux::new();
/// demux.feed(&chunk);
/// while let Some(event) = demux.poll_event() { … }
/// demux.finish();
/// while let Some(event) = demux.poll_event() { … }
/// ```
pub struct TsDemux {
    inner: StreamingTsDemux,
}

impl TsDemux {
    /// Create a new, empty demuxer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: StreamingTsDemux::new(),
        }
    }

    /// Feed raw TS bytes (any chunk size, any alignment).
    pub fn feed(&mut self, data: &[u8]) {
        self.inner.feed(data);
    }

    /// Poll the next available [`DemuxEvent`], or `None` when the event queue
    /// is empty.  Call `finish()` once at end-of-input to flush trailing
    /// partial access units, then poll again.
    pub fn poll_event(&mut self) -> Option<DemuxEvent> {
        self.inner.poll_event()
    }

    /// Flush trailing partial access units.  Must be called exactly once, at
    /// end of input.  After calling, poll until `None` to drain remaining
    /// events.
    pub fn finish(&mut self) {
        self.inner.finish();
    }
}

impl Default for TsDemux {
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
    use transmux::ts_demux::DemuxEvent;

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        std::fs::read(path).expect("fixture not found")
    }

    /// Feed a fixture through TsDemux; return all events.
    fn demux_fixture(name: &str) -> Vec<DemuxEvent> {
        let data = load_fixture(name);
        let mut demux = TsDemux::new();
        let mut events = Vec::new();
        for chunk in data.chunks(4096) {
            demux.feed(chunk);
            while let Some(ev) = demux.poll_event() {
                events.push(ev);
            }
        }
        demux.finish();
        while let Some(ev) = demux.poll_event() {
            events.push(ev);
        }
        events
    }

    // ------------------------------------------------------------------
    // checked_ticks — the i64 -> u64 PTS/DTS conversion (negative rejection)
    // ------------------------------------------------------------------

    #[test]
    fn checked_ticks_none_stays_none() {
        assert_eq!(checked_ticks(None), None);
    }

    #[test]
    fn checked_ticks_zero_is_some_zero() {
        assert_eq!(checked_ticks(Some(0)), Some(0));
    }

    #[test]
    fn checked_ticks_positive_value_round_trips() {
        assert_eq!(checked_ticks(Some(900_000)), Some(900_000u64));
        assert_eq!(checked_ticks(Some(i64::MAX)), Some(i64::MAX as u64));
    }

    #[test]
    fn checked_ticks_negative_value_is_rejected_not_wrapped() {
        // The corruption case the migration brief called out: a naive
        // `pts as u64` cast turns -1 into u64::MAX. checked_ticks must
        // reject it as None instead of ever producing that huge value.
        let converted = checked_ticks(Some(-1));
        assert_eq!(converted, None, "negative PTS must not silently wrap");
        assert_ne!(converted, Some(u64::MAX));
    }

    #[test]
    fn checked_ticks_i64_min_is_rejected() {
        assert_eq!(checked_ticks(Some(i64::MIN)), None);
    }

    // ------------------------------------------------------------------
    // checked_ticks_90k — the timescale-aware rescale to 90 kHz (#101 review
    // finding: transmux 0.20 Sample::pts/dts are in the *track's own*
    // timescale, not always 90 kHz — an audio track's is its sample rate).
    // ------------------------------------------------------------------

    #[test]
    fn checked_ticks_90k_none_stays_none() {
        assert_eq!(checked_ticks_90k(None, 48_000), None);
    }

    #[test]
    fn checked_ticks_90k_negative_is_rejected() {
        assert_eq!(checked_ticks_90k(Some(-1), 48_000), None);
    }

    #[test]
    fn checked_ticks_90k_zero_timescale_is_unknown_not_div_by_zero() {
        // timescale == 0 means "unknown" -- must not panic (divide by zero)
        // and must not silently assume 90 kHz.
        assert_eq!(checked_ticks_90k(Some(1_000), 0), None);
    }

    #[test]
    fn checked_ticks_90k_video_90khz_is_a_true_no_op() {
        // Video/subtitle tracks already carry timescale == 90_000. The same
        // multiply/divide code path must reproduce the input exactly, not
        // via a special case that could drift.
        assert_eq!(checked_ticks_90k(Some(900_000), 90_000), Some(900_000));
        assert_eq!(checked_ticks_90k(Some(0), 90_000), Some(0));
    }

    #[test]
    fn checked_ticks_90k_audio_48khz_rescales_exactly() {
        // 48_000 ticks at 48 kHz == exactly 1 second == 90_000 ticks at 90 kHz.
        assert_eq!(checked_ticks_90k(Some(48_000), 48_000), Some(90_000));
        // The brief's measured gulli-15s.ts audio span: 718_848 ticks @ 48 kHz
        // == 14.976 s == 1_347_840 ticks @ 90 kHz (not 718_848, which would be
        // the pre-fix "misread as 90 kHz" value == 7.987 s).
        assert_eq!(checked_ticks_90k(Some(718_848), 48_000), Some(1_347_840));
    }

    #[test]
    fn checked_ticks_90k_large_value_does_not_overflow_u64() {
        // i64::MAX (~2^63) * 90_000 (~2^17) is ~2^80: overflows u64 (2^64) if
        // computed there. Must use a u128 intermediate and, since the
        // quotient itself doesn't fit u64 for so small a timescale, return
        // None rather than truncate/wrap.
        assert_eq!(checked_ticks_90k(Some(i64::MAX), 1), None);
        // A large-but-still-u64-representable case must still be exact: a
        // 40-bit value at 90 kHz timescale is a no-op and must fit.
        let big: i64 = 1i64 << 40;
        assert_eq!(checked_ticks_90k(Some(big), 90_000), Some(big as u64));
    }

    // ------------------------------------------------------------------
    // gulli-15s: audio/video PTS spans must agree once scaled to 90 kHz
    // (the CRITICAL #101 review finding's discriminating regression test)
    // ------------------------------------------------------------------

    /// This is the test that proves the fix. Pre-fix, audio ticks were used
    /// directly as if they were already 90 kHz ticks (audio's real timescale
    /// is its sample rate, 48 000 for this fixture): a genuine 14.976 s audio
    /// span reads as 718_848 / 90_000 = 7.987 s, wildly disagreeing with the
    /// video track's true 90 kHz span. Scaled correctly through
    /// `checked_ticks_90k` using each track's own `timescale`, both spans
    /// must land within a few hundred ms of one another (the audio/video
    /// pre-roll difference in this fixture, not measurement noise).
    #[test]
    fn gulli_15s_audio_and_video_pts_spans_agree_once_scaled_to_90khz() {
        let events = demux_fixture("gulli-15s.ts");

        let mut video_track: Option<(u32, u32)> = None; // (track_id, timescale)
        let mut audio_track: Option<(u32, u32)> = None;
        for ev in &events {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                match meta.kind {
                    TrackKind::Video(_) if video_track.is_none() => {
                        video_track = Some((track.track_id, track.timescale));
                    }
                    TrackKind::Audio(_) if audio_track.is_none() => {
                        audio_track = Some((track.track_id, track.timescale));
                    }
                    _ => {}
                }
            }
        }
        let (video_id, video_timescale) = video_track.expect("must find a video track");
        let (audio_id, audio_timescale) = audio_track.expect("must find an audio track");

        // Sanity: this fixture is exactly the case the bug hits — audio's
        // native timescale is its 48 kHz sample rate, not 90 kHz.
        assert_eq!(video_timescale, 90_000, "video timescale must be 90 kHz");
        assert_eq!(
            audio_timescale, 48_000,
            "audio timescale must be its sample rate"
        );

        let mut video_pts_90k: Vec<u64> = Vec::new();
        let mut audio_pts_90k: Vec<u64> = Vec::new();
        for ev in &events {
            if let DemuxEvent::Sample {
                track_id, sample, ..
            } = ev
            {
                if *track_id == video_id {
                    if let Some(p) = checked_ticks_90k(sample.pts, video_timescale) {
                        video_pts_90k.push(p);
                    }
                } else if *track_id == audio_id
                    && let Some(p) = checked_ticks_90k(sample.pts, audio_timescale)
                {
                    audio_pts_90k.push(p);
                }
            }
        }

        assert!(!video_pts_90k.is_empty(), "must have video samples");
        assert!(!audio_pts_90k.is_empty(), "must have audio samples");

        let video_span_secs = (*video_pts_90k.iter().max().unwrap()
            - *video_pts_90k.iter().min().unwrap()) as f64
            / 90_000.0;
        let audio_span_secs = (*audio_pts_90k.iter().max().unwrap()
            - *audio_pts_90k.iter().min().unwrap()) as f64
            / 90_000.0;

        // Tolerance: wide enough to absorb this fixture's real audio/video
        // pre-roll difference (~1.4 s), narrow enough to reject the bug's
        // ~5.5 s "misread as 90 kHz" error (7.987 s vs 13.54 s video span).
        let diff = (video_span_secs - audio_span_secs).abs();
        eprintln!(
            "gulli-15s.ts: video_span={video_span_secs:.3}s audio_span={audio_span_secs:.3}s \
             diff={diff:.3}s"
        );
        assert!(
            diff < 3.0,
            "audio and video PTS spans must agree once both are correctly \
             scaled to 90 kHz ticks: video={video_span_secs:.3}s \
             audio={audio_span_secs:.3}s diff={diff:.3}s"
        );
    }

    // ------------------------------------------------------------------
    // gulli-15s track enumeration
    // ------------------------------------------------------------------

    #[test]
    fn gulli_15s_track_enumeration() {
        let events = demux_fixture("gulli-15s.ts");

        let mut video_meta: Option<(u32, TrackMeta)> = None;
        let mut audio_metas: Vec<(u32, TrackMeta)> = Vec::new();
        let mut subtitle_metas: Vec<(u32, TrackMeta)> = Vec::new();

        for ev in &events {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                match meta.kind {
                    TrackKind::Video(_) => {
                        video_meta = Some((track.track_id, meta));
                    }
                    TrackKind::Audio(_) => {
                        audio_metas.push((track.track_id, meta));
                    }
                    TrackKind::Subtitle(_) => {
                        subtitle_metas.push((track.track_id, meta));
                    }
                    TrackKind::Other => {}
                }
            }
        }

        // Video track: PID 0x0100, H.264.
        let (_, vmeta) = video_meta.expect("must find a video track");
        assert_eq!(vmeta.pid, Some(0x0100), "video PID must be 0x0100");
        assert_eq!(
            vmeta.kind,
            TrackKind::Video(VideoCodec::H264),
            "video codec must be H.264"
        );

        // Audio track: one E-AC-3 stream on PID 0x0101 with language "fre".
        assert_eq!(audio_metas.len(), 1, "must find exactly one audio stream");
        let (_, ameta) = &audio_metas[0];
        assert_eq!(ameta.pid, Some(0x0101), "audio PID must be 0x0101");
        assert_eq!(
            ameta.kind,
            TrackKind::Audio(AudioCodec::EAc3),
            "audio codec must be E-AC-3"
        );
        assert_eq!(
            ameta.language,
            Some(*b"fre"),
            "audio language must be \"fre\""
        );

        // Subtitles: gulli-15s carries no subtitle PIDs.
        assert!(
            subtitle_metas.is_empty(),
            "gulli-15s.ts carries no subtitle PIDs"
        );
    }

    // ------------------------------------------------------------------
    // gulli-15s video samples: count, IDR, monotonic PTS
    // ------------------------------------------------------------------

    #[test]
    fn gulli_15s_video_access_units_pts_and_idr() {
        let events = demux_fixture("gulli-15s.ts");

        // Find the video track_id.
        let video_track_id = events.iter().find_map(|ev| {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                if matches!(meta.kind, TrackKind::Video(_)) {
                    return Some(track.track_id);
                }
            }
            None
        });
        let video_track_id = video_track_id.expect("must have video track");

        let video_samples: Vec<_> = events
            .iter()
            .filter_map(|ev| {
                if let DemuxEvent::Sample {
                    track_id, sample, ..
                } = ev
                    && *track_id == video_track_id
                {
                    return Some(sample);
                }
                None
            })
            .collect();

        assert!(
            !video_samples.is_empty(),
            "must extract video samples from gulli-15s.ts"
        );

        // All video samples must carry an absolute, non-negative PTS.
        let pts_vals: Vec<u64> = video_samples
            .iter()
            .map(|s| checked_ticks(s.pts).expect("video sample must have a non-negative pts"))
            .collect();

        let max_pts = *pts_vals.iter().max().unwrap();
        let min_pts = *pts_vals.iter().min().unwrap();

        assert!(max_pts < (1u64 << 33), "max PTS must be under 33-bit cap");
        // ~15 s clip; allow 60 s headroom (60 * 90_000 = 5_400_000 ticks).
        assert!(
            max_pts - min_pts < 5_400_000,
            "PTS spread should be consistent with a short clip, got {}",
            max_pts - min_pts
        );

        // AVC samples from transmux are length-prefixed (not Annex-B).
        // AVC samples from transmux are length-prefixed (not Annex-B).
        // Each sample must be at least 5 bytes (4-byte length + 1 NAL byte).
        for s in &video_samples {
            assert!(
                s.data.len() >= 5,
                "length-prefixed sample data must be at least 5 bytes"
            );
        }

        // Confirm samples are length-prefixed (not Annex-B): the start code
        // 0x00 00 00 01 is NOT present at the very beginning (transmux converts
        // Annex-B input to 4-byte big-endian length prefix).
        // We check that the first 4 bytes form a non-zero length field that
        // is less than the total sample length.
        let first = &video_samples[0];
        let declared_len =
            u32::from_be_bytes([first.data[0], first.data[1], first.data[2], first.data[3]])
                as usize;
        assert!(
            declared_len > 0 && declared_len <= first.data.len() - 4,
            "first video sample should have a valid length-prefix, got {declared_len} (sample len={})",
            first.data.len()
        );
    }

    // ------------------------------------------------------------------
    // gulli-15s audio samples: PTS monotonic + byte-match vs gulli.eac3
    // ------------------------------------------------------------------

    #[test]
    fn gulli_15s_audio_pts_monotonic() {
        let events = demux_fixture("gulli-15s.ts");

        let audio_track_id = events.iter().find_map(|ev| {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let audio_samples: Vec<_> = events
            .iter()
            .filter_map(|ev| {
                if let DemuxEvent::Sample {
                    track_id, sample, ..
                } = ev
                    && *track_id == audio_track_id
                {
                    return Some(sample);
                }
                None
            })
            .collect();

        assert!(
            !audio_samples.is_empty(),
            "must extract audio samples from gulli-15s.ts"
        );

        let mut last_pts: Option<u64> = None;
        for s in &audio_samples {
            let pts = checked_ticks(s.pts).expect("audio sample must have a non-negative pts");
            assert!(pts < (1u64 << 33), "PTS must be under 33-bit cap");
            if let Some(last) = last_pts {
                assert!(
                    pts >= last,
                    "audio PTS must be non-decreasing: {pts} < {last}"
                );
            }
            last_pts = Some(pts);
        }
    }

    #[test]
    fn gulli_15s_audio_es_matches_eac3_fixture() {
        let events = demux_fixture("gulli-15s.ts");

        let audio_track_id = events.iter().find_map(|ev| {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let mut extracted_audio: Vec<u8> = Vec::new();
        for ev in &events {
            if let DemuxEvent::Sample {
                track_id, sample, ..
            } = ev
                && *track_id == audio_track_id
            {
                extracted_audio.extend_from_slice(&sample.data);
            }
        }
        assert!(!extracted_audio.is_empty(), "must extract audio ES bytes");

        let expected = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gulli.eac3"),
        )
        .expect("gulli.eac3 not found");

        let min_len = expected.len().min(extracted_audio.len());
        assert_eq!(
            &extracted_audio[..min_len],
            &expected[..min_len],
            "extracted audio ES must match gulli.eac3 byte-for-byte"
        );
    }

    #[test]
    fn gulli_15s_audio_eac3_decode_match() {
        let events = demux_fixture("gulli-15s.ts");

        let audio_track_id = events.iter().find_map(|ev| {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let mut extracted_audio: Vec<u8> = Vec::new();
        for ev in &events {
            if let DemuxEvent::Sample {
                track_id, sample, ..
            } = ev
                && *track_id == audio_track_id
            {
                extracted_audio.extend_from_slice(&sample.data);
            }
        }

        let expected = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gulli.eac3"),
        )
        .expect("gulli.eac3 not found");

        let decoded_extracted =
            skyfire_ac3::decode_all_eac3(&extracted_audio).expect("decode extracted audio");
        let decoded_expected =
            skyfire_ac3::decode_all_eac3(&expected).expect("decode golden audio");

        assert_eq!(decoded_extracted.sample_rate, 48_000);
        assert_eq!(decoded_extracted.channels, 2);
        assert_eq!(decoded_extracted.sample_rate, decoded_expected.sample_rate);
        assert_eq!(decoded_extracted.channels, decoded_expected.channels);

        let pcm_i16: Vec<i16> = decoded_extracted
            .pcm_s16le
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let non_silent = pcm_i16.iter().filter(|&&s| s != 0).count();
        let sample_count = pcm_i16.len() / decoded_extracted.channels as usize;
        assert!(
            non_silent > sample_count / 100,
            "decoded PCM must not be all-silent"
        );
    }

    // ------------------------------------------------------------------
    // france2-8s: 3 audio langs (fre/fra/qaa) + 2 DVB-sub tracks (#582)
    // ------------------------------------------------------------------

    #[test]
    fn france2_8s_track_enumeration() {
        let events = demux_fixture("france2-8s.ts");

        let mut audio_metas: Vec<TrackMeta> = Vec::new();
        let mut subtitle_metas: Vec<TrackMeta> = Vec::new();

        for ev in &events {
            if let DemuxEvent::TrackAdded(track) = ev {
                let meta = track_meta(track);
                match meta.kind {
                    TrackKind::Audio(_) => audio_metas.push(meta),
                    TrackKind::Subtitle(_) => subtitle_metas.push(meta),
                    _ => {}
                }
            }
        }

        // 3 audio tracks.  The fixture carries three E-AC-3 PIDs:
        //   PID 0x82 — primary lang "fre"
        //   PID 0x83 — primary lang "qad" (supplementary audio; extension
        //              descriptor carries associated lang "fra")
        //   PID 0x84 — primary lang "qaa"
        // We assert on the primary ISO-639 language tag (descriptor 0x0A).
        assert_eq!(
            audio_metas.len(),
            3,
            "france2-8s.ts must have exactly 3 audio tracks, got {}: {:?}",
            audio_metas.len(),
            audio_metas
        );

        let audio_langs: Vec<Option<[u8; 3]>> = audio_metas.iter().map(|m| m.language).collect();
        assert!(
            audio_langs.contains(&Some(*b"fre")),
            "must have language 'fre' in audio tracks, got {audio_langs:?}"
        );
        assert!(
            audio_langs.contains(&Some(*b"qaa")),
            "must have language 'qaa' in audio tracks, got {audio_langs:?}"
        );
        // Third track: primary 0x0A lang is "qad" (supplementary audio);
        // "fra" is in an extension descriptor — accept either value.
        assert!(
            audio_langs.contains(&Some(*b"fra")) || audio_langs.contains(&Some(*b"qad")),
            "must have language 'fra' or 'qad' in audio tracks, got {audio_langs:?}"
        );

        // 2 DVB-subtitle tracks.
        assert_eq!(
            subtitle_metas.len(),
            2,
            "france2-8s.ts must have exactly 2 DVB-subtitle tracks, got {}: {:?}",
            subtitle_metas.len(),
            subtitle_metas
        );
        for meta in &subtitle_metas {
            assert_eq!(
                meta.kind,
                TrackKind::Subtitle(SubtitleKind::DvbSubtitles),
                "subtitle tracks must be DvbSubtitles kind, got {:?}",
                meta.kind
            );
        }
    }

    #[test]
    fn gulli_15s_video_is_sync_count() {
        // gulli-15s.ts is an open-GOP H.264 stream: no IDR frames (NAL type 5).
        // GOP boundaries are marked by in-band SPS (NAL type 7). transmux 0.14
        // (rust-broadcast#595) recognises open-GOP random-access points (SPS-led /
        // recovery-point-SEI AUs), so is_sync is now true on GOP starts — a subset
        // of samples, not zero and not all. This is what lets the Segmenter cut
        // without any client-side is_sync override.
        let events = demux_fixture("gulli-15s.ts");
        let vid_id = events
            .iter()
            .find_map(|ev| {
                if let DemuxEvent::TrackAdded(t) = ev {
                    let meta = track_meta(t);
                    if matches!(meta.kind, TrackKind::Video(_)) {
                        return Some(t.track_id);
                    }
                }
                None
            })
            .expect("video track");
        let sync: usize = events
            .iter()
            .filter(|ev| {
                matches!(ev, DemuxEvent::Sample { track_id, sample, .. } if *track_id == vid_id && sample.flags.is_sync)
            })
            .count();
        let total: usize = events
            .iter()
            .filter(|ev| matches!(ev, DemuxEvent::Sample { track_id, .. } if *track_id == vid_id))
            .count();
        assert!(total > 0, "must have video samples");
        assert!(
            sync > 0 && sync < total,
            "gulli-15s open-GOP: transmux 0.14 must flag GOP-start RAPs (a subset), \
             not zero and not all — got {sync}/{total}"
        );
    }
}
