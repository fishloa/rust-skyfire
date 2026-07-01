//! Multichannel → stereo downmix for WebAudio output.
//!
//! Browsers reliably render only stereo; multichannel PCM routed to channels
//! the device does not output goes silent (issue #43). We downmix to stereo in
//! WASM (float, before handoff) using the standard ITU-R BS.775 / ATSC A/52
//! coefficients so 5.1 sources produce audible stereo everywhere.
//!
//! Input is interleaved S16LE at `channels` channels in WAVE order
//! (L, R, C, LFE, Ls, Rs for 5.1, as emitted by `oxideav-ac3`); output is
//! interleaved stereo `f32` in [-1.0, 1.0].

/// −3 dB center/surround downmix coefficient (ITU-R BS.775; issue #43 spec).
pub const DOWNMIX_COEFF: f32 = 0.707;

/// Downmix interleaved S16LE PCM to interleaved stereo `f32`.
///
/// - `channels == 1` (mono): duplicated to both L and R.
/// - `channels == 2` (stereo): passthrough (scaled to `f32`).
/// - `channels == 6` (5.1, WAVE order L,R,C,LFE,Ls,Rs): `L' = L + k·C + k·Ls`,
///   `R' = R + k·C + k·Rs`, LFE dropped (`k = DOWNMIX_COEFF`).
/// - other counts: ch0→L, ch1→R, a center-like ch2→both·k, LFE (ch3 when ≥6)
///   dropped, remaining channels split L/R·k. Output clamped to [-1.0, 1.0].
#[must_use]
pub fn downmix_s16le_to_stereo_f32(pcm_s16le: &[u8], channels: u16) -> Vec<f32> {
    const SCALE: f32 = 32_768.0;
    let ch = channels.max(1) as usize;

    // Decode interleaved i16 → f32 in [-1, 1).
    let flat: Vec<f32> = pcm_s16le
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / SCALE)
        .collect();

    let frames = flat.len() / ch;
    let mut out = Vec::with_capacity(frames * 2);
    let k = DOWNMIX_COEFF;

    for f in 0..frames {
        let frame = &flat[f * ch..f * ch + ch];
        let (mut l, mut r) = match ch {
            1 => (frame[0], frame[0]),
            2 => (frame[0], frame[1]),
            // WAVE order: L, R, C, LFE, Ls, Rs (LFE dropped).
            6 => (
                frame[0] + k * frame[2] + k * frame[4],
                frame[1] + k * frame[2] + k * frame[5],
            ),
            // Generic: ch0→L, ch1→R, ch2 (center-like)→both·k, ch3 (LFE when
            // ≥6 ch)→drop, remaining split L/R·k.
            n => {
                let mut l = frame[0];
                let mut r = if n > 1 { frame[1] } else { frame[0] };
                if n > 2 {
                    l += k * frame[2];
                    r += k * frame[2];
                }
                for (i, &s) in frame.iter().enumerate().skip(3) {
                    if n >= 6 && i == 3 {
                        continue; // LFE
                    }
                    if i % 2 == 0 {
                        l += k * s;
                    } else {
                        r += k * s;
                    }
                }
                (l, r)
            }
        };
        l = l.clamp(-1.0, 1.0);
        r = r.clamp(-1.0, 1.0);
        out.push(l);
        out.push(r);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build interleaved S16LE bytes from i16 samples.
    fn s16le(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    const SCALE: f32 = 32_768.0;

    #[test]
    fn stereo_passes_through_scaled() {
        // Two stereo frames: (1000, -2000), (0, 32767).
        let pcm = s16le(&[1000, -2000, 0, 32767]);
        let out = downmix_s16le_to_stereo_f32(&pcm, 2);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 1000.0 / SCALE).abs() < 1e-6);
        assert!((out[1] - -2000.0 / SCALE).abs() < 1e-6);
        assert!((out[2] - 0.0).abs() < 1e-6);
        assert!((out[3] - 32767.0 / SCALE).abs() < 1e-6);
    }

    #[test]
    fn mono_duplicates_to_both_channels() {
        // Two mono frames: 500, -1500.
        let pcm = s16le(&[500, -1500]);
        let out = downmix_s16le_to_stereo_f32(&pcm, 1);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 500.0 / SCALE).abs() < 1e-6);
        assert!((out[1] - 500.0 / SCALE).abs() < 1e-6);
        assert!((out[2] - -1500.0 / SCALE).abs() < 1e-6);
        assert!((out[3] - -1500.0 / SCALE).abs() < 1e-6);
    }

    #[test]
    fn five_one_uses_itu_coefficients_and_drops_lfe() {
        // One 5.1 frame, WAVE order L,R,C,LFE,Ls,Rs.
        let (l, r, c, lfe, ls, rs) = (1000i16, 2000, 100, 9999, 10, 20);
        let pcm = s16le(&[l, r, c, lfe, ls, rs]);
        let out = downmix_s16le_to_stereo_f32(&pcm, 6);
        assert_eq!(out.len(), 2, "one 5.1 frame → one stereo frame");
        let k = DOWNMIX_COEFF;
        let want_l = (f32::from(l) + k * f32::from(c) + k * f32::from(ls)) / SCALE;
        let want_r = (f32::from(r) + k * f32::from(c) + k * f32::from(rs)) / SCALE;
        assert!(
            (out[0] - want_l).abs() < 1e-6,
            "L: got {} want {}",
            out[0],
            want_l
        );
        assert!(
            (out[1] - want_r).abs() < 1e-6,
            "R: got {} want {}",
            out[1],
            want_r
        );
        // LFE (9999) must NOT leak in: neither channel should be near lfe/scale.
        assert!(out[0] < 0.1 && out[1] < 0.1, "LFE must be dropped");
    }

    #[test]
    fn output_is_clamped_to_unit_range() {
        // 5.1 frame that oversums well past full-scale on both channels.
        let pcm = s16le(&[32767, 32767, 32767, 0, 32767, 32767]);
        let out = downmix_s16le_to_stereo_f32(&pcm, 6);
        assert!(out[0] <= 1.0 && out[0] >= -1.0, "L clamped, got {}", out[0]);
        assert!(out[1] <= 1.0 && out[1] >= -1.0, "R clamped, got {}", out[1]);
        assert!((out[0] - 1.0).abs() < 1e-6, "oversum clamps to +1.0");
    }
}
