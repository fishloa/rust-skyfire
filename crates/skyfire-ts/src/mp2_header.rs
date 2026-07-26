//! Header-only channel-count probe for MPEG-1/2 Layer II frames.

/// Channel count from an MPEG audio frame header.
///
/// `mode` occupies bits 24–25 — the top two bits of byte 3. `0b11` is
/// single_channel (mono); every other value carries two channels
/// (stereo, joint_stereo, dual_channel). ISO/IEC 11172-3 §2.4.1.3.
///
/// Returns `None` unless the buffer starts with a frame sync (11 set bits).
#[must_use]
pub fn channels_from_header(buf: &[u8]) -> Option<u8> {
    if buf.len() < 4 {
        return None;
    }
    // Frame sync: 11 bits all set.
    if buf[0] != 0xFF || (buf[1] & 0xE0) != 0xE0 {
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
}
