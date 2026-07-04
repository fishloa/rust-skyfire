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
pub const fn decide(frame_pts_us: i64, clock_us: i64, tol_us: i64) -> FrameAction {
    if frame_pts_us < clock_us - tol_us {
        FrameAction::Drop
    } else if frame_pts_us > clock_us + tol_us {
        FrameAction::Hold
    } else {
        FrameAction::Present
    }
}

/// Abstract video frame handle with its PTS in microseconds.
///
/// The handle is an opaque integer (e.g. a pool index, a pointer-offset,
/// or a generation counter) that lets callers map back to their own
/// frame storage.  The queue does not store the actual frame data — it
/// only manages the metadata needed for presentation ordering and
/// sync decisions against the audio clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFrame {
    /// Presentation timestamp in microseconds (monotonic, unwrapped).
    pub pts_us: i64,
    /// Opaque handle/index that the caller uses to locate the real frame data.
    pub handle: u64,
}

/// PTS-ordered video-frame present queue.
///
/// Frames are pushed in **decode order** (PTS not necessarily monotonic —
/// B-frames arrive out of order) and popped in **PTS order**. Each pop
/// drives a [`decide`] call against the current audio-clock time with a
/// configurable tolerance.
///
/// # Drop policy
///
/// Late frames (PTS more than `drop_late_us` behind the clock) are
/// dropped automatically. This prevents unbounded queue growth when
/// decoding runs ahead of the clock.
///
/// # Capacity
///
/// The queue has a fixed capacity. Pushing beyond capacity is a no-op;
/// callers should check [`is_full`] before pushing.
#[derive(Debug, Clone)]
pub struct VideoFrameQueue {
    /// PTS-ordered heap of pending frames (min-heap on `pts_us`).
    buf: Vec<VideoFrame>,
    /// Maximum number of frames the queue can hold.
    capacity: usize,
    /// Half-frame tolerance (µs) passed to [`decide`].
    tol_us: i64,
    /// Frames with `pts_us < clock_us - drop_late_us` are dropped.
    drop_late_us: i64,
    /// Lag tracking: number of frames dropped because they were late.
    dropped_late_count: u64,
    /// Lag tracking: number of frames dropped because the queue was full.
    pub dropped_full_count: u64,
    /// Number of frames presented (popped via [`Present`]).
    pub presented_count: u64,
}

impl VideoFrameQueue {
    /// Create a new queue.
    ///
    /// `tol_us` is the half-frame tolerance passed to [`decide`]; typical
    /// values are ~10–20 ms (half a frame at 25/50 fps).
    ///
    /// `drop_late_us` controls how far behind the clock a frame must be
    /// before it is dropped. A good starting point is 50–100 ms.
    ///
    /// `capacity` caps the number of stored frames. B-frames typically
    /// mean ~16 frames in-flight for a GOP; 32 is a safe default.
    #[must_use]
    pub fn new(capacity: usize, tol_us: i64, drop_late_us: i64) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            capacity,
            tol_us,
            drop_late_us,
            dropped_late_count: 0,
            dropped_full_count: 0,
            presented_count: 0,
        }
    }

    /// Number of frames currently in the queue.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Whether the queue is at capacity.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.buf.len() >= self.capacity
    }

    /// Push a frame into the queue in PTS order.
    ///
    /// If the queue is full the frame is silently dropped (incrementing
    /// `dropped_full_count`). Otherwise the frame is inserted into a
    /// min-heap so that the next [`pop`] returns the frame with the
    /// smallest PTS.
    pub fn push(&mut self, frame: VideoFrame) {
        if self.buf.len() >= self.capacity {
            self.dropped_full_count = self.dropped_full_count.saturating_add(1);
            return;
        }
        // Insert at end then sift-up to maintain min-heap invariant.
        self.buf.push(frame);
        self.sift_up(self.buf.len() - 1);
    }

    /// Peek at the frame with the smallest PTS without removing it.
    #[must_use]
    pub fn peek(&self) -> Option<VideoFrame> {
        self.buf.first().copied()
    }

    /// Evaluate the next frame against the audio clock.
    ///
    /// Returns:
    /// - `FrameAction::Present` + the frame when its PTS is within tolerance
    ///   of the clock. The frame is removed from the queue.
    /// - `FrameAction::Drop` when the head frame is too late; the frame is
    ///   removed and `dropped_late_count` is incremented.
    /// - `FrameAction::Hold` + the head frame's PTS (without removing it)
    ///   when the next frame is still ahead of the clock.
    /// - `None` when the queue is empty.
    ///
    /// Late-frames that are behind the clock by more than `drop_late_us`
    /// are dropped even if `decide` would otherwise `Present` them — this
    /// keeps the queue from falling behind permanently.
    pub fn pop(&mut self, clock_us: i64) -> Option<(FrameAction, VideoFrame)> {
        loop {
            let head = self.peek()?;
            if head.pts_us < clock_us - self.drop_late_us {
                // Frame is too far behind — drop it.
                self.pop_head_inner();
                self.dropped_late_count = self.dropped_late_count.saturating_add(1);
                continue;
            }
            let action = decide(head.pts_us, clock_us, self.tol_us);
            match action {
                FrameAction::Present => {
                    self.presented_count = self.presented_count.saturating_add(1);
                    return Some((FrameAction::Present, self.pop_head_inner_unchecked()));
                }
                FrameAction::Drop => {
                    self.dropped_late_count = self.dropped_late_count.saturating_add(1);
                    self.pop_head_inner();
                    continue;
                }
                FrameAction::Hold => return Some((FrameAction::Hold, head)),
            }
        }
    }

    /// Drain all frames, dropping them as late.
    ///
    /// Returns the number of frames drained.
    pub fn drain(&mut self) -> usize {
        let n = self.buf.len();
        self.dropped_late_count = self.dropped_late_count.saturating_add(n as u64);
        self.buf.clear();
        n
    }

    /// Current half-frame tolerance (µs).
    #[must_use]
    pub const fn tol_us(&self) -> i64 {
        self.tol_us
    }

    /// Maximum age behind the clock before forced drop (µs).
    #[must_use]
    pub const fn drop_late_us(&self) -> i64 {
        self.drop_late_us
    }

    /// Set the tolerance for future [`pop`] calls.
    pub const fn set_tol_us(&mut self, tol_us: i64) {
        self.tol_us = tol_us;
    }

    /// Set the late-drop threshold for future [`pop`] calls.
    pub const fn set_drop_late_us(&mut self, drop_late_us: i64) {
        self.drop_late_us = drop_late_us;
    }

    /// Pop the head frame and return it, counting it as a late drop.
    pub(crate) fn pop_head_counted(&mut self) -> Option<VideoFrame> {
        let frame = self.pop_head_inner()?;
        self.dropped_late_count = self.dropped_late_count.saturating_add(1);
        Some(frame)
    }

    // ── private heap helpers ──────────────────────────────────────

    fn pop_head_inner(&mut self) -> Option<VideoFrame> {
        if self.buf.is_empty() {
            return None;
        }
        if self.buf.len() == 1 {
            return self.buf.pop();
        }
        let last = self.buf.pop().unwrap();
        let head = std::mem::replace(&mut self.buf[0], last);
        self.sift_down(0);
        Some(head)
    }

    fn pop_head_inner_unchecked(&mut self) -> VideoFrame {
        debug_assert!(!self.buf.is_empty());
        if self.buf.len() == 1 {
            return self.buf.pop().unwrap();
        }
        let last = self.buf.pop().unwrap();
        let head = std::mem::replace(&mut self.buf[0], last);
        self.sift_down(0);
        head
    }

    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) >> 1;
            if self.buf[idx].pts_us >= self.buf[parent].pts_us {
                break;
            }
            self.buf.swap(idx, parent);
            idx = parent;
        }
    }

    fn sift_down(&mut self, mut idx: usize) {
        let len = self.buf.len();
        loop {
            let left = (idx << 1) + 1;
            if left >= len {
                break;
            }
            let right = left + 1;
            let mut smallest = left;
            if right < len && self.buf[right].pts_us < self.buf[left].pts_us {
                smallest = right;
            }
            if self.buf[idx].pts_us <= self.buf[smallest].pts_us {
                break;
            }
            self.buf.swap(idx, smallest);
            idx = smallest;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_decisions() {
        assert_eq!(decide(1_000_000, 1_000_000, 20_000), FrameAction::Present);
        assert_eq!(decide(900_000, 1_000_000, 20_000), FrameAction::Drop);
        assert_eq!(decide(1_100_000, 1_000_000, 20_000), FrameAction::Hold);
    }

    #[test]
    fn queue_empty_returns_none() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        assert!(q.is_empty());
        assert_eq!(q.pop(0), None);
    }

    #[test]
    fn queue_present_in_order() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        q.push(VideoFrame {
            pts_us: 300_000,
            handle: 2,
        });

        // Clock at 90_000: frame 0 is within tolerance → Present.
        let (action, frame) = q.pop(90_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 0);

        // Clock at 200_000: frame 1 is Present.
        let (action, frame) = q.pop(200_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 1);

        // Clock at 210_000: frame 2 is still ahead → Hold.
        let (action, frame) = q.pop(210_000).unwrap();
        assert_eq!(action, FrameAction::Hold);
        assert_eq!(frame.handle, 2);
    }

    #[test]
    fn queue_reorders_out_of_order_input() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        // B-frame reorder: decode order is 0, 2, 1, 4, 3
        q.push(VideoFrame {
            pts_us: 0,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 80_000,
            handle: 2,
        }); // out of order
        q.push(VideoFrame {
            pts_us: 40_000,
            handle: 1,
        }); // out of order
        q.push(VideoFrame {
            pts_us: 160_000,
            handle: 4,
        }); // out of order
        q.push(VideoFrame {
            pts_us: 120_000,
            handle: 3,
        }); // out of order

        // Clock at 0: frame 0 → Present
        let (action, frame) = q.pop(0).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 0);

        // Clock at 40_000: frame 1 → Present
        let (action, frame) = q.pop(40_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 1);

        // Clock at 80_000: frame 2 → Present
        let (action, frame) = q.pop(80_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 2);

        // Clock at 120_000: frame 3 → Present
        let (action, frame) = q.pop(120_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(frame.handle, 3);

        // Clock at 130_000: frame 4 (PTS 160k) is 30k ahead > 20k tol → Hold.
        let (action, frame) = q.pop(130_000).unwrap();
        assert_eq!(action, FrameAction::Hold);
        assert_eq!(frame.handle, 4);
    }

    #[test]
    fn queue_drops_late_frames() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 50_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 80_000,
            handle: 1,
        });
        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 2,
        });

        // Clock at 300_000: frames 0 and 1 are >100ms behind → dropped.
        // Frame 2 is also late (200k vs 300k) but within drop_late_us?
        // 300_000 - 200_000 = 100_000 — exactly at threshold.
        // decide: 200_000 < 300_000 - 20_000 = 280_000 → Drop.
        // But drop_late check: 200_000 < 300_000 - 100_000 = 200_000? No (not <).
        // So decide returns Drop (frame is late), which also pops + counts.
        let result = q.pop(300_000);
        // All 3 frames should be consumed by the loop.
        assert!(result.is_none());
        assert!(q.is_empty());
        assert!(q.dropped_late_count >= 2);
    }

    #[test]
    fn queue_tolerance_prevents_thrash() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        // Frame PTS jitters ±10 ms around a clock that advances exactly.
        q.push(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });

        // Clock at 95_000: 100k is within ±20k → Present.
        assert_eq!(q.pop(95_000).unwrap().0, FrameAction::Present);

        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        // Clock at 215_000: 200k is 15k behind → still within tol → Present.
        assert_eq!(q.pop(215_000).unwrap().0, FrameAction::Present);

        q.push(VideoFrame {
            pts_us: 300_000,
            handle: 2,
        });
        // Clock at 285_000: 300k is 15k ahead → still within tol → Present.
        assert_eq!(q.pop(285_000).unwrap().0, FrameAction::Present);
    }

    #[test]
    fn queue_hold_when_ahead() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 500_000,
            handle: 0,
        });

        // Clock at 400_000: frame is 100k ahead > 20k tol → Hold.
        let (action, frame) = q.pop(400_000).unwrap();
        assert_eq!(action, FrameAction::Hold);
        assert_eq!(frame.handle, 0);
        assert_eq!(q.len(), 1); // frame not consumed

        // Clock catches up: 490_000, within 20k → Present.
        let (action, _) = q.pop(490_000).unwrap();
        assert_eq!(action, FrameAction::Present);
    }

    #[test]
    fn queue_full_drops_push() {
        let mut q = VideoFrameQueue::new(2, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        assert!(q.is_full());

        q.push(VideoFrame {
            pts_us: 300_000,
            handle: 2,
        });
        assert_eq!(q.dropped_full_count, 1);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn queue_drain_drops_all() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        assert_eq!(q.drain(), 2);
        assert!(q.is_empty());
        assert_eq!(q.dropped_late_count, 2);
    }

    #[test]
    fn queue_counters_increment() {
        let mut q = VideoFrameQueue::new(8, 20_000, 100_000);
        q.push(VideoFrame {
            pts_us: 100_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 200_000,
            handle: 1,
        });
        q.push(VideoFrame {
            pts_us: 50_000,
            handle: 2,
        }); // out of order, ends up first

        // Clock at 0: 50k is ahead → Hold on head (50k).
        let (action, _) = q.pop(0).unwrap();
        assert_eq!(action, FrameAction::Hold);

        // Clock at 50_000: 50k → Present.
        let (action, _) = q.pop(50_000).unwrap();
        assert_eq!(action, FrameAction::Present);
        assert_eq!(q.presented_count, 1);

        // Clock at 500_000: 100k and 200k are >100ms behind → dropped_late.
        let _ = q.pop(500_000);
        assert!(q.is_empty());
        assert!(q.dropped_late_count >= 2);
        assert_eq!(q.presented_count, 1);
    }

    // ── Integration test (epic #4 acceptance) ────────────────────

    /// Simulate an audio clock advancing in 10 ms steps (~480 samples at
    /// 48 kHz) while a stream of frames is pushed in decode order
    /// (non‑monotonic PTS). Assert that presented frames track the clock
    /// within tolerance and late frames are dropped.
    #[test]
    fn integration_simulated_av_sync() {
        let mut q = VideoFrameQueue::new(64, 20_000, 100_000);
        let mut clock = crate::audio_clock::AudioClock::new(0, 48_000);
        // Advance clock at same pace as frame spacing: 40 ms = 1920 samples at 48 kHz.
        let samples_per_step: u64 = 1920;

        // Build 50 frames at nominal 40 ms spacing (25 fps), pushed in
        // decode order with B-frame reorder (GOP=3: I B B).
        let display_order: Vec<i64> = (0..50).map(|i| i * 40_000).collect();

        let mut decode_order: Vec<usize> = Vec::new();
        let mut i = 0usize;
        while i < 50 {
            decode_order.push(i);
            for b in 1..=2 {
                let bi = i + b;
                if bi < 50 {
                    decode_order.push(bi);
                }
            }
            i += 3;
        }

        // Push all frames before starting playback (simulates a decode
        // burst ahead of the clock).
        for &display_idx in &decode_order {
            q.push(VideoFrame {
                pts_us: display_order[display_idx],
                handle: display_idx as u64,
            });
        }
        assert_eq!(q.len(), 50, "all 50 frames should be queued");

        // Now simulate playback: advance clock and present frames.
        let mut presented: Vec<u64> = Vec::new();

        // First, pop frames at clock=0 (frame with PTS=0).
        loop {
            match q.pop(crate::audio_clock::AudioClock::media_time_us(&clock)) {
                Some((FrameAction::Present, f)) => {
                    presented.push(f.handle);
                }
                Some((FrameAction::Drop, _)) => {}
                Some((FrameAction::Hold, _)) => break,
                None => break,
            }
        }

        for _step in 0..100 {
            let _ = clock.advance(samples_per_step);

            loop {
                let clock_us = clock.media_time_us();
                match q.pop(clock_us) {
                    Some((FrameAction::Present, f)) => {
                        let diff = (f.pts_us - clock_us).abs();
                        assert!(
                            diff <= q.tol_us(),
                            "presented frame PTS {} too far from clock {} (diff={diff})",
                            f.pts_us,
                            clock_us
                        );
                        presented.push(f.handle);
                    }
                    Some((FrameAction::Drop, f)) => {
                        assert!(
                            f.pts_us < clock_us - q.tol_us(),
                            "dropped frame PTS {} should be behind clock {}",
                            f.pts_us,
                            clock_us
                        );
                    }
                    Some((FrameAction::Hold, _)) => break,
                    None => break,
                }
            }

            if presented.len() >= 50 && q.is_empty() {
                break;
            }
        }

        assert_eq!(
            presented.len(),
            50,
            "expected 50 presented, got {} (dropped_late={}, dropped_full={})",
            presented.len(),
            q.dropped_late_count,
            q.dropped_full_count
        );

        let mut sorted = presented.clone();
        sorted.sort_unstable();
        assert_eq!(presented, sorted, "presented frames must be in PTS order");
    }

    #[test]
    fn integration_late_frames_dropped_when_clock_runs_ahead() {
        let mut q = VideoFrameQueue::new(32, 20_000, 100_000);
        let mut clock = crate::audio_clock::AudioClock::new(0, 48_000);

        q.push(VideoFrame {
            pts_us: 40_000,
            handle: 0,
        });
        q.push(VideoFrame {
            pts_us: 80_000,
            handle: 1,
        });
        q.push(VideoFrame {
            pts_us: 120_000,
            handle: 2,
        });

        // Advance clock far ahead — all frames are well behind.
        // 500_000 us at 48 kHz = 24_000 samples.
        let _ = clock.advance(24_000);
        assert!(clock.media_time_us() > 450_000);

        loop {
            let clock_us = clock.media_time_us();
            match q.pop(clock_us) {
                Some((FrameAction::Present, _)) => {
                    panic!("no frame should present when clock is far ahead");
                }
                Some((FrameAction::Drop, _)) => { /* expected */ }
                Some((FrameAction::Hold, _)) => break,
                None => break,
            }
        }

        assert!(q.is_empty());
        assert_eq!(q.dropped_late_count, 3);
    }
}
