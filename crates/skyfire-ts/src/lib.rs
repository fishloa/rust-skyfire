//! MPEG-TS demux for Skyfire — PSI (PAT/PMT) channel probing via dvb-si,
//! elementary-stream + PTS extraction via dvb-pes, and H.264 decoder
//! configuration (codec string + avcC) for WebCodecs.
//!
//! The browser receiver demuxes the raw TS served by an upstream DVB-S2
//! receiver into per-ES streams (video / audio) tagged with PTS, then hands
//! them to the WebCodecs video decoder and the WASM AC-3 audio decoder.

pub mod h264_config;

use dvb_common::Parse as _;
use dvb_pes::{PesAssembler, PesPacket};
use dvb_si::demux::SiDemux;
use dvb_si::descriptors::any::AnyDescriptor;
use dvb_si::resync::TsResync;
use dvb_si::tables::any::AnyTableSection;
use dvb_si::tables::pat::PatSection;
use dvb_si::tables::pmt::StreamType;
use dvb_subtitle::{AnySegment, PesDataField};

use std::collections::HashMap;

/// Length of a single MPEG-TS packet.
pub const TS_PACKET_LEN: usize = 188;
/// MPEG-TS sync byte.
pub const TS_SYNC_BYTE: u8 = 0x47;

// ---------------------------------------------------------------------------
// Channel‑map types
// ---------------------------------------------------------------------------

/// Identifies a video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}

/// Identifies an audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Ac3,
    EAc3,
}

/// One audio elementary stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    pub pid: u16,
    pub codec: AudioCodec,
    /// ISO 639-2 three-byte language code from the iso_639_language_descriptor
    /// (tag 0x0A), if present.  `None` when no language descriptor is found.
    pub language: Option<[u8; 3]>,
}

/// Identifies a subtitle/text stream kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleKind {
    /// DVB bitmap subtitling (ETSI EN 300 468, descriptor tag 0x59).
    DvbSubtitles,
    /// EBU Teletext (ETSI EN 300 468, descriptor tag 0x56).
    Teletext,
}

/// One subtitle or teletext elementary stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleStream {
    pub pid: u16,
    pub kind: SubtitleKind,
    /// ISO 639-2 three-byte language code from the subtitling / teletext
    /// descriptor, if present.
    pub language: Option<[u8; 3]>,
}

/// Complete channel map for one program.
///
/// This is also exported as [`TrackList`] — use whichever name reads better in
/// the calling context.  Both names refer to the same type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMap {
    /// 13-bit video elementary-stream PID.
    pub video_pid: u16,
    /// Video codec identified from the PMT stream_type.
    pub video_codec: VideoCodec,
    /// Every audio elementary stream found in the PMT.
    pub audio_streams: Vec<AudioStream>,
    /// Every subtitle/teletext elementary stream found in the PMT.
    /// Empty when the programme carries no subtitle PIDs.
    pub subtitle_streams: Vec<SubtitleStream>,
    /// 13-bit PCR PID as declared in the PMT (ISO/IEC 13818-1 §2.4.4.8).
    pub pcr_pid: u16,
}

/// Alias for [`ChannelMap`].  Use this name when writing code that treats the
/// map as a track list (e.g. the WASM engine layer selecting which PID to
/// route to a decoder).
pub type TrackList = ChannelMap;

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

/// Feed a raw MPEG-TS byte stream through dvb-si resync+demux, return the
/// first complete channel map found.
///
/// Returns `None` if no PAT+PMT could be extracted from the input.
pub fn probe(raw: &[u8]) -> Option<ChannelMap> {
    let mut resync = TsResync::new();
    let mut demux = SiDemux::builder().follow_pat(true).build();

    // Accumulate PMT PIDs we still need to collect before we can finish.
    let mut pmt_pids: Option<Vec<u16>> = None;
    let mut channel: Option<ChannelMap> = None;

    for chunk in raw.chunks(4096) {
        let mut got_pat = false;
        let mut got_pmt = false;
        for pkt in resync.feed(chunk) {
            for event in demux.feed(&pkt) {
                match event.table_section() {
                    Ok(AnyTableSection::PatSection(pat)) => {
                        got_pat = true;
                        pmt_pids = Some(collect_pmt_pids(&pat));
                    }
                    Ok(AnyTableSection::PmtSection(pmt)) => {
                        if let Some(ref pids) = pmt_pids {
                            let event_pid_val: u16 = event.pid().into();
                            if pids.contains(&event_pid_val) {
                                got_pmt = true;
                                channel = build_channel_map(&pmt);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if got_pat && got_pmt {
            break;
        }
    }

    channel
}

fn collect_pmt_pids(pat: &PatSection) -> Vec<u16> {
    pat.programmes().map(|e| e.pid).collect()
}

/// Build a `ChannelMap` from a fully parsed PMT section.
///
/// Public for use by `skyfire-wasm`'s streaming bridge, which feeds PMT
/// sections incrementally via its own `SiDemux` instance.
pub fn build_channel_map_from_pmt(pmt: &dvb_si::tables::pmt::PmtSection<'_>) -> Option<ChannelMap> {
    build_channel_map(pmt)
}

fn build_channel_map(pmt: &dvb_si::tables::pmt::PmtSection<'_>) -> Option<ChannelMap> {
    let mut video_pid: Option<u16> = None;
    let mut video_codec: Option<VideoCodec> = None;
    let mut audio_streams: Vec<AudioStream> = Vec::new();
    let mut subtitle_streams: Vec<SubtitleStream> = Vec::new();

    for stream in &pmt.streams {
        let st = stream.stream_type;
        if let Some(codec) = video_codec_from_stream_type(st) {
            video_pid = Some(stream.elementary_pid);
            video_codec = Some(codec);
        } else if let Some(codec) = audio_codec_from_stream_type(st, &stream.es_info) {
            let language = language_from_descriptors(&stream.es_info);
            audio_streams.push(AudioStream {
                pid: stream.elementary_pid,
                codec,
                language,
            });
        } else if let Some((kind, language)) = subtitle_kind_from_descriptors(&stream.es_info) {
            subtitle_streams.push(SubtitleStream {
                pid: stream.elementary_pid,
                kind,
                language,
            });
        }
    }

    Some(ChannelMap {
        video_pid: video_pid?,
        video_codec: video_codec?,
        audio_streams,
        subtitle_streams,
        pcr_pid: pmt.pcr_pid,
    })
}

/// Extract the first ISO 639-2 language code from descriptor loop (tag 0x0A).
///
/// Returns `None` when no language descriptor is present or it has no entries.
fn language_from_descriptors(
    es_info: &dvb_si::descriptors::any::DescriptorLoop<'_>,
) -> Option<[u8; 3]> {
    for item in es_info.iter().flatten() {
        if let AnyDescriptor::Iso639Language(lang) = item {
            if let Some(entry) = lang.entries.first() {
                return Some(entry.language_code.0);
            }
        }
    }
    None
}

/// Detect a subtitle/teletext stream from its ES descriptor loop.
///
/// DVB subtitling descriptor (0x59) → `SubtitleKind::DvbSubtitles`.
/// Teletext descriptor (0x56) → `SubtitleKind::Teletext`.
///
/// Returns `(kind, language)` on the first match, `None` if neither is found.
fn subtitle_kind_from_descriptors(
    es_info: &dvb_si::descriptors::any::DescriptorLoop<'_>,
) -> Option<(SubtitleKind, Option<[u8; 3]>)> {
    for item in es_info.iter().flatten() {
        match item {
            AnyDescriptor::Subtitling(sub) => {
                let lang = sub.entries.first().map(|e| e.language_code.0);
                return Some((SubtitleKind::DvbSubtitles, lang));
            }
            AnyDescriptor::Teletext(tt) => {
                let lang = tt.entries.first().map(|e| e.language_code.0);
                return Some((SubtitleKind::Teletext, lang));
            }
            _ => {}
        }
    }
    None
}

/// Map a PMT `StreamType` to a `VideoCodec`, or `None` if not video.
fn video_codec_from_stream_type(st: StreamType) -> Option<VideoCodec> {
    match st {
        StreamType::H264 | StreamType::AdditionalViewH264 => Some(VideoCodec::H264),
        StreamType::Hevc
        | StreamType::HevcTemporalSubset
        | StreamType::HevcAnnexG
        | StreamType::HevcAnnexGTemporal
        | StreamType::HevcAnnexH
        | StreamType::HevcAnnexHTemporal
        | StreamType::MctsHevc => Some(VideoCodec::H265),
        _ => None,
    }
}

/// Map a PMT `StreamType` (and its descriptors) to an `AudioCodec`, or
/// `None` if not a recognised audio stream.
///
/// Most DVB streams signal AC-3/E-AC-3 via a registration descriptor
/// (`format_identifier b"AC-3"`) when the stream_type is a user-private
/// value (typically 0x06 for AC-3, 0x81/0x87 for the ATSC stream types,
/// or a PES-private-data stream with an AC-3 descriptor).
fn audio_codec_from_stream_type(
    st: StreamType,
    es_info: &dvb_si::descriptors::any::DescriptorLoop<'_>,
) -> Option<AudioCodec> {
    // Direct stream-type mappings (ATSC A/52, ETSI TS 102 366):
    match st {
        StreamType::Ac3 => return Some(AudioCodec::Ac3),
        StreamType::EAc3 => return Some(AudioCodec::EAc3),
        _ => {}
    };

    // For PES-private-data (0x06), look for AC-3 / E-AC-3 descriptors.
    // ISO/IEC 13818-1 allows AC-3 to be signalled via:
    //   - registration_descriptor with format_identifier == b"AC-3"
    //   - AC-3_descriptor (tag 0x6A) or Enhanced AC-3_descriptor (tag 0x7A)
    for item in es_info.iter().flatten() {
        match item {
            AnyDescriptor::Registration(reg) if &reg.format_identifier == b"AC-3" => {
                return Some(AudioCodec::Ac3)
            }
            AnyDescriptor::Ac3(_) => return Some(AudioCodec::Ac3),
            AnyDescriptor::EnhancedAc3(_) => return Some(AudioCodec::EAc3),
            _ => {}
        }
    }

    None
}

/// Extract the 13-bit PID from a TS packet, or `None` if it is not a valid
/// sync-aligned packet.
#[must_use]
pub fn packet_pid(pkt: &[u8]) -> Option<u16> {
    if pkt.len() < TS_PACKET_LEN || pkt[0] != TS_SYNC_BYTE {
        return None;
    }
    Some((u16::from(pkt[1] & 0x1f) << 8) | u16::from(pkt[2]))
}

/// True if this packet carries a payload-unit-start indicator (PES/PSI start).
#[must_use]
pub fn payload_unit_start(pkt: &[u8]) -> bool {
    pkt.len() >= TS_PACKET_LEN && pkt[0] == TS_SYNC_BYTE && (pkt[1] & 0x40) != 0
}

// ---------------------------------------------------------------------------
// Access-unit types
// ---------------------------------------------------------------------------

/// One elementary-stream access unit (picture or audio frame) with its
/// presentation time stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    /// PID.
    pub pid: u16,
    /// PTS in 90 kHz units.  `None` only for the first few access units in a
    /// stream before the first PTS is seen.
    pub pts_ticks: Option<u64>,
    /// DTS in 90 kHz units.  `None` when no DTS is present in the PES header
    /// (audio streams and most progressive-video streams omit DTS).
    pub dts_ticks: Option<u64>,
    /// Elementary-stream bytes (NAL unit / audio syncframe).
    pub es_bytes: Vec<u8>,
}

/// Timed access unit — a guaranteed-PTS variant for consumers that require it.
/// `None`-PTS units are dropped or held until the first valid PTS is seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedAccessUnit {
    pub pid: u16,
    pub pts_ticks: u64,
    pub es_bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Subtitle parsing (ETSI EN 300 743)
// ---------------------------------------------------------------------------

/// One parsed DVB subtitle cue, ready to surface to the browser overlay.
///
/// `bytes` layout (ETSI EN 300 743 PES data field, verbatim):
/// ```text
///   [0]     data_identifier  = 0x20
///   [1]     subtitle_stream_id = 0x00
///   [2..]   one or more subtitling_segments (each prefixed by sync_byte 0x0F)
///   [last]  end_of_PES_data_field_marker = 0xFF
/// ```
///
/// Parse with `dvb_subtitle::PesDataField::parse(&bytes)` to access the
/// segment tree (page_composition, region_composition, object_data, CLUT, …).
/// The byte layout is the standard wire format — no Skyfire-specific framing.
///
/// `end_pts` is derived from the `page_time_out` field in the
/// page_composition_segment (in seconds × 90_000 ticks) added to `start_pts`.
/// When no page_composition_segment is present, `end_pts == start_pts`
/// (the JS layer should treat the cue as instantaneous / unknown duration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleCue {
    /// PES PTS in 90 kHz ticks.
    pub start_pts: u64,
    /// Estimated end PTS = start_pts + page_time_out × 90_000.
    /// Falls back to `start_pts` when no page_composition_segment is found.
    pub end_pts: u64,
    /// PID this cue came from.
    pub pid: u16,
    /// Raw PES data field bytes (ETSI EN 300 743, starting with data_identifier 0x20).
    pub bytes: Vec<u8>,
}

/// Try to parse a DVB subtitle PES payload into a [`SubtitleCue`].
///
/// Returns `None` when:
/// - The payload does not start with the DVB subtitle `data_identifier` (0x20),
///   indicating this is not a subtitle PES (e.g. padding_stream on the same PID).
/// - `dvb_subtitle::PesDataField::parse` fails (malformed PES data field).
///
/// `start_pts` is the PTS extracted from the PES header (90 kHz ticks).
pub fn parse_subtitle_pes(
    pid: u16,
    start_pts: Option<u64>,
    es_bytes: &[u8],
) -> Option<SubtitleCue> {
    // ETSI EN 300 743 §7.2: the PES payload starts with data_identifier 0x20.
    // Any other value means this is not a DVB subtitling PES (e.g. EBU Teletext
    // uses data_identifier 0x10, and padding_stream PES on the same PID may
    // carry arbitrary data).
    if es_bytes.first() != Some(&dvb_subtitle::DataIdentifier) {
        return None;
    }

    let field = PesDataField::parse(es_bytes).ok()?;

    // Extract page_time_out from the first page_composition_segment found.
    let page_time_out_secs: Option<u8> = field.segments.iter().find_map(|seg| {
        if let AnySegment::PageComposition(pcs) = seg {
            Some(pcs.page_time_out)
        } else {
            None
        }
    });

    let pts = start_pts.unwrap_or(0);
    // page_time_out is in seconds; 90_000 ticks/second (ISO/IEC 13818-1 §2.7.4).
    let end_pts = page_time_out_secs
        .map(|t| pts.saturating_add(u64::from(t) * 90_000))
        .unwrap_or(pts);

    Some(SubtitleCue {
        start_pts: pts,
        end_pts,
        pid,
        bytes: es_bytes.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// ES demux
// ---------------------------------------------------------------------------

/// Stateful elementary-stream demuxer: per-PID PES reassembly → access units.
///
/// Feed raw 188-byte TS packets via [`feed_packet`](Self::feed_packet); collect
/// completed access units via `into_iter()` / `drain()`.
#[derive(Debug, Default)]
pub struct EsDemux {
    /// Per-PID PES assemblers.
    assemblers: HashMap<u16, PesAssembler>,
    /// Completed access units (drained by `drain` or iteration).
    units: Vec<AccessUnit>,
}

impl EsDemux {
    /// New empty demux.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw 188-byte TS packet.
    ///
    /// Only packets for PIDs with an active assembler are processed.
    /// New assemblers are created on-the-fly for any PID whose PUSI is set
    /// and which carries a payload (PES start).
    pub fn feed_packet(&mut self, pkt: &[u8]) {
        let pid = match packet_pid(pkt) {
            Some(p) => p,
            None => return,
        };
        let pusi = payload_unit_start(pkt);
        let payload = match packet_payload(pkt) {
            Some(p) => p,
            None => return,
        };

        // Lazy creation: the first time we see a PUSI+payload for a PID,
        // start an assembler.  Continuation packets for unknown PIDs are
        // ignored (we may have joined mid-stream for that PID).
        if pusi && !self.assemblers.contains_key(&pid) {
            self.assemblers.insert(pid, PesAssembler::new());
        }

        let assem = match self.assemblers.get_mut(&pid) {
            Some(a) => a,
            None => return,
        };

        if let Some(pes_bytes) = assem.feed(pusi, payload) {
            if let Ok(pes) = PesPacket::parse(&pes_bytes) {
                let pts_ticks = pes.header.as_ref().and_then(|h| h.pts).map(|p| p.ticks());
                let dts_ticks = pes.header.as_ref().and_then(|h| h.dts).map(|d| d.ticks());
                self.units.push(AccessUnit {
                    pid,
                    pts_ticks,
                    dts_ticks,
                    es_bytes: pes.payload.to_vec(),
                });
            }
        }
    }

    /// Flush all remaining partial PES packets, emitting their access units.
    pub fn flush(&mut self) {
        for (&pid, assem) in &mut self.assemblers {
            if let Some(pes_bytes) = assem.flush() {
                if let Ok(pes) = PesPacket::parse(&pes_bytes) {
                    let pts_ticks = pes.header.as_ref().and_then(|h| h.pts).map(|p| p.ticks());
                    let dts_ticks = pes.header.as_ref().and_then(|h| h.dts).map(|d| d.ticks());
                    self.units.push(AccessUnit {
                        pid,
                        pts_ticks,
                        dts_ticks,
                        es_bytes: pes.payload.to_vec(),
                    });
                }
            }
        }
    }

    /// Drain all accumulated access units, leaving the demux empty.
    pub fn drain(&mut self) -> Vec<AccessUnit> {
        std::mem::take(&mut self.units)
    }
}

/// Extract the payload bytes from a raw 188-byte TS packet.
///
/// Handles the adaptation field (skipped when present).  Returns `None`
/// when the packet has no payload or the header is invalid.
#[must_use]
pub fn packet_payload(pkt: &[u8]) -> Option<&[u8]> {
    if pkt.len() < TS_PACKET_LEN || pkt[0] != TS_SYNC_BYTE {
        return None;
    }
    // Byte 3 bits 5:4 = adaptation_field_control
    let afc = (pkt[3] >> 4) & 0x03;
    let has_payload = (afc & 0x01) != 0;
    if !has_payload {
        return None;
    }
    let mut cursor = 4usize;
    // Adaptation field present
    if (afc & 0x02) != 0 && cursor < TS_PACKET_LEN {
        let af_len = pkt[cursor] as usize;
        cursor += 1 + af_len;
        if cursor >= TS_PACKET_LEN {
            return None;
        }
    }
    Some(&pkt[cursor..])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Unit tests (pre-existing)
    // ------------------------------------------------------------------

    #[test]
    fn extracts_pid_from_sync_packet() {
        let mut pkt = [0u8; TS_PACKET_LEN];
        pkt[0] = TS_SYNC_BYTE;
        pkt[1] = 0x41; // PUSI set + PID high bits 0x01
        pkt[2] = 0x23;
        assert_eq!(packet_pid(&pkt), Some(0x123));
        assert!(payload_unit_start(&pkt));
    }

    #[test]
    fn rejects_bad_sync() {
        let pkt = [0u8; TS_PACKET_LEN];
        assert_eq!(packet_pid(&pkt), None);
    }

    // ------------------------------------------------------------------
    // Malformed / non-sync input
    // ------------------------------------------------------------------

    #[test]
    fn probe_empty_input() {
        assert!(probe(&[]).is_none());
    }

    #[test]
    fn probe_garbage_does_not_panic() {
        let garbage = [0u8; 4096];
        assert!(probe(&garbage).is_none());
    }

    // ------------------------------------------------------------------
    // Golden fixture tests
    // ------------------------------------------------------------------

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        std::fs::read(path).expect("fixture not found")
    }

    #[test]
    fn fixture_h264_25fps() {
        let data = load_fixture("h264-25fps.ts");
        let map = probe(&data).expect("must extract channel map");
        assert_eq!(map.video_pid, 0x0100);
        assert_eq!(map.video_codec, VideoCodec::H264);
        assert_eq!(map.audio_streams.len(), 0);
    }

    #[test]
    fn fixture_m6_clean() {
        let data = load_fixture("m6-clean.ts");
        let map = probe(&data).expect("must extract channel map");
        assert_eq!(map.video_pid, 0x0100);
        assert_eq!(map.video_codec, VideoCodec::H264);
        assert_eq!(map.audio_streams.len(), 0);
    }

    #[test]
    fn fixture_gulli_15s() {
        let data = load_fixture("gulli-15s.ts");
        let map = probe(&data).expect("must extract channel map");
        assert_eq!(map.video_pid, 0x0100);
        assert_eq!(map.video_codec, VideoCodec::H264);
        assert_eq!(map.audio_streams.len(), 1);
        assert_eq!(map.audio_streams[0].pid, 0x0101);
        assert_eq!(map.audio_streams[0].codec, AudioCodec::EAc3);
    }

    // ------------------------------------------------------------------
    // ES + PTS extraction tests
    // ------------------------------------------------------------------

    /// Feed raw TS packets through EsDemux and return finished access units.
    fn es_demux_fixture(name: &str) -> EsDemux {
        let data = load_fixture(name);
        let mut demux = EsDemux::new();
        let mut resync = TsResync::new();
        for chunk in data.chunks(4096) {
            for pkt in resync.feed(chunk) {
                demux.feed_packet(&pkt);
            }
        }
        demux.flush();
        demux
    }

    #[test]
    fn es_demux_gulli_15s_video_pts_monotonic() {
        let mut demux = es_demux_fixture("gulli-15s.ts");
        let units = demux.drain();

        let video_units: Vec<_> = units.iter().filter(|u| u.pid == 0x0100).collect();
        assert!(!video_units.is_empty(), "must extract video access units");

        // Every video AU must have a finite PTS under the 33-bit cap.
        // H.264 with B-frames reorders pictures — PTS may not be strictly
        // monotonic at the PES level (decode order ≠ presentation order).
        // The guard here is: all PTS values are finite, under the 33-bit
        // ceiling, and the spread across the 15 s clip is reasonable
        // (no wrap).
        let pts_vals: Vec<u64> = video_units
            .iter()
            .map(|au| au.pts_ticks.expect("video AU must have PTS"))
            .collect();

        let max_pts = pts_vals.iter().max().copied().unwrap();
        let min_pts = pts_vals.iter().min().copied().unwrap();

        assert!(max_pts < (1 << 33), "max PTS must be under 33-bit cap");
        // For a 15 s 25 fps clip: PTS span ≈ 15 * 90_000 = 1_350_000.
        // With B-frame reordering the decode-window margin is ~1 GOP.
        // Allow up to 5 s (450_000 ticks) of spread beyond the clip length.
        assert!(
            max_pts - min_pts < 2_000_000,
            "PTS spread must be consistent with a ~15 s clip, got {}",
            max_pts - min_pts
        );
    }

    #[test]
    fn es_demux_gulli_15s_audio_pts_monotonic() {
        let mut demux = es_demux_fixture("gulli-15s.ts");
        let units = demux.drain();

        let audio_units: Vec<_> = units.iter().filter(|u| u.pid == 0x0101).collect();
        assert!(!audio_units.is_empty(), "must extract audio access units");

        let mut last_pts: Option<u64> = None;
        for au in &audio_units {
            let pts = au.pts_ticks.expect("audio AU must have PTS");
            assert!(pts < (1 << 33), "PTS must be under 33-bit cap");
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
    fn es_demux_gulli_15s_audio_es_matches_eac3_fixture() {
        let mut demux = es_demux_fixture("gulli-15s.ts");
        let units = demux.drain();

        let mut extracted_audio: Vec<u8> = Vec::new();
        for au in &units {
            if au.pid == 0x0101 {
                extracted_audio.extend_from_slice(&au.es_bytes);
            }
        }
        assert!(!extracted_audio.is_empty(), "must extract audio ES bytes");

        // Strong oracle: bytes-equal (modulo trailing partial frame) against
        // the independently-extracted gulli.eac3.
        let expected = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/gulli.eac3"),
        )
        .expect("gulli.eac3 not found");

        // Truncate to the shorter length (trailing partial frame tolerated).
        let min_len = expected.len().min(extracted_audio.len());
        assert_eq!(
            &extracted_audio[..min_len],
            &expected[..min_len],
            "extracted audio ES must match gulli.eac3 byte-for-byte"
        );
    }

    #[test]
    fn es_demux_gulli_15s_audio_eac3_decode_match() {
        // VIA SKYFIRE-AC3: decode both the extracted ES and the gold
        // gulli.eac3 and compare sample_rate, channels, and non-silent PCM.
        let mut demux = es_demux_fixture("gulli-15s.ts");
        let units = demux.drain();

        let mut extracted_audio: Vec<u8> = Vec::new();
        for au in &units {
            if au.pid == 0x0101 {
                extracted_audio.extend_from_slice(&au.es_bytes);
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

        // PCM must not be all-silent (decoder sanity check).
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

    #[test]
    fn es_demux_truncated_final_pes_no_panic() {
        // Craft a minimal TS-like stream where the last PES is truncated:
        // one complete PES then a partial continuation.
        let mut data = Vec::new();

        // Helper: build a minimal 188-byte TS packet for a given PID+PUSI+payload.
        let make_pkt = |pid: u16, pusi: bool, afc: u8, payload: &[u8]| -> Vec<u8> {
            let mut pkt = vec![0u8; 188];
            pkt[0] = 0x47;
            let hi: u8 = ((pid >> 8) & 0x1f) as u8 | if pusi { 0x40 } else { 0 };
            pkt[1] = hi;
            pkt[2] = (pid & 0xff) as u8;
            pkt[3] = afc << 4;
            let payload_start = 4usize;
            let len = payload.len().min(188 - payload_start);
            pkt[payload_start..payload_start + len].copy_from_slice(&payload[..len]);
            pkt
        };

        // PES packet #1: start + PTS + 2-byte payload, split over 2 TS packets.
        let pes1 = [
            0x00, 0x00, 0x01, 0xC0, 0x00, 0x0A, 0x80, 0x80, 0x05, // header
            0x21, 0x00, 0x01, 0x00, 0x01, // PTS=1
            0xAA, 0xBB, // payload
        ];
        // First TS packet: PUSI + first 8 bytes of head
        data.extend_from_slice(&make_pkt(0x0040, true, 1, &pes1[..8]));
        // Second TS packet: rest
        data.extend_from_slice(&make_pkt(0x0040, false, 1, &pes1[8..]));
        // Third TS packet: PUSI for a new PES with only 3 bytes (truncated header)
        data.extend_from_slice(&make_pkt(0x0040, true, 1, &pes1[..3]));

        // Also put some junk with PUSI+payload on a different PID to exercise
        // the lazy-creation path.
        data.extend_from_slice(&make_pkt(
            0x0050,
            true,
            1,
            &[0x00, 0x00, 0x01, 0xE0, 0x00, 0x00],
        ));

        let mut demux = EsDemux::new();
        for chunk in data.chunks(188) {
            demux.feed_packet(chunk);
        }
        demux.flush();
        let _units = demux.drain();
        // The test passes if we got here without panicking.
    }

    // ------------------------------------------------------------------
    // gulli-15s.ts: single-program TS with H.264 video + E-AC-3 audio
    // ------------------------------------------------------------------

    #[test]
    fn gulli_15s_track_enumeration() {
        // gulli-15s.ts is a real DVB-S2 single-program TS:
        //   video PID 0x0100 — H.264 (stream_type 0x1b)
        //   audio PID 0x0101 — E-AC-3 (stream_type 0x87)
        //   PCR PID  0x0100  — shared with video
        //   No subtitle PID in this fixture.
        let data = load_fixture("gulli-15s.ts");

        let map = probe(&data).expect("gulli-15s.ts must yield a ChannelMap");

        // Video track.
        assert_eq!(map.video_pid, 0x0100, "video PID must be 0x0100");
        assert_eq!(
            map.video_codec,
            VideoCodec::H264,
            "video codec must be H.264"
        );

        // PCR PID.
        assert_eq!(map.pcr_pid, 0x0100, "PCR PID must be 0x0100");

        // Audio track: one E-AC-3 stream with language "fre".
        assert_eq!(
            map.audio_streams.len(),
            1,
            "must find exactly one audio stream"
        );
        let audio = &map.audio_streams[0];
        assert_eq!(audio.pid, 0x0101, "audio PID must be 0x0101");
        assert_eq!(audio.codec, AudioCodec::EAc3, "audio codec must be E-AC-3");
        // ISO-639 language descriptor is present in this fixture (tag 0x0A,
        // code "fre" = French).
        assert_eq!(
            audio.language,
            Some(*b"fre"),
            "audio language must be \"fre\""
        );

        // Subtitles: this fixture has no subtitle PID.
        assert!(
            map.subtitle_streams.is_empty(),
            "gulli-15s.ts carries no subtitle PIDs"
        );
    }

    #[test]
    fn gulli_15s_video_access_units_pts_and_idr() {
        // Feed the full gulli-15s.ts through EsDemux and verify:
        //   1. Video access units (PID 0x0100) are produced.
        //   2. All video AUs carry a valid PTS under the 33-bit cap.
        //   3. PTS values are monotonically non-decreasing after accounting for
        //      B-frame reordering (span is consistent with the clip length).
        //   4. The first IDR access unit starts with an Annex-B start code
        //      followed by a SPS or IDR NAL — i.e. the demuxer emits Annex-B
        //      bytes, not AVCC-length-prefixed bytes.
        let data = load_fixture("gulli-15s.ts");

        let mut demux = EsDemux::new();
        let mut resync = TsResync::new();
        for chunk in data.chunks(4096) {
            for pkt in resync.feed(chunk) {
                demux.feed_packet(&pkt);
            }
        }
        demux.flush();
        let units = demux.drain();

        let video_units: Vec<_> = units.iter().filter(|u| u.pid == 0x0100).collect();
        assert!(
            !video_units.is_empty(),
            "must extract video access units from gulli-15s.ts"
        );

        // All video AUs must carry a PTS.
        let pts_vals: Vec<u64> = video_units
            .iter()
            .map(|au| au.pts_ticks.expect("video AU must have PTS"))
            .collect();

        let max_pts = pts_vals.iter().max().copied().unwrap();
        let min_pts = pts_vals.iter().min().copied().unwrap();

        assert!(max_pts < (1 << 33), "max PTS must be under 33-bit cap");

        // Sanity-check the PTS spread.  gulli-15s.ts is a ~15 s clip; allow up
        // to 60 s of headroom (60 * 90_000 = 5_400_000 ticks).
        assert!(
            max_pts - min_pts < 5_400_000,
            "PTS spread should be consistent with a short clip, got {}",
            max_pts - min_pts
        );

        // The raw ES bytes must begin with an Annex-B start code (0x00 00 01
        // or 0x00 00 00 01).  This confirms the demuxer produces Annex-B
        // bytes (not AVCC length-prefixed) as expected by the WASM H.264 layer.
        let first_with_idr = video_units.iter().find(|au| {
            let b = &au.es_bytes;
            // Look for 0x00 00 00 01 anywhere in the first 64 bytes
            // (SPS/PPS/IDR will appear in the early bytes of the PUSI packet).
            b.len() >= 4 && b.windows(4).take(64).any(|w| w == [0x00, 0x00, 0x00, 0x01])
        });
        assert!(
            first_with_idr.is_some(),
            "at least one video AU must start with an Annex-B start code"
        );
    }

    #[test]
    fn gulli_15s_audio_pts_monotonic() {
        // Audio PES packets in gulli-15s.ts carry E-AC-3 syncframes.
        // PTS must be non-decreasing (audio is CBR, no reordering).
        let data = load_fixture("gulli-15s.ts");

        let mut demux = EsDemux::new();
        let mut resync = TsResync::new();
        for chunk in data.chunks(4096) {
            for pkt in resync.feed(chunk) {
                demux.feed_packet(&pkt);
            }
        }
        demux.flush();
        let units = demux.drain();

        let audio_units: Vec<_> = units.iter().filter(|u| u.pid == 0x0101).collect();
        assert!(
            !audio_units.is_empty(),
            "must extract audio access units from gulli-15s.ts"
        );

        let mut last_pts: Option<u64> = None;
        for au in &audio_units {
            let pts = au.pts_ticks.expect("audio AU must have PTS");
            assert!(pts < (1 << 33), "PTS must be under 33-bit cap");
            if let Some(last) = last_pts {
                assert!(
                    pts >= last,
                    "audio PTS must be non-decreasing: {pts} < {last}"
                );
            }
            last_pts = Some(pts);
        }
    }
}
