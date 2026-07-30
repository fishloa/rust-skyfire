//! Header-only channel-count probe for MPEG-1/2 Layer II frames.

/// Channel count from an MPEG audio frame header.
///
/// `mode` occupies bits 24–25 — the top two bits of byte 3. `0b11` is
/// single_channel (mono); every other value carries two channels
/// (stereo, joint_stereo, dual_channel). ISO/IEC 11172-3 §2.4.1.3.
///
/// The 11-bit sync pattern alone is not a reliable frame marker: `0xFF 0xEx`
/// byte pairs turn up inside real payload data too. So beyond sync this also
/// checks `version` (bits 20–19, reject reserved `01`), `layer` (bits 18–17,
/// require `10` = Layer II — this probe is Layer-II-only, per its name), and
/// `bitrate_index` (bits 15–12, reject reserved `1111`). Each of those is a
/// second, independent chance to reject a false sync inside payload bytes.
///
/// Returns `None` unless the buffer starts with a frame sync and passes all
/// of the above — never a guessed default.
#[must_use]
pub fn channels_from_header(buf: &[u8]) -> Option<u8> {
    if buf.len() < 4 {
        return None;
    }
    // Frame sync: 11 bits all set.
    if buf[0] != 0xFF || (buf[1] & 0xE0) != 0xE0 {
        return None;
    }
    // version: bits 20-19 (buf[1] bits 4-3). '01' is reserved.
    let version = (buf[1] >> 3) & 0x3;
    if version == 0b01 {
        return None;
    }
    // layer: bits 18-17 (buf[1] bits 2-1). Only Layer II ('10') is handled.
    let layer = (buf[1] >> 1) & 0x3;
    if layer != 0b10 {
        return None;
    }
    // bitrate index: top 4 bits of buf[2]. '1111' is reserved ("bad").
    let bitrate_index = buf[2] >> 4;
    if bitrate_index == 0b1111 {
        return None;
    }
    let mode = (buf[3] >> 6) & 0x3;
    Some(if mode == 0b11 { 1 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::channels_from_header;

    fn header(mode: u8) -> [u8; 4] {
        // sync(11)=all ones, version(2)=11 MPEG1, layer(2)=10 LayerII,
        // protection(1)=1, bitrate(4), sampling(2), padding(1), private(1),
        // mode(2) in the top bits of byte 3.
        [0xFF, 0xFD, 0x50, mode << 6]
    }

    #[test]
    fn single_channel_mode_is_mono() {
        assert_eq!(channels_from_header(&header(0b11)), Some(1));
    }

    #[test]
    fn stereo_joint_and_dual_are_two_channels() {
        assert_eq!(channels_from_header(&header(0b00)), Some(2)); // stereo
        assert_eq!(channels_from_header(&header(0b01)), Some(2)); // joint stereo
        assert_eq!(channels_from_header(&header(0b10)), Some(2)); // dual channel
    }

    #[test]
    fn rejects_short_buffers_and_missing_sync() {
        assert_eq!(channels_from_header(&[]), None);
        assert_eq!(channels_from_header(&[0xFF, 0xFD, 0x50]), None);
        assert_eq!(channels_from_header(&[0x00, 0x00, 0x00, 0x00]), None);
    }

    #[test]
    fn rejects_reserved_version() {
        // sync=111, version=01 (reserved), layer=10 (Layer II), protection=1.
        // Everything but `version` is otherwise a valid Layer II header.
        assert_eq!(channels_from_header(&[0xFF, 0xED, 0x50, 0x00]), None);
    }

    #[test]
    fn rejects_non_layer_ii() {
        // sync=111, version=11 (MPEG1), layer=01 (Layer III, not II), protection=1.
        assert_eq!(channels_from_header(&[0xFF, 0xFB, 0x50, 0x00]), None);
    }

    #[test]
    fn rejects_reserved_bitrate_index() {
        // buf[2] top nibble = 1111 (reserved "bad" bitrate index).
        assert_eq!(channels_from_header(&[0xFF, 0xFD, 0xF0, 0x00]), None);
    }
}
