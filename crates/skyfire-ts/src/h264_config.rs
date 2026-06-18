use h264_reader::annexb::AnnexBReader;
use h264_reader::nal::{sps::SeqParameterSet, Nal, RefNal, UnitType};
use h264_reader::push::NalInterest;

/// Decoder configuration ready for WebCodecs `VideoDecoder.configure`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoConfig {
    /// RFC 6381 codec string (e.g. `avc1.640028`).
    pub codec: String,
    /// `AVCDecoderConfigurationRecord` bytes (`avcC` box).
    pub description: Vec<u8>,
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

    let description = build_avcc_description(
        sps.profile_idc.into(),
        sps.constraint_flags.into(),
        sps.level_idc,
        sps_bytes,
        pps_bytes,
    );

    Some(VideoConfig { codec, description })
}

/// Build an AVCDecoderConfigurationRecord per ISO/IEC 14496-15.
///
/// Layout:
///   u8  configuration_version = 1
///   u8  AVCProfileIndication
///   u8  profile_compatibility
///   u8  AVCLevelIndication
///   6 bits reserved + 2 bits lengthSizeMinusOne
///   3 bits reserved + 5 bits numOfSequenceParameterSets
///   for each SPS:
///     u16 sequenceParameterSetLength
///     u8[length] sequenceParameterSetNALUnit
///   u8  numOfPictureParameterSets
///   for each PPS:
///     u16 pictureParameterSetLength
///     u8[length] pictureParameterSetNALUnit
fn build_avcc_description(
    profile_idc: u8,
    constraint_flags: u8,
    level_idc: u8,
    sps_data: &[u8],
    pps: &[u8],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(7 + 2 + sps_data.len() + 1 + 2 + pps.len());

    // configuration_version = 1
    bytes.push(1);
    // AVCProfileIndication
    bytes.push(profile_idc);
    // profile_compatibility
    bytes.push(constraint_flags);
    // AVCLevelIndication
    bytes.push(level_idc);
    // 6 bits reserved (0b111111) + 2 bits lengthSizeMinusOne = 3 (4-byte length)
    bytes.push(0b1111_1100 | 3);
    // 3 bits reserved (0b111) + 5 bits numOfSequenceParameterSets = 1
    bytes.push(0b111_00000 | 1);
    // SPS length (16 bits big-endian)
    let sps_len = sps_data.len() as u16;
    bytes.push((sps_len >> 8) as u8);
    bytes.push(sps_len as u8);
    // SPS NAL unit bytes
    bytes.extend_from_slice(sps_data);
    // numOfPictureParameterSets = 1
    bytes.push(1);
    // PPS length (16 bits big-endian)
    let pps_len = pps.len() as u16;
    bytes.push((pps_len >> 8) as u8);
    bytes.push(pps_len as u8);
    // PPS NAL unit bytes
    bytes.extend_from_slice(pps);

    bytes
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
    fn golden_gulli_15s() {
        let video_units = video_access_units("gulli-15s.ts", 0x0100);
        let config = h264_decoder_config(&video_units).expect("must build config from gulli-15s");

        assert_eq!(config.codec, "avc1.640028");

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
    fn missing_param_sets_returns_none() {
        let units: Vec<crate::AccessUnit> = vec![crate::AccessUnit {
            pid: 0x0100,
            pts_ticks: Some(0),
            es_bytes: vec![0x00, 0x00, 0x00, 0x01, 0x41, 0x9a], // non-IDR slice, no SPS/PPS
        }];
        assert!(h264_decoder_config(&units).is_none());
    }

    #[test]
    fn empty_access_units_returns_none() {
        assert!(h264_decoder_config(&[]).is_none());
    }
}
