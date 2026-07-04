use crate::audio_clock::AudioClock;
use crate::frame_queue::{FrameAction, VideoFrame, VideoFrameQueue};

/// Current A/V latency snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Latency {
    /// Signed offset in microseconds: positive = video ahead of audio,
    /// negative = video behind audio. `None` when the queue is empty
    /// (no frame to measure against).
    pub offset_us: Option<i64>,
    /// Whether the video pipeline is currently stalled (no frames available
    /// and the clock has advanced past the last known PTS).
    pub stalled: bool,
}

/// Configuration for the catch‑up burst mechanism.
#[derive(Debug, Clone, Copy)]
pub struct CatchUpConfig {
    /// When video is behind the audio clock by this many microseconds, the
    /// controller begins dropping frames in bursts to catch up.
    pub behind_threshold_us: i64,
    /// Maximum number of frames to drop in a single catch‑up burst.
    /// After this many consecutive drops, the burst ends even if still behind.
    pub max_burst_drops: usize,
    /// After a catch‑up burst ends (either because the video caught up or
    /// `max_burst_drops` was reached), no new burst starts until this
    /// cooldown period (in microseconds of audio clock advancement) has
    /// elapsed. Prevents repeated bursts after a natural recovery.
    pub burst_cooldown_us: i64,
}

impl Default for CatchUpConfig {
    fn default() -> Self {
        Self {
            behind_threshold_us: 40_000,
            max_burst_drops: 4,
            burst_cooldown_us: 500_000,
        }
    }
}

/// High‑level A/V sync controller wrapping an audio clock and a video
/// present queue. Implements catch‑up burst drop policy, stall detection,
/// and latency reporting on top of the lower‑level [`AudioClock`] and
/// [`VideoFrameQueue`] primitives.
///
/// Audio remains the master; the controller never slaves audio to video
/// and never uses a wall clock as master.
pub struct SyncController {
    /// The audio‑master media clock.
    pub clock: AudioClock,
    /// The PTS‑ordered video present queue.
    pub queue: VideoFrameQueue,
    /// Catch‑up burst configuration.
    pub catch_up: CatchUpConfig,
    /// Last known PTS of the most‑recently presented (or peeked) frame,
    /// used for stall detection and offset calculation.
    last_frame_pts_us: Option<i64>,
    /// Consecutive drops in the current catch‑up burst.
    pub(crate) burst_drop_count: usize,
    /// Audio clock time (µs) at which the last catch‑up burst ended.
    last_burst_end_clock_us: i64,
    /// Whether the video pipeline is currently stalled.
    stalled: bool,
}

impl SyncController {
    /// Create a new sync controller.
    #[must_use]
    pub const fn new(clock: AudioClock, queue: VideoFrameQueue, catch_up: CatchUpConfig) -> Self {
        Self {
            clock,
            queue,
            catch_up,
            last_frame_pts_us: None,
            burst_drop_count: 0,
            last_burst_end_clock_us: i64::MIN,
            stalled: false,
        }
    }

    /// Advance the audio clock by `samples` PCM frames.
    #[must_use]
    pub fn advance_audio(&mut self, samples: u64) -> i64 {
        self.clock.advance(samples)
    }

    /// Push a decoded video frame into the present queue.
    pub fn push_video(&mut self, frame: VideoFrame) {
        self.queue.push(frame);
        if self.stalled {
            self.stalled = false;
        }
    }

    /// Tick the sync loop: pop the next frame from the queue against the
    /// current audio clock time, applying catch‑up burst policy.
    ///
    /// Returns `None` when the queue is empty or all frames are held.
    /// Otherwise returns the action and frame as decided by the policy.
    ///
    /// # Catch‑up burst policy
    ///
    /// When the head frame is more than `catch_up_behind_threshold_us` behind
    /// the audio clock, frames are dropped in bursts. A burst can drop at
    /// most `max_burst_drops` frames consecutively. After a burst, a cooldown
    /// period prevents a new burst until the audio clock has advanced by
    /// `burst_cooldown_us`.
    pub fn tick(&mut self) -> Option<(FrameAction, VideoFrame)> {
        let clock_us = self.clock.media_time_us();

        // ── Stall detection ──────────────────────────────────────────
        if self.queue.is_empty() {
            if let Some(last_pts) = self.last_frame_pts_us
                && clock_us > last_pts
            {
                self.stalled = true;
            }
            return None;
        }

        // ── Peek at the head frame to decide catch‑up ────────────────
        let head = self.queue.peek()?;

        // Check if we're in a catch‑up scenario: the head frame is behind
        // by more than the configured threshold.
        let behind = clock_us - head.pts_us;

        let in_burst = self.burst_drop_count > 0;

        let should_catch_up = behind > self.catch_up.behind_threshold_us
            && (self.burst_drop_count < self.catch_up.max_burst_drops);

        // Check cooldown: if a previous burst ended, don't start a new one
        // until enough clock time has elapsed.
        let in_cooldown = if self.last_burst_end_clock_us > i64::MIN {
            clock_us - self.last_burst_end_clock_us < self.catch_up.burst_cooldown_us
        } else {
            false
        };

        let in_burst_or_catching = if in_burst {
            // Continue existing burst.
            should_catch_up
        } else if should_catch_up && !in_cooldown {
            // Start a new burst.
            true
        } else {
            false
        };

        if in_burst_or_catching {
            // Drop the head frame as part of catch‑up burst.
            self.queue.pop_head();
            self.queue.dropped_late_count = self.queue.dropped_late_count.saturating_add(1);
            self.burst_drop_count += 1;

            let dropped_frame = head;

            // Check if burst should end.
            let burst_ended = self.burst_drop_count >= self.catch_up.max_burst_drops
                || head.pts_us >= clock_us - self.catch_up.behind_threshold_us;

            if burst_ended {
                self.last_burst_end_clock_us = clock_us;
                self.burst_drop_count = 0;
            }

            return Some((FrameAction::Drop, dropped_frame));
        }

        // ── Normal pop path (burst ended or not catching up) ────────
        let result = self.queue.pop(clock_us);

        // If the normal pop ended a burst (because the head is now within
        // threshold), reset burst state.
        if self.burst_drop_count > 0 && result.is_some() {
            self.last_burst_end_clock_us = clock_us;
            self.burst_drop_count = 0;
        }

        if let Some((action, frame)) = result {
            self.last_frame_pts_us = Some(frame.pts_us);
            Some((action, frame))
        } else {
            None
        }
    }

    /// Current A/V latency snapshot.
    #[must_use]
    pub fn latency(&self) -> Latency {
        let clock_us = self.clock.media_time_us();
        let offset_us = self.queue.peek().map(|f| f.pts_us - clock_us);
        Latency {
            offset_us,
            stalled: self.stalled,
        }
    }

    /// Whether the video pipeline is stalled.
    #[must_use]
    pub const fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Signal an audio underrun to the wrapped clock.
    pub const fn signal_underrun(&mut self) {
        self.clock.signal_underrun();
    }

    /// Feed a raw PTS to the wrapped clock.
    #[must_use]
    pub fn push_pts(&mut self, raw_pts: u64) -> i64 {
        self.clock.push_pts(raw_pts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_clock::AudioClock;
    use crate::frame_queue::{VideoFrame, VideoFrameQueue};

    /// Advance the audio clock by `duration_us` at 48 kHz.
    fn advance_clock_us(clock: &mut AudioClock, duration_us: u64) {
        let samples = duration_us.saturating_mul(u64::from(clock.sample_rate)) / 1_000_000;
        let _ = clock.advance(samples);
    }

    fn make_controller(capacity: usize, tol_us: i64, drop_late_us: i64) -> SyncController {
        let clock = AudioClock::new(0, 48_000);
        let queue = VideoFrameQueue::new(capacity, tol_us, drop_late_us);
        let catch_up = CatchUpConfig {
            behind_threshold_us: 50_000,
            max_burst_drops: 3,
            burst_cooldown_us: 200_000,
        };
        SyncController::new(clock, queue, catch_up)
    }

    #[test]
    fn catch_up_drops_when_behind() {
        let mut ctrl = make_controller(8, 20_000, 200_000);

        // Push frames that are well behind the clock.
        ctrl.push_video(VideoFrame {
            pts_us: 10_000,
            handle: 0,
        });
        ctrl.push_video(VideoFrame {
            pts_us: 30_000,
            handle: 1,
        });
        ctrl.push_video(VideoFrame {
            pts_us: 50_000,
            handle: 2,
        });
        ctrl.push_video(VideoFrame {
            pts_us: 200_000,
            handle: 3,
        });

        // Advance audio clock to 200_000 us. Frames 0/1/2 are ~150-190k behind,
        // well beyond behind_threshold_us (50k). Frame 3 is at clock time.
        advance_clock_us(&mut ctrl.clock, 200_000);

        // First tick: head is 10_000, behind by 190_000 > 50k → drop (burst).
        let (action, f) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Drop);
        assert_eq!(f.handle, 0);

        // Second: head 30_000, behind by 170_000 → drop (burst).
        let (action, f) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Drop);
        assert_eq!(f.handle, 1);

        // Third: head 50_000, behind by 150_000 → drop (burst).
        // max_burst_drops = 3, so this is the last allowed drop in this burst.
        let (action, f) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Drop);
        assert_eq!(f.handle, 2);

        // Fourth: burst ended (cooldown started). Head is 200_000, within
        // tolerance of clock (200_000) → Present.
        let (action, f) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(f.handle, 3);

        assert!(ctrl.queue.is_empty());
    }

    #[test]
    fn catch_up_burst_respects_max_drops() {
        let mut ctrl = make_controller(8, 20_000, 500_000);

        // Push many late frames.
        for i in 0..10 {
            ctrl.push_video(VideoFrame {
                pts_us: i * 10_000,
                handle: i as u64,
            });
        }

        // Advance clock to 500_000 us — all frames are far behind.
        advance_clock_us(&mut ctrl.clock, 500_000);

        // First burst should drop exactly max_burst_drops (3) frames.
        let mut dropped = 0;
        for _ in 0..3 {
            let (action, _) = ctrl.tick().unwrap();
            assert_eq!(action, FrameAction::Drop);
            dropped += 1;
        }
        assert_eq!(dropped, 3);

        // After burst, cooldown applies. Next tick should NOT drop
        // (even though still behind) because cooldown hasn't elapsed.
        // Since all remaining frames are still behind, the normal pop
        // path will decide Drop (frames are behind tol). But the catch-up
        // burst is in cooldown. The regular pop() may still drop via its
        // own decide() logic, but that's not a catch-up burst.
        // We check that burst state was reset.
        assert_eq!(ctrl.burst_drop_count, 0);
    }

    #[test]
    fn catch_up_cooldown_prevents_repeated_bursts() {
        let mut ctrl = make_controller(8, 20_000, 500_000);

        // Push frames that trigger a burst.
        for i in 0..6 {
            ctrl.push_video(VideoFrame {
                pts_us: i * 10_000,
                handle: i as u64,
            });
        }

        // Advance clock to put frames behind.
        advance_clock_us(&mut ctrl.clock, 200_000);

        // Tick: first max_burst_drops (3) ticks should Drop as burst,
        // then the burst ends (cooldown starts). Remaining frames are
        // still behind but catch-up won't trigger again.
        let mut burst_drops = 0;
        for _ in 0..10 {
            match ctrl.tick() {
                Some((FrameAction::Drop, _)) => burst_drops += 1,
                Some((FrameAction::Hold, _)) => break,
                None => break,
                _ => {}
            }
        }
        // Burst should have fired at most max_burst_drops times.
        assert!(burst_drops <= ctrl.catch_up.max_burst_drops);
        assert_eq!(ctrl.burst_drop_count, 0); // burst state reset

        // Push more frames that would be behind the clock.
        advance_clock_us(&mut ctrl.clock, 10_000);
        for i in 0..3 {
            ctrl.push_video(VideoFrame {
                pts_us: ctrl.clock.media_time_us() - 100_000,
                handle: 100 + i as u64,
            });
        }

        // Still within cooldown (only ~10_000 us since burst ended,
        // cooldown is 200_000 us) → burst must NOT fire.
        let saved_burst = ctrl.burst_drop_count;
        for _ in 0..3 {
            match ctrl.tick() {
                Some((FrameAction::Drop, _)) => { /* normal decide may drop */ }
                Some((FrameAction::Hold, _)) => break,
                None => break,
                _ => {}
            }
        }
        // Burst counter must not have changed.
        assert_eq!(ctrl.burst_drop_count, saved_burst);
    }

    #[test]
    fn no_catch_up_when_within_threshold() {
        let mut ctrl = make_controller(8, 20_000, 200_000);

        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        ctrl.push_video(VideoFrame {
            pts_us: 140_000,
            handle: 1,
        });

        // Advance clock to 130_000 us. Frame 0 is 30k behind, frame 1 is
        // 10k ahead. Both within behind_threshold_us (50k).
        advance_clock_us(&mut ctrl.clock, 130_000);

        // Frame 0: 100k vs 130k → behind by 30k. Within tol (20k)? No — it's
        // 30k behind, so decide would Drop it via normal logic. But catch-up
        // should NOT trigger because 30k < 50k threshold.
        // The normal pop drops it. That's fine — catch-up didn't trigger.
        let (_action, _f) = ctrl.tick().unwrap();
        // It may be Drop from normal decide, but NOT from catch-up burst.
        // Verify burst counter didn't increment.
        assert_eq!(ctrl.burst_drop_count, 0);

        // Whether it's Drop or Hold depends on tol_us, but importantly
        // the catch-up burst was not activated.
    }

    // ── Stall detection tests ───────────────────────────────────────

    #[test]
    fn stall_detected_when_queue_empty_and_clock_ahead() {
        let mut ctrl = make_controller(8, 20_000, 100_000);

        // Push and present one frame so last_frame_pts_us is set.
        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);
        let (action, _) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Present);

        // Queue is now empty. Advance clock past last frame PTS.
        advance_clock_us(&mut ctrl.clock, 50_000); // clock at ~150_000

        // Tick returns None (empty queue) and should detect stall.
        assert!(ctrl.tick().is_none());
        assert!(ctrl.is_stalled());

        let lat = ctrl.latency();
        assert!(lat.stalled);
        assert!(lat.offset_us.is_none()); // empty queue → no offset
    }

    #[test]
    fn stall_clears_when_frame_arrives() {
        let mut ctrl = make_controller(8, 20_000, 100_000);

        // Present frame 0.
        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);
        ctrl.tick();

        // Drive clock past last frame → stall.
        advance_clock_us(&mut ctrl.clock, 50_000);
        assert!(ctrl.tick().is_none());
        assert!(ctrl.is_stalled());

        // Push a new frame → stall clears.
        ctrl.push_video(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        assert!(!ctrl.is_stalled());
    }

    #[test]
    fn no_stall_when_queue_has_frames() {
        let mut ctrl = make_controller(8, 20_000, 100_000);

        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        ctrl.push_video(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });

        // Advance clock far ahead — frames are late but queue is not empty.
        advance_clock_us(&mut ctrl.clock, 500_000);

        // Tick should drop frames (late), but not stall.
        let mut saw_drop = false;
        while let Some((action, _)) = ctrl.tick() {
            if action == FrameAction::Drop {
                saw_drop = true;
            }
        }
        assert!(saw_drop);
        assert!(!ctrl.is_stalled());
    }

    // ── Latency tests ───────────────────────────────────────────────

    #[test]
    fn latency_reports_offset_when_queue_has_frames() {
        let mut ctrl = make_controller(8, 20_000, 200_000);

        ctrl.push_video(VideoFrame {
            pts_us: 150_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);

        let lat = ctrl.latency();
        assert!(!lat.stalled);
        // Head PTS is 150_000, clock is ~100_000 → offset = +50_000 (ahead).
        assert!(lat.offset_us.is_some());
        let offset = lat.offset_us.unwrap();
        assert!(offset > 0, "expected positive offset, got {offset}");
        assert!(offset < 60_000, "offset={offset} should be ~50k");
    }

    #[test]
    fn latency_reports_negative_when_behind() {
        let mut ctrl = make_controller(8, 20_000, 500_000);

        ctrl.push_video(VideoFrame {
            pts_us: 50_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 200_000);

        let lat = ctrl.latency();
        assert!(!lat.stalled);
        let offset = lat.offset_us.unwrap();
        assert!(offset < 0, "expected negative offset, got {offset}");
    }

    #[test]
    fn latency_none_when_queue_empty() {
        let ctrl = make_controller(8, 20_000, 100_000);
        let lat = ctrl.latency();
        assert!(lat.offset_us.is_none());
        assert!(!lat.stalled);
    }

    #[test]
    fn latency_stalled_after_underrun() {
        let mut ctrl = make_controller(8, 20_000, 100_000);

        // Present one frame, then empty queue.
        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);
        ctrl.tick();

        // Advance clock past last frame → stall.
        advance_clock_us(&mut ctrl.clock, 100_000);
        ctrl.tick();

        let lat = ctrl.latency();
        assert!(lat.stalled);
        assert!(lat.offset_us.is_none());
    }

    // ── Edge case tests ─────────────────────────────────────────────

    #[test]
    fn extreme_empty_tick_no_panic() {
        let mut ctrl = make_controller(8, 20_000, 100_000);
        // Tick with no frames.
        assert!(ctrl.tick().is_none());
        // Push and present a frame so last_frame_pts_us is set.
        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);
        ctrl.tick();
        // Now advance clock far past last frame → stall.
        advance_clock_us(&mut ctrl.clock, 1_000_000);
        assert!(ctrl.tick().is_none());
        assert!(ctrl.is_stalled());
    }

    #[test]
    fn push_video_after_stall_recovers() {
        let mut ctrl = make_controller(8, 20_000, 100_000);

        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        advance_clock_us(&mut ctrl.clock, 100_000);
        ctrl.tick();

        // Stall.
        advance_clock_us(&mut ctrl.clock, 200_000);
        assert!(ctrl.tick().is_none());
        assert!(ctrl.is_stalled());

        // Push new frame after stall.
        ctrl.push_video(VideoFrame {
            pts_us: 400_000,
            handle: 1,
        });
        assert!(!ctrl.is_stalled());

        // Tick should handle the frame normally.
        let result = ctrl.tick();
        assert!(result.is_some());
    }

    #[test]
    fn burst_cooldown_resets_if_clock_advances_enough() {
        let mut ctrl = make_controller(8, 20_000, 500_000);

        // Push late frames to trigger burst.
        for i in 0..6 {
            ctrl.push_video(VideoFrame {
                pts_us: i * 10_000,
                handle: i as u64,
            });
        }
        advance_clock_us(&mut ctrl.clock, 200_000);

        // Trigger and exhaust first burst.
        let mut drops = 0;
        while ctrl.burst_drop_count < ctrl.catch_up.max_burst_drops {
            match ctrl.tick() {
                Some((FrameAction::Drop, _)) => drops += 1,
                _ => break,
            }
        }
        assert_eq!(drops, ctrl.catch_up.max_burst_drops);

        // Now advance clock beyond cooldown period.
        advance_clock_us(&mut ctrl.clock, ctrl.catch_up.burst_cooldown_us as u64 + 1);

        // Push more late frames. A new burst should be allowed.
        ctrl.push_video(VideoFrame {
            pts_us: 100_000,
            handle: 100,
        });

        // Head frame PTS 100_000 vs clock ~200_000+200_000+1 = ~400_001.
        // Behind by ~300_000 > 50_000 threshold → new burst allowed.
        let (action, _) = ctrl.tick().unwrap();
        assert_eq!(action, FrameAction::Drop);
        assert_eq!(ctrl.burst_drop_count, 1);
    }
}
