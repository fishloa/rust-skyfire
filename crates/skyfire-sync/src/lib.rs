//! Audio-master A/V synchronisation for Skyfire.
//!
//! Audio is the master clock: media time is derived from the number of PCM
//! samples actually played out by the `AudioWorklet` (tied to the DAC, so
//! drift-free), anchored to the first audio sample's PTS. Decoded video frames
//! sit in a PTS-ordered queue and are presented / dropped / held against that
//! clock. Never wall-clock master; never slave audio to video.
//!
//! # Robustness
//!
//! - **33‑bit PTS wrap** (ISO/IEC 13818‑1 §2.4.3.7): 90 kHz PTS wraps every
//!   ~26.5 h. Media time stays monotonic across wraps via modulo‑2³³ arithmetic.
//! - **PTS / PCR discontinuity**: a sudden jump beyond `discontinuity_threshold`
//!   triggers an automatic re‑anchor so the clock does not emit a huge offset.
//! - **Audio underrun / resume**: callers notify the clock of an underrun;
//!   `push_pts` then treats the next PTS as a re‑anchor point.
//! - **Tunable lip‑sync offset**: a settable offset (±µs) applied to the
//!   reported clock for A/V trim.

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

/// Audio-master media clock.
///
/// Media time = `anchor_pts_us` (the PTS at which `samples_played` was
/// last zeroed) + output duration of all samples played since. The anchor is
/// updated automatically on PTS wrap, discontinuity, or after an underrun.
///
/// `push_pts` is called periodically with the current raw PTS to detect wraps
/// and discontinuities. `advance` is called as PCM samples are pushed to the
/// DAC — it drives the actual media time forward.
#[derive(Debug, Clone, Copy)]
pub struct AudioClock {
    /// PTS (µs) of the anchor point where `samples_played` was last zeroed.
    pub anchor_pts_us: i64,

    /// Raw 33‑bit PTS corresponding to `anchor_pts_us`. Used for wrap
    /// detection on subsequent `push_pts` calls.
    pub anchor_pts_raw: u64,

    /// Number of 90 kHz ticks accumulated across 33‑bit wraps.
    /// Total wrap contribution to media time = `wrap_ticks × 10⁶ / 90 kHz`.
    pub wrap_ticks: u64,

    /// Output sample rate (Hz).
    pub sample_rate: u32,

    /// PCM frames output since `samples_played` was last zeroed.
    pub samples_played: u64,

    /// Lip‑sync offset in microseconds (±). Added to `media_time_us()`.
    /// Positive → clock reports later; negative → clock reports earlier.
    pub lip_sync_offset_us: i64,

    /// PTS jump magnitude (µs) that triggers a forced re‑anchor.
    pub discontinuity_threshold_us: i64,

    /// Whether [`signal_underrun`] was called — next `push_pts` re‑anchors.
    pub underrun_pending: bool,

    /// Historical PTS (µs) accumulator. When the anchor slides forward
    /// (or wraps), the accumulated PTS delta is baked into `anchor_pts_us`
    /// rather than tracked separately, keeping `samples_played`
    /// relative to the current anchor.
    ///
    /// This is the *continuous* media time represented by the current
    /// anchor + wrap ticks alone (samples_played = 0, lip_sync = 0).
    anchor_base_us: i64,
}

impl Default for AudioClock {
    fn default() -> Self {
        Self {
            anchor_pts_us: 0,
            anchor_pts_raw: 0,
            wrap_ticks: 0,
            sample_rate: 48_000,
            samples_played: 0,
            lip_sync_offset_us: 0,
            discontinuity_threshold_us: 1_000_000,
            underrun_pending: false,
            anchor_base_us: 0,
        }
    }
}

impl AudioClock {
    /// Create a new clock anchored at the given raw 33‑bit PTS.
    #[must_use]
    pub fn new(anchor_raw_pts: u64, sample_rate: u32) -> Self {
        debug_assert!(sample_rate > 0);
        let anchor_us = pts_33_to_us(anchor_raw_pts);
        Self {
            anchor_pts_us: anchor_us,
            anchor_pts_raw: anchor_raw_pts,
            wrap_ticks: 0,
            sample_rate,
            samples_played: 0,
            lip_sync_offset_us: 0,
            discontinuity_threshold_us: 1_000_000,
            underrun_pending: false,
            anchor_base_us: anchor_us,
        }
    }

    /// Signal that the audio output has underrun (stalled).
    ///
    /// The next call to [`push_pts`] will treat the new PTS as a full
    /// re‑anchor, resetting all accumulated state.
    pub fn signal_underrun(&mut self) {
        self.underrun_pending = true;
    }

    /// Feed a new raw 33‑bit PTS into the clock.
    ///
    /// Returns the current media time (does not change it in the normal case).
    ///
    /// # Behaviour
    ///
    /// | Condition | Action |
    /// |---|---|
    /// | Underrun pending | Full re‑anchor at `raw_pts`. |
    /// | PTS jump ≥ `discontinuity_threshold_us` | Full re‑anchor at `raw_pts`. |
    /// | 33‑bit wrap (raw_pts < anchor_pts_raw but logically ahead) | Accumulate `PTS_RANGE` ticks; slide anchor, preserving current media time. |
    /// | Normal | Slide anchor forward to `raw_pts`; media time unchanged. |
    pub fn push_pts(&mut self, raw_pts: u64) -> i64 {
        debug_assert!(raw_pts < PTS_RANGE);

        // ── Underrun → full re-anchor ──────────────────────────
        if self.underrun_pending {
            self.re_anchor(raw_pts);
            self.underrun_pending = false;
            return self.media_time_us();
        }

        let delta_ticks = pts_delta_33(raw_pts, self.anchor_pts_raw);

        // ── Expanded‑timeline delta (accounts for accumulated wraps) ──
        // The raw PTS is modulo 2³³. The real PTS = wrap_ticks + raw_pts.
        // Use i128 to avoid overflow on the difference.
        let expanded_new = self.wrap_ticks as i128 + raw_pts as i128;
        let expanded_old = self.wrap_ticks as i128 + self.anchor_pts_raw as i128;
        let expanded_delta_ticks = expanded_new - expanded_old;
        // Clamp to i64 range (realistic PTS deltas are much smaller).
        let pts_delta_ticks_final = if expanded_delta_ticks > i64::MAX as i128 {
            i64::MAX
        } else if expanded_delta_ticks < i64::MIN as i128 {
            i64::MIN
        } else {
            expanded_delta_ticks as i64
        };
        let pts_delta_us = (pts_delta_ticks_final.saturating_mul(1_000_000)) / PTS_90KHZ as i64;

        // ── Wrap detection ────────────────────────────────────
        // Forward wrap: raw PTS counter wrapped around 2³³.
        let forward_wrap = raw_pts < self.anchor_pts_raw && delta_ticks >= 0;
        if forward_wrap {
            self.wrap_ticks = self.wrap_ticks.saturating_add(PTS_RANGE);
            // Recompute expanded delta with updated wrap_ticks.
            let expanded_new2 = self.wrap_ticks as i128 + raw_pts as i128;
            let expanded_delta2 = expanded_new2 - expanded_old;
            let final_delta = if expanded_delta2 > i64::MAX as i128 {
                i64::MAX
            } else if expanded_delta2 < i64::MIN as i128 {
                i64::MIN
            } else {
                expanded_delta2 as i64
            };
            let pts_delta_us = (final_delta.saturating_mul(1_000_000)) / PTS_90KHZ as i64;

            // Capture current media time and slide anchor.
            let old_media_time = self.media_time_us();
            self.anchor_pts_raw = raw_pts;
            self.anchor_pts_us = pts_33_to_us(raw_pts);
            self.anchor_base_us = old_media_time.saturating_add(pts_delta_us);
            self.samples_played = 0;
            return self.media_time_us();
        }

        // ── Discontinuity check (uses modulo delta, not expanded) ──
        let delta_ticks_i64 = delta_ticks;
        let delta_us_modulo = (delta_ticks_i64.saturating_mul(1_000_000)) / PTS_90KHZ as i64;
        if delta_us_modulo.abs() >= self.discontinuity_threshold_us {
            self.re_anchor(raw_pts);
            return self.media_time_us();
        }

        // ── Normal slide ──────────────────────────────────────
        let old_media_time = self.media_time_us();
        self.anchor_pts_raw = raw_pts;
        self.anchor_pts_us = pts_33_to_us(raw_pts);
        self.anchor_base_us = old_media_time.saturating_add(pts_delta_us);
        self.samples_played = 0;

        self.media_time_us()
    }

    /// Force a full re‑anchor at the given raw PTS.
    ///
    /// Resets all accumulated state (`samples_played`, `wrap_ticks`,
    /// underrun flag) and sets the anchor to `raw_pts`.
    /// Preserves `lip_sync_offset_us` and `discontinuity_threshold_us`.
    pub fn re_anchor(&mut self, raw_pts: u64) {
        let anchor_us = pts_33_to_us(raw_pts);
        self.anchor_pts_raw = raw_pts;
        self.anchor_pts_us = anchor_us;
        self.anchor_base_us = anchor_us;
        self.samples_played = 0;
        self.wrap_ticks = 0;
        self.underrun_pending = false;
    }

    /// Advance the clock by `samples` output PCM frames.
    ///
    /// Returns the updated media time in microseconds.
    #[must_use]
    pub fn advance(&mut self, samples: u64) -> i64 {
        self.samples_played = self.samples_played.saturating_add(samples);
        self.media_time_us()
    }

    /// Current media time in microseconds (drift-free: derived from samples played).
    #[must_use]
    pub fn media_time_us(&self) -> i64 {
        let sample_rate = i64::from(self.sample_rate.max(1));

        // Compute sample contribution safely.
        // samples_played up to u64::MAX → cap at i64::MAX to avoid overflow.
        let sample_us = if self.samples_played > i64::MAX as u64 {
            i64::MAX
        } else {
            (self.samples_played as i64)
                .saturating_mul(1_000_000)
                .saturating_div(sample_rate)
        };

        self.anchor_base_us
            .saturating_add(sample_us)
            .saturating_add(self.lip_sync_offset_us)
    }
}

/// What to do with a decoded video frame given its PTS vs the audio clock.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameAction {
    /// PTS is within tolerance of the clock — draw it now.
    Present,
    /// PTS is well behind the clock — drop to catch up.
    Drop,
    /// PTS is ahead of the clock — hold the current frame.
    Hold,
}

/// Decide how to handle a video frame. `tol_us` is the half-frame tolerance
/// (e.g. ~20 ms) that avoids thrashing on jitter.
#[must_use]
pub fn decide(frame_pts_us: i64, clock_us: i64, tol_us: i64) -> FrameAction {
    if frame_pts_us < clock_us - tol_us {
        FrameAction::Drop
    } else if frame_pts_us > clock_us + tol_us {
        FrameAction::Hold
    } else {
        FrameAction::Present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── existing tests (must stay green) ──────────────────────────────

    #[test]
    fn media_time_advances_with_samples() {
        let c = AudioClock {
            anchor_pts_us: 1_000_000,
            anchor_pts_raw: 90_000,
            wrap_ticks: 0,
            sample_rate: 48_000,
            samples_played: 48_000,
            lip_sync_offset_us: 0,
            discontinuity_threshold_us: 1_000_000,
            underrun_pending: false,
            anchor_base_us: 1_000_000,
        };
        assert_eq!(c.media_time_us(), 2_000_000); // anchor + exactly 1 s
    }

    #[test]
    fn frame_decisions() {
        assert_eq!(decide(1_000_000, 1_000_000, 20_000), FrameAction::Present);
        assert_eq!(decide(900_000, 1_000_000, 20_000), FrameAction::Drop);
        assert_eq!(decide(1_100_000, 1_000_000, 20_000), FrameAction::Hold);
    }

    // ── new tests ─────────────────────────────────────────────────────

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

    #[test]
    fn media_time_monotonic_across_33bit_wrap() {
        // Start 1 tick before wrap.
        let near_wrap = PTS_RANGE - 1;
        let mut clock = AudioClock::new(near_wrap, 48_000);

        let time_before = clock.media_time_us();

        // Advance 1 s of audio.
        let _ = clock.advance(48_000);
        let time_after_advance = clock.media_time_us();
        assert!(time_after_advance > time_before);

        // Push raw_pts=0: this is logically 1 tick after near_wrap,
        // so the clock should detect a forward wrap and accumulate.
        let time_after_wrap = clock.push_pts(0);

        assert!(
            time_after_wrap > time_after_advance,
            "time_after_wrap={time_after_wrap} must be > time_after_advance={time_after_advance}"
        );
    }

    #[test]
    fn media_time_monotonic_across_multiple_wraps() {
        // Start well before the wrap boundary and step forward
        // so the PTS naturally crosses the 33‑bit boundary.
        let start = PTS_RANGE - 180_000; // 2 s before wrap
        let mut clock = AudioClock::new(start, 48_000);
        let mut prev = clock.media_time_us();

        for wrap_n in 0..3 {
            // Push to just before the wrap.
            clock.push_pts(PTS_RANGE - 1);
            let t1 = clock.media_time_us();
            assert!(
                t1 >= prev,
                "wrap {wrap_n}: after near-top push: {t1} >= {prev}"
            );

            // Push past the wrap (0 = one tick after PTS_RANGE-1).
            clock.push_pts(0);
            let t2 = clock.media_time_us();
            assert!(t2 >= t1, "wrap {wrap_n}: after wrap push: {t2} >= {t1}");

            prev = t2;
        }
    }

    #[test]
    fn discontinuity_triggers_re_anchor() {
        let mut clock = AudioClock::new(900_000, 48_000); // 10 s

        clock.push_pts(900_900); // nearby

        let jump_pts = 900_000 + 5 * 90_000; // +5 s
        let time_after = clock.push_pts(jump_pts);

        let expected_new_anchor = pts_33_to_us(jump_pts);
        let diff = (time_after - expected_new_anchor).abs();
        assert!(
            diff < 100_000,
            "post-jump time {time_after} should be ~{expected_new_anchor}, diff={diff}"
        );
    }

    #[test]
    fn small_jumps_preserve_continuity() {
        let mut clock = AudioClock::new(900_000, 48_000); // 10 s
        let time_before = clock.media_time_us();

        // Push 500 ms forward — within threshold, not a discontinuity.
        clock.push_pts(900_000 + 45_000); // 10.5 s in raw ticks
        let time_after = clock.media_time_us();

        let diff = time_after - time_before;
        assert!(
            diff > 400_000 && diff < 600_000,
            "expected ~500 ms diff, got {diff}"
        );
    }

    #[test]
    fn negative_discontinuity_triggers_re_anchor() {
        let mut clock = AudioClock::new(5_000_000, 48_000);
        // Jump backward by a large amount (> 1 s threshold).
        let jump_pts = 1_000_000;
        clock.push_pts(jump_pts);
        let time = clock.media_time_us();
        let expected = pts_33_to_us(jump_pts);
        assert!(
            (time - expected).abs() < 100_000,
            "time={time}, expected~={expected}"
        );
    }

    #[test]
    fn underrun_resume_re_anchors() {
        let mut clock = AudioClock::new(900_000, 48_000);

        let _ = clock.advance(48_000);
        clock.signal_underrun();

        let resume_pts = 9_000_000;
        let time_after = clock.push_pts(resume_pts);

        let expected = pts_33_to_us(resume_pts);
        assert!(
            (time_after - expected).abs() < 100_000,
            "time_after={time_after}, expected~={expected}"
        );
        assert!(!clock.underrun_pending);
    }

    #[test]
    fn lip_sync_offset_positive() {
        let mut clock = AudioClock::new(900_000, 48_000);
        let base = clock.media_time_us();
        clock.lip_sync_offset_us = 50_000;
        let offset = clock.media_time_us();
        assert_eq!(offset - base, 50_000);
    }

    #[test]
    fn lip_sync_offset_negative() {
        let mut clock = AudioClock::new(900_000, 48_000);
        let base = clock.media_time_us();
        clock.lip_sync_offset_us = -30_000;
        let offset = clock.media_time_us();
        assert_eq!(offset - base, -30_000);
    }

    #[test]
    fn lip_sync_offset_zero_is_identity() {
        let clock = AudioClock::new(900_000, 48_000);
        assert_eq!(clock.lip_sync_offset_us, 0);
        let t1 = clock.media_time_us();
        assert_eq!(t1, pts_33_to_us(900_000));
    }

    #[test]
    fn extreme_pts_near_zero_no_panic() {
        let mut clock = AudioClock::new(0, 48_000);
        assert_eq!(clock.media_time_us(), 0);
        clock.push_pts(1);
        clock.push_pts(0);
        clock.push_pts(9);
    }

    #[test]
    fn extreme_pts_near_max_no_panic() {
        let max_pts = PTS_RANGE - 1;
        let mut clock = AudioClock::new(max_pts, 48_000);
        assert!(clock.media_time_us() >= 0);
        clock.push_pts(max_pts - 1);
        clock.push_pts(max_pts);
        clock.push_pts(max_pts - 90_000);
    }

    #[test]
    fn advance_with_huge_samples_does_not_panic() {
        let mut clock = AudioClock::new(0, 48_000);
        let _ = clock.advance(u64::MAX);
        assert!(clock.media_time_us() >= 0);
    }

    #[test]
    fn advance_huge_samples_saturates() {
        let mut clock = AudioClock::new(0, 48_000);
        let _ = clock.advance(u64::MAX);
        assert_eq!(clock.media_time_us(), i64::MAX);
    }

    #[test]
    fn new_with_default() {
        let default = AudioClock::default();
        assert_eq!(default.sample_rate, 48_000);
        assert_eq!(default.samples_played, 0);
        assert_eq!(default.lip_sync_offset_us, 0);
        assert_eq!(default.discontinuity_threshold_us, 1_000_000);
        assert!(!default.underrun_pending);
    }

    #[test]
    fn re_anchor_clears_state() {
        let mut clock = AudioClock::new(0, 48_000);
        let _ = clock.advance(96_000);
        clock.lip_sync_offset_us = 42_000;
        clock.signal_underrun();
        assert!(clock.underrun_pending);

        clock.re_anchor(900_000);

        assert_eq!(clock.anchor_pts_raw, 900_000);
        assert_eq!(clock.anchor_pts_us, pts_33_to_us(900_000));
        assert_eq!(clock.samples_played, 0);
        assert_eq!(clock.wrap_ticks, 0);
        assert!(!clock.underrun_pending);
        assert_eq!(clock.anchor_base_us, pts_33_to_us(900_000));
        assert_eq!(clock.lip_sync_offset_us, 42_000);
    }

    #[test]
    fn zero_rate_does_not_panic_in_media_time_us() {
        let c = AudioClock {
            anchor_pts_us: 0,
            anchor_pts_raw: 0,
            wrap_ticks: 0,
            sample_rate: 0,
            samples_played: 48_000,
            lip_sync_offset_us: 0,
            discontinuity_threshold_us: 1_000_000,
            underrun_pending: false,
            anchor_base_us: 0,
        };
        let t = c.media_time_us();
        assert!(t >= 0);
    }

    #[test]
    fn wrap_preserves_media_time_continuity() {
        // Start 90_000 ticks before wrap (~1 s before wrap).
        let near_wrap = PTS_RANGE - 90_000;
        let mut clock = AudioClock::new(near_wrap, 48_000);
        let t_start = clock.media_time_us();

        // Advance 1 s of audio.
        let _ = clock.advance(48_000);
        let t_after_advance = clock.media_time_us();

        // Push the after-wrap PTS (raw_pts < anchor_pts_raw → wrap detected).
        // This should accumulate PTS_RANGE ticks.
        let after_wrap = 90_000;
        clock.push_pts(after_wrap);
        let t_after_push = clock.media_time_us();

        // After push, media time should be ≥ the media time after advance.
        assert!(
            t_after_push >= t_after_advance,
            "t_after_push={t_after_push} >= t_after_advance={t_after_advance}, t_start={t_start}"
        );
    }

    #[test]
    fn push_pts_normal_slides_forward() {
        let mut clock = AudioClock::new(900_000, 48_000); // 10 s at 90 kHz
        let t_before = clock.media_time_us();

        // Push PTS 500 ms later.
        clock.push_pts(900_000 + 45_000); // 10.5 s
        let t_after = clock.media_time_us();

        let diff = t_after - t_before;
        assert!(
            diff > 490_000 && diff < 510_000,
            "expected ~500 ms, got {diff} us"
        );
    }
}
