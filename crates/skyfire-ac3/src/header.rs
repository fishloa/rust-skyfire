//! Header-only channel-count probe for AC-3 / E-AC-3 sync frames.
//!
//! Delegates the actual BSI bitstream parsing to the already-pinned
//! `oxideav-ac3` dependency (`oxideav_ac3::bsi` / `oxideav_ac3::eac3::bsi`)
//! rather than re-implementing the syntax table here — this module only
//! reads the one field (`bsid`) it needs to pick which upstream parser to
//! call, and maps the result back onto a total, decode-free API.

/// Channel count for the sync frame at the start of `buf`, read from the
/// header alone — no decode, no decoder state.
///
/// Dispatches base AC-3 vs E-AC-3 on `bsid`, which sits at bit offset 40 —
/// byte 5, top 5 bits — in both syntaxes precisely so that a reader can pick
/// a branch without a general-purpose bit reader. Base AC-3 is `bsid <= 8`
/// (ETSI TS 102 366 §5.4.2.1); E-AC-3 is `bsid` 11–16 (§E.1.3.1.5).
///
/// `bsid` 9/10 is intentionally treated as unrecognised here (`None`), even
/// though [`crate::IncrementalDecoder`] (`lib.rs`, near `bsid <= 10`) accepts
/// those as a legacy-compatible base-AC-3 variant and decodes them. That is
/// a deliberate difference in strictness, not a bug — see the matching
/// cross-reference comment in `lib.rs`.
///
/// For E-AC-3 this only inspects the first (independent, `strmtyp == 0`)
/// substream's `acmod`/`lfeon`. A genuine 7.1 program carried via a
/// *dependent* substream's `chanmap` is never read here, so such a stream
/// reports the independent substream's count (6), not `None` — a wrong
/// number rather than an honest "unknown".
///
/// Returns `None` when the buffer does not start with a sync frame, is too
/// short, or carries an unrecognised `bsid`. Callers MUST treat `None` as
/// "unknown" and never substitute a guess.
#[must_use]
pub fn channels_from_syncframe(buf: &[u8]) -> Option<u8> {
    if !crate::is_ac3_syncframe(buf) {
        return None;
    }

    // bsid is at bit offset 40 = byte 5, top 5 bits, in both syntaxes.
    let bsid = buf.get(5)? >> 3;

    let nchans = if bsid <= 8 {
        // Base AC-3: bsi() begins immediately after the 5-byte syncinfo.
        oxideav_ac3::bsi::parse(buf.get(5..)?).ok()?.nchans
    } else if (11..=16).contains(&bsid) {
        // E-AC-3 (Annex E): bsi() begins immediately after the 16-bit syncword.
        oxideav_ac3::eac3::bsi::parse(buf.get(2..)?).ok()?.nchans
    } else {
        return None;
    };

    Some(nchans)
}

#[cfg(test)]
mod tests {
    use super::channels_from_syncframe;

    /// Build a base AC-3 syncframe header with the given acmod/lfeon.
    ///
    /// Layout: syncword(16) crc1(16) fscod(2) frmsizecod(6) | bsid(5) bsmod(3)
    /// acmod(3) [cmixlev(2)] [surmixlev(2)] [dsurmod(2)] lfeon(1) dialnorm(5)
    /// compre(1) langcode_flag(1) audprodie(1) [dual-mono ch2 block]
    /// copyrightb(1) origbs(1) timecod1e(1) timecod2e(1) addbsie(1).
    ///
    /// The conditional fields (`cmixlev`, `surmixlev`, `dsurmod`) are filled
    /// with `1` bits rather than `0`. A parser that forgets to *skip* one of
    /// these shifts every field that follows — notably `lfeon` — so a
    /// zero-filled conditional would parse "correctly" by accident even with
    /// the skip missing entirely. Non-zero fill makes a missing skip change
    /// the asserted channel count, which is the point of this test data.
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
            bits.push_str("11"); // cmixlev
        }
        if acmod & 4 != 0 {
            bits.push_str("11"); // surmixlev
        }
        if acmod == 2 {
            bits.push_str("11"); // dsurmod
        }
        bits.push(if lfeon { '1' } else { '0' });
        bits.push_str("00001"); // dialnorm (non-reserved; value unused here)
        bits.push('0'); // compre = 0 (no compr byte)
        bits.push('0'); // langcode_flag = 0
        bits.push('0'); // audprodie = 0
        if acmod == 0 {
            bits.push_str("00001"); // dialnorm2
            bits.push('0'); // compr2e
            bits.push('0'); // langcod2e
            bits.push('0'); // audprodi2e
        }
        bits.push('0'); // copyrightb
        bits.push('0'); // origbs
        bits.push('0'); // timecod1e
        bits.push('0'); // timecod2e
        bits.push('0'); // addbsie
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

    /// Build an E-AC-3 (independent substream) syncframe header.
    ///
    /// Layout: syncword(16) strmtyp(2) substreamid(3) frmsiz(11) fscod(2)
    /// numblkscod(2) acmod(3) lfeon(1) bsid(5) dialnorm(5) compre(1)
    /// [dual-mono ch2 block] mixmdate(1) infomdate(1) addbsie(1).
    /// `strmtyp = 0` (independent), so the dependent-substream `chanmap`
    /// block is never present.
    fn eac3_header(acmod: u8, lfeon: bool) -> Vec<u8> {
        let mut bits = String::new();
        bits.push_str("0000101101110111"); // syncword
        bits.push_str("00"); // strmtyp = 0 (independent substream)
        bits.push_str("000"); // substreamid
        bits.push_str("00000011111"); // frmsiz = 31 -> 64-byte frame (upstream rejects < 8)
        bits.push_str("00"); // fscod = 48 kHz (!= 3)
        bits.push_str("11"); // numblkscod = 6 blocks
        bits.push_str(&format!("{acmod:03b}"));
        bits.push(if lfeon { '1' } else { '0' });
        bits.push_str("10000"); // bsid = 16 -> E-AC-3
        bits.push_str("00001"); // dialnorm
        bits.push('0'); // compre = 0
        if acmod == 0 {
            bits.push_str("00001"); // dialnorm2
            bits.push('0'); // compr2e
        }
        bits.push('0'); // mixmdate
        bits.push('0'); // infomdate
        bits.push('0'); // addbsie
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
    fn ac3_dsurmod_skip_is_pinned_for_acmod_2() {
        // acmod == 2 (2/0 stereo) is the one layout that carries a dsurmod
        // field. Build it with dsurmod = 0b11 and lfeon = 0: a parser that
        // omits the dsurmod skip reads the first of those two '1' bits as
        // lfeon (= 1) and reports 3 channels instead of 2. This pins the
        // skip against exactly that regression.
        let buf = ac3_header(2, false);
        assert_eq!(channels_from_syncframe(&buf), Some(2));
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
