/// 33‑bit PTS range (0 .. 2³³). ISO/IEC 13818‑1 §2.4.3.7.
pub const PTS_RANGE: u64 = 1u64 << 33;

/// 90 kHz PTS clock frequency.
pub const PTS_90KHZ: u64 = 90_000;

/// Convert a raw 33‑bit (modulo 2³³) PTS at 90 kHz into microseconds.
///
/// The result is always in `[0, (PTS_RANGE / PTS_90KHZ) × 10⁶)` ≈ [0, 47.7 s).
/// Callers combine this with wrap‑aware logic to produce monotonic media time.
#[must_use]
pub fn pts_33_to_us(raw: u64) -> i64 {
    debug_assert!(raw < PTS_RANGE, "PTS must be 33-bit (0 .. {PTS_RANGE})");
    ((raw.saturating_mul(100)) / 9) as i64
}

/// Compute the signed delta between two 33‑bit PTS values, handling wrap.
///
/// Returns `(new - old)` modulo 2³³, normalised to the range
/// `(-PTS_RANGE/2, +PTS_RANGE/2]` (at 90 kHz ticks). A positive result means
/// `new` is ahead of `old` in the wrapped timeline.
#[must_use]
pub fn pts_delta_33(new_raw: u64, old_raw: u64) -> i64 {
    debug_assert!(new_raw < PTS_RANGE);
    debug_assert!(old_raw < PTS_RANGE);
    let raw_diff = new_raw.wrapping_sub(old_raw) & (PTS_RANGE - 1);
    let half = (PTS_RANGE / 2) as i64;
    let diff = raw_diff as i64;
    if diff > half {
        diff - PTS_RANGE as i64
    } else {
        diff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pts_33_to_us_converts_correctly() {
        assert_eq!(pts_33_to_us(0), 0);
        assert_eq!(pts_33_to_us(90_000), 1_000_000);
        assert_eq!(pts_33_to_us(9), 100);
    }

    #[test]
    fn pts_delta_33_no_wrap() {
        assert_eq!(pts_delta_33(900, 0), 900);
        assert_eq!(pts_delta_33(0, 900), -900);
    }

    #[test]
    fn pts_delta_33_forward_wrap() {
        let old = PTS_RANGE - 900;
        let new = 900;
        let delta = pts_delta_33(new, old);
        assert!(delta > 0);
        assert_eq!(delta, 1800);
    }

    #[test]
    fn pts_delta_33_backward_wrap() {
        let old = 900;
        let new = PTS_RANGE - 900;
        let delta = pts_delta_33(new, old);
        assert!(delta < 0);
        assert_eq!(delta, -1800);
    }

    #[test]
    fn pts_delta_33_half_range() {
        let delta = pts_delta_33(PTS_RANGE / 2, 0);
        assert_eq!(delta, PTS_RANGE as i64 / 2);
    }
}
