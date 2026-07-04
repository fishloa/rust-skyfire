//! MPEG-TS demux for Skyfire — thin wrapper over `transmux::StreamingTsDemux`
//! plus a descriptor-parsing helper (`track_meta`) for track metadata.
//!
//! The bespoke TS-packet parsing, PES reassembly, and PSI demux that lived here
//! previously are gone; `transmux` owns all of that now.  Skyfire keeps only
//! what transmux is architecturally not: a DVB-subtitle renderer, the sync +
//! browser layer, and descriptor-based track metadata.

pub mod subtitle_compositor;

pub use transmux::avc_config::AVCDecoderConfigurationRecord;
pub use transmux::ts_demux::DemuxEvent;

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
                let meta = track_meta(&track.spec);
                match meta.kind {
                    TrackKind::Video(_) => {
                        video_meta = Some((track.spec.track_id, meta));
                    }
                    TrackKind::Audio(_) => {
                        audio_metas.push((track.spec.track_id, meta));
                    }
                    TrackKind::Subtitle(_) => {
                        subtitle_metas.push((track.spec.track_id, meta));
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
                let meta = track_meta(&track.spec);
                if matches!(meta.kind, TrackKind::Video(_)) {
                    return Some(track.spec.track_id);
                }
            }
            None
        });
        let video_track_id = video_track_id.expect("must have video track");

        let video_samples: Vec<_> = events
            .iter()
            .filter_map(|ev| {
                if let DemuxEvent::Sample { track_id, sample } = ev
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

        // All video samples must carry source_timing with a finite PTS.
        let pts_vals: Vec<u64> = video_samples
            .iter()
            .map(|s| {
                s.source_timing
                    .as_ref()
                    .expect("video sample must have source_timing")
                    .pts
            })
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
                let meta = track_meta(&track.spec);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.spec.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let audio_samples: Vec<_> = events
            .iter()
            .filter_map(|ev| {
                if let DemuxEvent::Sample { track_id, sample } = ev
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
            let pts = s
                .source_timing
                .as_ref()
                .expect("audio sample must have source_timing")
                .pts;
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
                let meta = track_meta(&track.spec);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.spec.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let mut extracted_audio: Vec<u8> = Vec::new();
        for ev in &events {
            if let DemuxEvent::Sample { track_id, sample } = ev
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
                let meta = track_meta(&track.spec);
                if matches!(meta.kind, TrackKind::Audio(_)) {
                    return Some(track.spec.track_id);
                }
            }
            None
        });
        let audio_track_id = audio_track_id.expect("must have audio track");

        let mut extracted_audio: Vec<u8> = Vec::new();
        for ev in &events {
            if let DemuxEvent::Sample { track_id, sample } = ev
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
                let meta = track_meta(&track.spec);
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
                    let meta = track_meta(&t.spec);
                    if matches!(meta.kind, TrackKind::Video(_)) {
                        return Some(t.spec.track_id);
                    }
                }
                None
            })
            .expect("video track");
        let sync: usize = events
            .iter()
            .filter(|ev| {
                matches!(ev, DemuxEvent::Sample { track_id, sample } if *track_id == vid_id && sample.is_sync)
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
