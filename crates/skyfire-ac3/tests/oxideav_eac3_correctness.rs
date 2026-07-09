//! E-AC-3 decode correctness gate (regression guard for OxideAV#13).
//!
//! Decodes the SAME committed real-broadcast E-AC-3 elementary stream two ways —
//!   (a) oxideav-ac3 DIRECTLY via `eac3::decode_eac3_packet` (no skyfire-ac3
//!       sample processing), and
//!   (b) ffmpeg's reference decode of the same bytes (committed golden PCM) —
//! and asserts they MATCH: sane peak (not railed), high waveform correlation
//! (PSNR ≥ 50 dB), and no silence dropouts.
//!
//! History: oxideav-ac3 v0.0.9 (rev 2d56c09) FAILED this — it railed to full
//! scale (peak 1.0 vs 0.33), was uncorrelated (PSNR ≈27 dB), and dropped ~84
//! windows (the france-2 "glitchy audio" bug). v0.0.10 (rev 8ea8f60) passes it
//! (peak 0.33, PSNR ≈59 dB, 0 dropouts). If this test regresses, real-broadcast
//! E-AC-3 decode has broken again.
//!
//! Fixtures (committed):
//!   fixtures/france2-3s.eac3                  — 3s of a real france-2 E-AC-3 ES
//!   fixtures/france2-3s.eac3.ffmpeg-s16le     — ffmpeg -f s16le -ac 2 of the same ES

use oxideav_ac3::eac3;

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    std::fs::read(p).unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// Decode an E-AC-3 elementary stream by calling oxideav-ac3's own decode entry
/// (`oxideav_ac3::eac3::decode_eac3_packet`) directly — NO skyfire-ac3 sample
/// processing. `frame.pcm_s16le` is oxideav-ac3's raw output, concatenated as-is.
fn decode_via_oxideav(es: &[u8]) -> Vec<u8> {
    let mut state = eac3::Eac3DecoderState::default();
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 6 <= es.len() {
        if !(es[off] == 0x0B && es[off + 1] == 0x77) {
            off += 1;
            continue;
        }
        let frmsiz = ((u16::from(es[off + 2]) & 0x07) << 8) | u16::from(es[off + 3]);
        let flen = ((frmsiz as usize) + 1) * 2;
        if flen == 0 || off + flen > es.len() {
            break;
        }
        if let Ok(frame) = eac3::decode_eac3_packet(&mut state, &es[off..off + flen]) {
            out.extend_from_slice(&frame.pcm_s16le);
        }
        off += flen;
    }
    out
}

/// De-interleave one channel to normalised f32.
fn chan(interleaved_s16: &[u8], ch: usize, c: usize) -> Vec<f32> {
    let s: Vec<i16> = interleaved_s16
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    s.iter()
        .skip(c)
        .step_by(ch)
        .map(|&v| v as f32 / 32768.0)
        .collect()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, &v| m.max(v.abs()))
}

/// Best-lag PSNR (dB) of `a` against reference `b` — how close the waveform is.
/// A correct decode is near-sample-exact (high PSNR); an uncorrelated decode is
/// low. Searches ±`maxlag` to absorb any codec-delay offset (per issue #13).
fn best_psnr(a: &[f32], b: &[f32], maxlag: i32) -> f64 {
    let n = a.len().min(b.len());
    if n < 4096 {
        return 0.0;
    }
    let win = 4096usize.min(n - maxlag as usize - 1);
    let start = maxlag as usize + 1;
    let mut best = 0.0f64;
    for lag in -maxlag..=maxlag {
        let mut sse = 0.0f64;
        for i in 0..win {
            let ai = start + i;
            let bi = ai as i32 + lag;
            let d = a[ai] as f64 - b[bi as usize] as f64;
            sse += d * d;
        }
        let mse = sse / win as f64;
        let psnr = if mse > 0.0 {
            10.0 * (1.0 / mse).log10()
        } else {
            f64::INFINITY
        };
        if psnr > best {
            best = psnr;
        }
    }
    best
}

/// Count 10ms windows where `a` is silent (<-60 dBFS) but reference `b` has
/// signal (>-40 dBFS) — the "full-second silence dropouts" from issue #13.
fn dropout_windows(a: &[f32], b: &[f32], sr: usize) -> usize {
    let w = sr / 100; // 10 ms
    let n = a.len().min(b.len());
    let mut drops = 0;
    let mut i = 0;
    while i + w <= n {
        let ea: f64 = a[i..i + w].iter().map(|&v| (v * v) as f64).sum::<f64>() / w as f64;
        let eb: f64 = b[i..i + w].iter().map(|&v| (v * v) as f64).sum::<f64>() / w as f64;
        if ea < 1e-6 && eb > 1e-4 {
            drops += 1;
        }
        i += w;
    }
    drops
}

/// Asserts oxideav-ac3 decodes real-broadcast E-AC-3 correctly — matching ffmpeg's
/// reference decode of the same committed bytes: sane peak (not railed), high
/// waveform correlation (PSNR ≥ 50 dB), and no silence dropouts. Regression guard
/// for the france-2 "glitchy audio" bug (OxideAV#13), fixed by v0.0.10.
#[test]
fn oxideav_ac3_decodes_real_eac3_matching_ffmpeg() {
    let es = fixture("france2-3s.eac3");
    let golden = fixture("france2-3s.eac3.ffmpeg-s16le"); // stereo s16le, ffmpeg reference

    let ours = decode_via_oxideav(&es);
    assert!(!ours.is_empty(), "oxideav-ac3 produced no PCM");

    for c in 0..2 {
        let refc = chan(&golden, 2, c);
        let ourc = chan(&ours, 2, c);
        let ref_peak = peak(&refc);
        let our_peak = peak(&ourc);
        let psnr = best_psnr(&ourc, &refc, 512);
        let drops = dropout_windows(&ourc, &refc, 48_000);
        println!(
            "ch{c}: ffmpeg peak={ref_peak:.3}  oxideav peak={our_peak:.3}  PSNR(ours vs ffmpeg)={psnr:.1} dB  oxideav-only-dropouts(10ms)={drops}"
        );

        // Reference sanity: the source decodes to a sane, non-railed level.
        assert!(
            ref_peak > 0.05 && ref_peak < 0.9,
            "ch{c}: reference peak should be sane, got {ref_peak}"
        );

        // CORRECTNESS #1 — no full-scale railing: our peak tracks the reference.
        assert!(
            our_peak < 0.9 && (our_peak - ref_peak).abs() < 0.1,
            "ch{c}: oxideav must not rail — peak {our_peak:.3} should track ffmpeg {ref_peak:.3}"
        );

        // CORRECTNESS #2 — near-sample-exact waveform (OxideAV#13 target ≥50 dB).
        assert!(
            psnr >= 50.0,
            "ch{c}: oxideav must match the reference waveform (PSNR {psnr:.1} dB, need ≥50)"
        );

        // CORRECTNESS #3 — no silence dropouts the reference lacks.
        assert_eq!(
            drops, 0,
            "ch{c}: oxideav must not drop audio the reference has"
        );
    }
}
