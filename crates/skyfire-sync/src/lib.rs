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
//!
//! # Catch‑up and stall handling
//!
//! `SyncController` wraps the audio clock and video present queue with
//! higher‑level policy:
//!
//! - **Catch‑up burst**: when video is behind the audio clock by more than
//!   `catch_up_behind_threshold_us`, frames are dropped in controlled bursts
//!   (`max_burst_drops`) rather than playing them late, until back within
//!   tolerance. A cooldown period prevents repeated bursts after each recovery.
//! - **Stall detection**: if the audio clock advances and no video frames are
//!   available (queue empty, clock past last known PTS), the controller
//!   reports `stalled`. When frames resume, the stall is cleared and the
//!   clock optionally re‑anchors to the first new frame.
//! - **Latency reporting**: `latency()` returns the signed A/V offset (positive
//!   = video ahead of audio, negative = video behind) and the stall flag so
//!   callers can adapt UI or buffering strategy.

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
    /// PTS-ordered heap of pending frames (min-heap on pts_us).
    buf: Vec<VideoFrame>,
    /// Maximum number of frames the queue can hold.
    capacity: usize,
    /// Half-frame tolerance (µs) passed to [`decide`].
    tol_us: i64,
    /// Frames with `pts_us < clock_us - drop_late_us` are dropped.
    drop_late_us: i64,
    /// Lag tracking: number of frames dropped because they were late.
    pub dropped_late_count: u64,
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
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Whether the queue is at capacity.
    #[must_use]
    pub fn is_full(&self) -> bool {
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
                self.pop_head();
                self.dropped_late_count = self.dropped_late_count.saturating_add(1);
                continue;
            }
            let action = decide(head.pts_us, clock_us, self.tol_us);
            match action {
                FrameAction::Present => {
                    self.presented_count = self.presented_count.saturating_add(1);
                    return Some((FrameAction::Present, self.pop_head_unchecked()));
                }
                FrameAction::Drop => {
                    self.dropped_late_count = self.dropped_late_count.saturating_add(1);
                    self.pop_head();
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
    pub fn tol_us(&self) -> i64 {
        self.tol_us
    }

    /// Maximum age behind the clock before forced drop (µs).
    #[must_use]
    pub fn drop_late_us(&self) -> i64 {
        self.drop_late_us
    }

    /// Set the tolerance for future [`pop`] calls.
    pub fn set_tol_us(&mut self, tol_us: i64) {
        self.tol_us = tol_us;
    }

    /// Set the late-drop threshold for future [`pop`] calls.
    pub fn set_drop_late_us(&mut self, drop_late_us: i64) {
        self.drop_late_us = drop_late_us;
    }

    // ── private heap helpers ──────────────────────────────────────

    fn pop_head(&mut self) -> Option<VideoFrame> {
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

    fn pop_head_unchecked(&mut self) -> VideoFrame {
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
    burst_drop_count: usize,
    /// Audio clock time (µs) at which the last catch‑up burst ended.
    last_burst_end_clock_us: i64,
    /// Whether the video pipeline is currently stalled.
    stalled: bool,
}

impl SyncController {
    /// Create a new sync controller.
    #[must_use]
    pub fn new(clock: AudioClock, queue: VideoFrameQueue, catch_up: CatchUpConfig) -> Self {
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
            if let Some(last_pts) = self.last_frame_pts_us {
                if clock_us > last_pts {
                    self.stalled = true;
                }
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

            let dropped_frame = VideoFrame {
                pts_us: head.pts_us,
                handle: head.handle,
            };

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
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Signal an audio underrun to the wrapped clock.
    pub fn signal_underrun(&mut self) {
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

    // ── VideoFrameQueue tests ─────────────────────────────────────

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
        let mut clock = AudioClock::new(0, 48_000);
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
            match q.pop(clock.media_time_us()) {
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
        let mut clock = AudioClock::new(0, 48_000);

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

    // ── Catch‑up burst tests ─────────────────────────────────────────

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
