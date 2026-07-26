//! Header-only channel-count probe for AC-3 / E-AC-3 sync frames.

use crate::AC3_SYNCWORD;

/// Channels contributed by each `acmod` value, before LFE.
/// ETSI TS 102 366 Table 5.8.
const ACMOD_CHANNELS: [u8; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

/// Minimal big-endian bit reader. No `unsafe`, no allocation.
struct BitReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    /// Reads `n` bits (n <= 16), or `None` past the end of the buffer.
    fn bits(&mut self, n: usize) -> Option<u16> {
        if self.pos + n > self.buf.len() * 8 {
            return None;
        }
        let mut out = 0u16;
        for _ in 0..n {
            let byte = self.buf[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            out = (out << 1) | u16::from(bit);
            self.pos += 1;
        }
        Some(out)
    }
}

/// Channel count for the sync frame at the start of `buf`, read from the
/// header alone — no decode, no decoder state.
///
/// Dispatches base AC-3 vs E-AC-3 on `bsid`, which sits at bit offset 40 in
/// both syntaxes precisely so that a reader can do this. Base AC-3 is
/// `bsid <= 8` (ETSI TS 102 366 §5.4.2.1); E-AC-3 is `bsid` 11–16 (§E.1.3.1.5).
///
/// Returns `None` when the buffer does not start with a sync frame, is too
/// short, or carries an unrecognised `bsid`. Callers MUST treat `None` as
/// "unknown" and never substitute a guess.
#[must_use]
pub fn channels_from_syncframe(buf: &[u8]) -> Option<u8> {
    if !crate::is_ac3_syncframe(buf) {
        return None;
    }
    debug_assert_eq!(
        u16::from(buf[0]) << 8 | u16::from(buf[1]),
        AC3_SYNCWORD,
        "is_ac3_syncframe guarantees the syncword"
    );

    // bsid is at bit 40 in both syntaxes.
    let bsid = BitReader { buf, pos: 40 }.bits(5)?;

    let (acmod, lfeon) = if bsid <= 8 {
        // Base AC-3: syncinfo(40) | bsid(5) bsmod(3) acmod(3) [cmixlev(2)]
        // [surmixlev(2)] [dsurmod(2)] lfeon(1)
        let mut r = BitReader::new(buf);
        r.skip(40 + 5 + 3);
        let acmod = r.bits(3)?;
        if acmod & 1 != 0 && acmod != 1 {
            r.skip(2); // cmixlev
        }
        if acmod & 4 != 0 {
            r.skip(2); // surmixlev
        }
        if acmod == 2 {
            r.skip(2); // dsurmod
        }
        (acmod, r.bits(1)?)
    } else if (11..=16).contains(&bsid) {
        // E-AC-3: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2)
        // [fscod2|numblkscod](2) acmod(3) lfeon(1)
        let mut r = BitReader::new(buf);
        r.skip(16 + 2 + 3 + 11);
        let _fscod = r.bits(2)?;
        r.skip(2); // fscod2 or numblkscod — same width either way
        let acmod = r.bits(3)?;
        (acmod, r.bits(1)?)
    } else {
        return None;
    };

    let base = *ACMOD_CHANNELS.get(usize::from(acmod))?;
    Some(base + u8::from(lfeon == 1))
}

#[cfg(test)]
mod tests {
    use super::channels_from_syncframe;

    /// Build a base AC-3 syncframe header with the given acmod/lfeon.
    /// Layout: syncword(16) crc1(16) fscod(2) frmsizecod(6) | bsid(5) bsmod(3) acmod(3) ... lfeon(1)
    fn ac3_header(acmod: u8, lfeon: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits.push_str("0000101101110111"); // syncword 0x0B77
        bits.push_str("0000000000000000"); // crc1
        bits.push_str("00"); // fscod = 48 kHz
        bits.push_str("000000"); // frmsizecod
        bits.push_str("01000"); // bsid = 8 -> base AC-3
        bits.push_str("000"); // bsmod
        bits.push_str(&format!("{acmod:03b}"));
        if acmod & 1 != 0 && acmod != 1 {
            bits.push_str("00"); // cmixlev
        }
        if acmod & 4 != 0 {
            bits.push_str("00"); // surmixlev
        }
        if acmod == 2 {
            bits.push_str("00"); // dsurmod
        }
        bits.push(if lfeon { '1' } else { '0' });
        while !bits.len().is_multiple_of(8) {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .map(|c| {
                c.iter()
                    .fold(0u8, |acc, b| (acc << 1) | u8::from(*b == b'1'))
            })
            .collect()
    }

    /// E-AC-3: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2)
    ///         numblkscod(2) acmod(3) lfeon(1) bsid(5)
    fn eac3_header(acmod: u8, lfeon: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits.push_str("0000101101110111"); // syncword
        bits.push_str("00"); // strmtyp = 0
        bits.push_str("000"); // substreamid
        bits.push_str("00000000000"); // frmsiz
        bits.push_str("00"); // fscod = 48 kHz (!= 3)
        bits.push_str("11"); // numblkscod = 6 blocks
        bits.push_str(&format!("{acmod:03b}"));
        bits.push(if lfeon { '1' } else { '0' });
        bits.push_str("10000"); // bsid = 16 -> E-AC-3
        while !bits.len().is_multiple_of(8) {
            bits.push('0');
        }
        bits.as_bytes()
            .chunks(8)
            .map(|c| {
                c.iter()
                    .fold(0u8, |acc, b| (acc << 1) | u8::from(*b == b'1'))
            })
            .collect()
    }

    #[test]
    fn ac3_acmod_table_matches_spec_table_5_8() {
        // ETSI TS 102 366 Table 5.8: acmod -> channel count, before LFE.
        for (acmod, want) in [
            (0u8, 2u8),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 3),
            (5, 4),
            (6, 4),
            (7, 5),
        ] {
            assert_eq!(
                channels_from_syncframe(&ac3_header(acmod, false)),
                Some(want),
                "acmod {acmod}"
            );
        }
    }

    #[test]
    fn ac3_lfe_adds_one_channel() {
        // acmod 7 + lfeon = 3/2.1 = 5.1 = 6 channels.
        assert_eq!(channels_from_syncframe(&ac3_header(7, true)), Some(6));
        assert_eq!(channels_from_syncframe(&ac3_header(2, true)), Some(3));
    }

    #[test]
    fn eac3_acmod_and_lfe_parse_at_annex_e_offsets() {
        assert_eq!(channels_from_syncframe(&eac3_header(2, false)), Some(2));
        assert_eq!(channels_from_syncframe(&eac3_header(7, true)), Some(6));
    }

    #[test]
    fn rejects_non_syncframe_and_short_buffers() {
        assert_eq!(channels_from_syncframe(&[]), None);
        assert_eq!(channels_from_syncframe(&[0x0B, 0x77]), None);
        assert_eq!(channels_from_syncframe(&[0xFF; 16]), None);
    }

    #[test]
    fn real_eac3_fixture_reports_the_ffprobe_channel_count() {
        // fixtures/france2-3s.eac3 is a raw E-AC-3 elementary stream.
        // `ffprobe -show_entries stream=codec_name,channels` -> eac3,2
        // (verified 2026-07-26).
        let data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/france2-3s.eac3"
        ))
        .expect("fixture");
        assert_eq!(channels_from_syncframe(&data), Some(2));
    }
}
