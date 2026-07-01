use broadcast_common::traits::Serialize;
use h264_reader::annexb::AnnexBReader;
use h264_reader::nal::{
    sps::{ChromaFormat, SeqParameterSet},
    Nal, RefNal, UnitType,
};
use h264_reader::push::NalInterest;
use transmux::nalu_types::{AvcPps, AvcSps};
use transmux::{AVCConfigurationBox, AVCDecoderConfigurationRecord};

/// Decoder configuration ready for WebCodecs `VideoDecoder.configure`.
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// RFC 6381 codec string (e.g. `avc1.640028`).
    pub codec: String,
    /// `AVCDecoderConfigurationRecord` bytes (`avcC` box) — bare record,
    /// no box header, for WebCodecs `description`.
    pub description: Vec<u8>,
    /// True when the SPS has `frame_mbs_only_flag == 0` — i.e. the stream
    /// is interlaced / field-coded (PAFF or MBAFF). WebCodecs cannot
    /// decode this; the shell must route it through the software decoder.
    pub interlaced: bool,
    /// Coded luma width in pixels (from SPS, after frame_cropping).
    pub width: u16,
    /// Coded luma height in pixels (from SPS, after frame_cropping).
    pub height: u16,
    /// Transmux AVCConfigurationBox, for building an MSE init segment
    /// (Task 4+).
    pub avcc_box: transmux::AVCConfigurationBox,
}

/// Build a WebCodecs `VideoDecoder` config by extracting SPS/PPS from
/// H.264 video access units.
pub fn h264_decoder_config(access_units: &[crate::AccessUnit]) -> Option<VideoConfig> {
    let video_units: Vec<&[u8]> = access_units.iter().map(|au| &au.es_bytes[..]).collect();

    let mut reader = AnnexBReader::accumulate(H264ParamSetCollector {
        sps: None,
        pps: None,
    });

    for es in &video_units {
        reader.push(es);
    }

    let collector = reader.into_nal_handler();
    build_video_config(&collector.sps?, &collector.pps?)
}

/// State holder that captures the first SPS and PPS NALs.
struct H264ParamSetCollector {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl h264_reader::push::AccumulatedNalHandler for H264ParamSetCollector {
    fn nal(&mut self, nal: RefNal<'_>) -> NalInterest {
        let Ok(header) = nal.header() else {
            return NalInterest::Ignore;
        };
        match header.nal_unit_type() {
            UnitType::SeqParameterSet if self.sps.is_none() => {
                let mut buf = Vec::new();
                if std::io::Read::read_to_end(&mut nal.reader(), &mut buf).is_ok() {
                    self.sps = Some(buf);
                }
                NalInterest::Buffer
            }
            UnitType::PicParameterSet if self.pps.is_none() => {
                let mut buf = Vec::new();
                if std::io::Read::read_to_end(&mut nal.reader(), &mut buf).is_ok() {
                    self.pps = Some(buf);
                }
                NalInterest::Buffer
            }
            _ => NalInterest::Ignore,
        }
    }
}

/// Given the raw SPS and PPS NAL unit bytes (including the NAL header byte),
/// produce a `VideoConfig`.
fn build_video_config(sps_bytes: &[u8], pps_bytes: &[u8]) -> Option<VideoConfig> {
    let sps_nal = RefNal::new(sps_bytes, &[], true);
    let sps = SeqParameterSet::from_bits(sps_nal.rbsp_bits()).ok()?;

    let codec = sps.rfc6381().to_string();

    // §7.4.2.1.1 — frame_mbs_only_flag == 0 ⇒ the stream may carry
    // field/MBAFF pictures (interlaced). h264_reader models this as
    // `FrameMbsFlags::Fields { .. }`.
    let interlaced = matches!(
        sps.frame_mbs_flags,
        h264_reader::nal::sps::FrameMbsFlags::Fields { .. }
    );

    let (width, height) = sps_dimensions(&sps);
    let record = avc_record(&sps, sps_bytes, pps_bytes);

    // WebCodecs `description` = the raw record bytes (no box header).
    let mut description = vec![0u8; record.serialized_len()];
    let n = record.serialize_into(&mut description).ok()?;
    description.truncate(n);

    let avcc_box = AVCConfigurationBox::new(record);

    Some(VideoConfig {
        codec,
        description,
        interlaced,
        width,
        height,
        avcc_box,
    })
}

/// Coded luma dimensions from an h264_reader SPS (§7.4.2.1.1, accounting for
/// frame_cropping and frame_mbs_only). Returns (width, height) in pixels.
fn sps_dimensions(sps: &SeqParameterSet) -> (u16, u16) {
    let (w, h) = sps.pixel_dimensions().unwrap_or((0, 0));
    (w as u16, h as u16)
}

/// High-family profile IDs that trigger ISO 14496-15 §5.3.3.1.2 ext fields.
/// Matches the list in `transmux::avc_config::HIGH_PROFILE_IDS`.
const HIGH_PROFILE_IDS: [u8; 4] = [100, 110, 122, 144];

/// Build the transmux `AVCDecoderConfigurationRecord` from raw SPS/PPS NAL
/// bytes and the decoded SPS (for high-profile extension fields).
pub(crate) fn avc_record(
    sps: &SeqParameterSet,
    sps_bytes: &[u8],
    pps_bytes: &[u8],
) -> AVCDecoderConfigurationRecord {
    let profile: u8 = sps.profile_idc.into();
    let high = HIGH_PROFILE_IDS.contains(&profile);
    let (chroma_format, luma8, chroma8) = if high {
        (
            Some(chroma_format_idc(&sps.chroma_info.chroma_format)),
            Some(sps.chroma_info.bit_depth_luma_minus8),
            Some(sps.chroma_info.bit_depth_chroma_minus8),
        )
    } else {
        (None, None, None)
    };
    AVCDecoderConfigurationRecord {
        configuration_version: 1,
        profile_indication: profile,
        profile_compatibility: sps.constraint_flags.into(),
        level_indication: sps.level_idc,
        length_size_minus_one: 3,
        sps: vec![AvcSps(sps_bytes.to_vec())],
        pps: vec![AvcPps(pps_bytes.to_vec())],
        chroma_format,
        bit_depth_luma_minus8: luma8,
        bit_depth_chroma_minus8: chroma8,
        sps_ext: vec![],
    }
}

fn chroma_format_idc(fmt: &ChromaFormat) -> u8 {
    match fmt {
        ChromaFormat::Monochrome => 0,
        ChromaFormat::YUV420 => 1,
        ChromaFormat::YUV422 => 2,
        ChromaFormat::YUV444 => 3,
        ChromaFormat::Invalid(v) => *v as u8,
    }
}

/// Convert an Annex-B NAL stream to AVCC length-prefixed format.
///
/// Scans for 3-byte (`00 00 01`) and 4-byte (`00 00 00 01`) start codes
/// and replaces each with a 4-byte big-endian length prefix equal to the
/// size of the following NAL unit (excluding the start code).
///
/// The output is suitable for `EncodedVideoChunk` when the `VideoDecoder`
/// is configured with an avcC `description`.
/// Convert an Annex-B NAL stream to AVCC length-prefixed format
/// (4-byte big-endian length before each NAL), suitable for
/// `EncodedVideoChunk` when the `VideoDecoder` has an avcC `description`.
///
/// Thin wrapper over `transmux::annexb_to_length_prefixed` (ISO/IEC 14496-15
/// length-prefixed mdat form). Retained so existing call sites keep compiling.
pub fn annexb_to_avcc(annexb: &[u8]) -> Vec<u8> {
    transmux::annexb_to_length_prefixed(annexb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EsDemux;
    use dvb_si::resync::TsResync;

    fn load_fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name);
        std::fs::read(path).expect("fixture not found")
    }

    fn video_access_units(fixture: &str, video_pid: u16) -> Vec<crate::AccessUnit> {
        let data = load_fixture(fixture);
        let mut demux = EsDemux::new();
        let mut resync = TsResync::new();
        for chunk in data.chunks(4096) {
            for pkt in resync.feed(chunk) {
                demux.feed_packet(&pkt);
            }
        }
        demux.flush();
        let units = demux.drain();
        units.into_iter().filter(|au| au.pid == video_pid).collect()
    }

    #[test]
    fn annexb_to_avcc_matches_transmux() {
        // Two NALs: 4-byte then 3-byte start code.
        let annexb: &[u8] = &[0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0xCC];
        let got = annexb_to_avcc(annexb);
        let want = transmux::annexb_to_length_prefixed(annexb);
        assert_eq!(got, want);
        // Explicit expected: [len=2][67 AA][len=3][68 BB CC]
        assert_eq!(
            got,
            vec![0, 0, 0, 2, 0x67, 0xAA, 0, 0, 0, 3, 0x68, 0xBB, 0xCC]
        );
    }

    #[test]
    fn golden_gulli_15s() {
        let video_units = video_access_units("gulli-15s.ts", 0x0100);
        let config = h264_decoder_config(&video_units).expect("must build config from gulli-15s");

        assert_eq!(config.codec, "avc1.640028");
        assert_eq!(config.width, 1920, "gulli-15s width");
        assert_eq!(config.height, 1080, "gulli-15s height");

        // Verify avcC structure:
        assert_eq!(config.description[0], 1, "configuration_version");
        assert_eq!(config.description[1], 100, "profile_idc (High)");
        assert_eq!(config.description[2], 0, "constraint_flags");
        assert_eq!(config.description[3], 40, "level_idc (L4.0)");
        // lengthSizeMinusOne = 3 in low 2 bits of byte 4
        assert_eq!(config.description[4] & 0x03, 3);
        // numSPS = 1 in low 5 bits of byte 5
        assert_eq!(config.description[5] & 0x1f, 1);
        // SPS length
        let sps_len = u16::from_be_bytes([config.description[6], config.description[7]]) as usize;
        assert!(sps_len > 0, "SPS must be non-empty");
        // SPS data starts with 0x67 (SPS NAL header)
        assert_eq!(config.description[8], 0x67, "SPS NAL header");
        // PPS count = 1
        let pps_offset = 6 + 2 + sps_len;
        assert_eq!(config.description[pps_offset], 1, "numPPS");
        // PPS length
        let pps_len = u16::from_be_bytes([
            config.description[pps_offset + 1],
            config.description[pps_offset + 2],
        ]) as usize;
        assert!(pps_len > 0, "PPS must be non-empty");
        // PPS starts with 0x68
        assert_eq!(config.description[pps_offset + 3], 0x68, "PPS NAL header");

        // High profile ⇒ ISO 14496-15 §5.3.3.1.2 ext fields present after PPS.
        assert!(
            config.description.len() > pps_offset + 3 + pps_len,
            "high-profile ext bytes must follow the PPS"
        );

        // Full golden avcC bytes:
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
    fn golden_h264_25fps() {
        let video_units = video_access_units("h264-25fps.ts", 0x0100);
        let config = h264_decoder_config(&video_units).expect("must build config from h264-25fps");

        assert_eq!(config.codec, "avc1.F4000C");

        let expected_avcc: &[u8] = &[
            0x01, // version
            0xf4, // profile_idc = 244 (High 4:4:4)
            0x00, // constraint_flags
            0x0c, // level_idc = 12 (1.2)
            0xff, // reserved + lengthSizeMinusOne=3
            0xe1, // reserved + numSPS=1
            0x00, 0x19, // SPS length = 25
            // SPS NAL unit:
            0x67, 0xf4, 0x00, 0x0c, 0x91, 0x9b, 0x28, 0x20, 0x27, 0x60, 0x22, 0x00, 0x00, 0x03,
            0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x64, 0x1e, 0x28, 0x53, 0x2c,
            0x01, // numPPS = 1
            0x00, 0x06, // PPS length = 6
            // PPS NAL unit:
            0x68, 0xeb, 0xe3, 0xc4, 0x48, 0x44,
        ];
        assert_eq!(
            config.description, expected_avcc,
            "avcC golden bytes mismatch"
        );
    }

    #[test]
    fn avcc_record_roundtrips() {
        let video_units = video_access_units("gulli-15s.ts", 0x0100);
        let cfg = h264_decoder_config(&video_units).unwrap();
        // Parse back via AVCConfigurationBox::parse_body (bare record bytes).
        let reparsed = transmux::AVCConfigurationBox::parse_body(&cfg.description)
            .expect("record must reparse");
        assert_eq!(reparsed.config.profile_indication, 100);
        assert_eq!(reparsed.config.level_indication, 40);
    }

    #[test]
    fn missing_param_sets_returns_none() {
        let units: Vec<crate::AccessUnit> = vec![crate::AccessUnit {
            pid: 0x0100,
            pts_ticks: Some(0),
            dts_ticks: None,
            es_bytes: vec![0x00, 0x00, 0x00, 0x01, 0x41, 0x9a], // non-IDR slice, no SPS/PPS
        }];
        assert!(h264_decoder_config(&units).is_none());
    }

    #[test]
    fn empty_access_units_returns_none() {
        assert!(h264_decoder_config(&[]).is_none());
    }
}
