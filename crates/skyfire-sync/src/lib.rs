//! Audio-master A/V synchronisation for Skyfire.
//!
//! Audio is the master clock: media time is derived from the number of PCM
//! samples actually played out by the `AudioWorklet` (tied to the DAC, so
//! drift-free), anchored to the first audio sample's PTS. Decoded video frames
//! sit in a PTS-ordered queue and are presented / dropped / held against that
//! clock. Never wall-clock master; never slave audio to video.

/// Audio-master media clock.
#[derive(Debug, Clone, Copy)]
pub struct AudioClock {
    /// PTS (µs) of the first audio sample played — the anchor.
    pub anchor_pts_us: i64,
    /// Output sample rate (Hz).
    pub sample_rate: u32,
    /// PCM frames the worklet has output so far.
    pub samples_played: u64,
}

impl AudioClock {
    /// Current media time in microseconds (drift-free: derived from samples played).
    #[must_use]
    pub fn media_time_us(&self) -> i64 {
        debug_assert!(self.sample_rate > 0);
        self.anchor_pts_us
            + (self.samples_played as i64 * 1_000_000) / i64::from(self.sample_rate.max(1))
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

    #[test]
    fn media_time_advances_with_samples() {
        let c = AudioClock { anchor_pts_us: 1_000_000, sample_rate: 48_000, samples_played: 48_000 };
        assert_eq!(c.media_time_us(), 2_000_000); // anchor + exactly 1 s
    }

    #[test]
    fn frame_decisions() {
        assert_eq!(decide(1_000_000, 1_000_000, 20_000), FrameAction::Present);
        assert_eq!(decide(900_000, 1_000_000, 20_000), FrameAction::Drop);
        assert_eq!(decide(1_100_000, 1_000_000, 20_000), FrameAction::Hold);
    }
}
