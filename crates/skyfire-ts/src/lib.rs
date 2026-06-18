//! MPEG-TS demux for Skyfire — PSI (PAT/PMT) channel probing via dvb-si,
//! elementary streams + PTS extraction to follow.
//!
//! The browser receiver demuxes the raw TS served by an upstream DVB-S2
//! receiver into per-ES streams (video / audio) tagged with PTS, then hands
//! them to the WebCodecs video decoder and the WASM AC-3 audio decoder.

use dvb_si::demux::SiDemux;
use dvb_si::descriptors::any::AnyDescriptor;
use dvb_si::resync::TsResync;
use dvb_si::tables::any::AnyTableSection;
use dvb_si::tables::pat::PatSection;
use dvb_si::tables::pmt::StreamType;

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
}

/// Complete channel map for one program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMap {
    pub video_pid: u16,
    pub video_codec: VideoCodec,
    pub audio_streams: Vec<AudioStream>,
}

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
fn build_channel_map(pmt: &dvb_si::tables::pmt::PmtSection<'_>) -> Option<ChannelMap> {
    let mut video_pid: Option<u16> = None;
    let mut video_codec: Option<VideoCodec> = None;
    let mut audio_streams: Vec<AudioStream> = Vec::new();

    for stream in &pmt.streams {
        let st = stream.stream_type;
        if let Some(codec) = video_codec_from_stream_type(st) {
            video_pid = Some(stream.elementary_pid);
            video_codec = Some(codec);
        } else if let Some(codec) = audio_codec_from_stream_type(st, &stream.es_info) {
            audio_streams.push(AudioStream {
                pid: stream.elementary_pid,
                codec,
            });
        }
    }

    Some(ChannelMap {
        video_pid: video_pid?,
        video_codec: video_codec?,
        audio_streams,
    })
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
}
