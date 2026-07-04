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

mod audio_clock;
mod frame_queue;
mod pts;
mod sync_controller;

pub use audio_clock::AudioClock;
pub use frame_queue::{FrameAction, VideoFrame, VideoFrameQueue, decide};
pub use pts::{PTS_90KHZ, PTS_RANGE, pts_33_to_us, pts_delta_33};
pub use sync_controller::{CatchUpConfig, Latency, SyncController};
