//! Proof that `oxideav-ac3` (at the rev this workspace pins) mis-decodes real
//! E-AC-3: it emits full-scale "railed" sample bursts that the source does NOT
//! contain. We decode the SAME committed elementary stream two ways —
//!   (a) oxideav-ac3 DIRECTLY (not through skyfire-ac3's wrapper), and
//!   (b) ffmpeg's reference decode of the same bytes (committed golden PCM) —
//! and show the reference is clean while oxideav-ac3 rails. This isolates fault
//! to oxideav-ac3 itself.
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

/// Proves the fault is in oxideav-ac3 (not skyfire-ac3, not the source): decode the
/// SAME committed E-AC-3 bytes via `oxideav_ac3::eac3::decode_eac3_packet` and via
/// ffmpeg (committed golden). The reference is clean and correlated; oxideav-ac3
/// rails to full scale AND is uncorrelated with the reference (issue OxideAV#13).
#[test]
fn oxideav_ac3_misdecodes_eac3_that_ffmpeg_decodes_correctly() {
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

        // Reference proves the SOURCE is cleanly decodable: sane level, not railed.
        assert!(
            ref_peak > 0.05 && ref_peak < 0.9,
            "ch{c}: reference peak should be sane, got {ref_peak}"
        );

        // FAULT #1 — oxideav-ac3 rails to full scale on the same bytes (ffmpeg does not).
        assert!(
            our_peak >= 0.99 && our_peak > ref_peak + 0.3,
            "ch{c}: oxideav should rail to full scale vs the clean reference (ffmpeg={ref_peak:.3}, oxideav={our_peak:.3})"
        );

        // FAULT #2 — oxideav-ac3 output is UNCORRELATED with the reference waveform.
        // A correct decode is near-sample-exact (issue #13 target ≥50 dB); this is
        // far below, i.e. the wrong waveform — not merely louder.
        assert!(
            psnr < 40.0,
            "ch{c}: oxideav should be far from the reference (PSNR {psnr:.1} dB « the ≥50 dB a correct decode gives)"
        );

        // FAULT #3 — oxideav-ac3 has silence dropouts where the reference has audio.
        assert!(
            drops > 0,
            "ch{c}: oxideav should show dropout windows the reference does not"
        );
    }
}
