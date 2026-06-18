//! MPEG-TS demux for Skyfire — low-level packet framing today; PSI (PAT/PMT)
//! and PES/PTS extraction to follow (planned: integrate rust-dvb `dvb-si`).
//!
//! The browser receiver demuxes the raw TS served by an upstream DVB-S2 receiver into per-ES
//! streams (video / audio) tagged with PTS, then hands them to the WebCodecs
//! video decoder and the WASM AC-3 audio decoder.

/// Length of a single MPEG-TS packet.
pub const TS_PACKET_LEN: usize = 188;
/// MPEG-TS sync byte.
pub const TS_SYNC_BYTE: u8 = 0x47;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
