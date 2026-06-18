//! AC-3 / E-AC-3 decoder for Skyfire.
//!
//! WebCodecs has no AC-3/E-AC-3 audio decoder (this is the gap that killed the
//! old MSE attempt). Audio is light, so a pure-Rust decoder compiled to WASM is
//! cheap: decode to interleaved PCM and push through a WebAudio `AudioWorklet`.
//! Header parsing today; full decode to follow.

/// AC-3 / E-AC-3 sync word (`0x0B77`).
pub const AC3_SYNCWORD: u16 = 0x0B77;

/// True if the buffer begins with an AC-3 / E-AC-3 sync frame.
#[must_use]
pub fn is_ac3_syncframe(buf: &[u8]) -> bool {
    buf.len() >= 2 && (u16::from(buf[0]) << 8 | u16::from(buf[1])) == AC3_SYNCWORD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_syncword() {
        assert!(is_ac3_syncframe(&[0x0B, 0x77, 0x00]));
        assert!(!is_ac3_syncframe(&[0x47, 0x00]));
    }
}
