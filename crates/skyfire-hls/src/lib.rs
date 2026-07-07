//! Source-agnostic HLS-of-TS orchestration. Feed raw MPEG-TS bytes; poll a
//! rolling or VOD playlist plus keyframe-aligned `.ts` segments. Wraps
//! `transmux::ts_hls::StreamingTsHlsSegmenter` (the chop) with the
//! build-on-`TracksResolved` + RAP-trim orchestration; owns no HTTP, no async.

use std::collections::VecDeque;
use std::sync::Arc;

use skyfire_ts::TsDemux;
use transmux::ts_hls::StreamingTsHlsSegmenter;

/// How a session segments and windows.
#[derive(Debug, Clone)]
pub struct HlsConfig {
    /// Target segment duration in seconds (segments cut on the first video RAP
    /// at/after this). zenith uses 4.
    pub target_secs: u32,
    /// `None` = VOD (retain every segment; append `#EXT-X-ENDLIST` on `finish`).
    /// `Some(n)` = rolling media playlist of at most `n` segments.
    pub window: Option<usize>,
    /// Segment filename prefix; segment `k` is `"{uri_prefix}{k}.ts"`.
    pub uri_prefix: String,
}

impl HlsConfig {
    #[must_use]
    pub fn vod() -> Self {
        Self {
            target_secs: 4,
            window: None,
            uri_prefix: "seg".to_string(),
        }
    }
    #[must_use]
    pub fn rolling(window: usize) -> Self {
        Self {
            target_secs: 4,
            window: Some(window.max(1)),
            uri_prefix: "seg".to_string(),
        }
    }
}

/// A committed segment retained for serving + playlist generation.
#[derive(Clone)]
pub struct StoredSegment {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub duration: f64,
    pub discontinuous: bool,
}

/// Incremental HLS-of-TS session. See crate docs.
#[allow(dead_code)]
pub struct HlsSession {
    cfg: HlsConfig,
    demux: TsDemux,
    seg: Option<StreamingTsHlsSegmenter>,
    // Track specs collected from TrackAdded, in arrival order.
    pending_specs: Vec<transmux::TrackSpec>,
    known_track_ids: Vec<u32>,
    video_track_id: Option<u32>,
    tracks_resolved: bool,
    // Samples buffered before the segmenter is built, in arrival order.
    buffer: Vec<(u32, transmux::pipeline::Sample)>,
    buffer_capped: bool,
    // Committed segments (retained fully for VOD; trimmed to window for rolling).
    segments: VecDeque<StoredSegment>,
    next_seq: u64,
    media_sequence: u64, // first retained segment's sequence (rolling eviction)
    finished: bool,
}

/// Upper bound on samples buffered while waiting for the first video RAP /
/// TracksResolved — prevents unbounded growth on a stream that never resolves.
#[allow(dead_code)]
const MAX_PREBUFFER_SAMPLES: usize = 4096;

impl HlsSession {
    #[must_use]
    pub fn new(cfg: HlsConfig) -> Self {
        Self {
            cfg,
            demux: TsDemux::new(),
            seg: None,
            pending_specs: Vec::new(),
            known_track_ids: Vec::new(),
            video_track_id: None,
            tracks_resolved: false,
            buffer: Vec::new(),
            buffer_capped: false,
            segments: VecDeque::new(),
            next_seq: 0,
            media_sequence: 0,
            finished: false,
        }
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.segments.is_empty()
    }

    #[must_use]
    pub fn segment(&self, name: &str) -> Option<Arc<Vec<u8>>> {
        self.segments
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.bytes.clone())
    }

    pub fn feed(&mut self, _data: &[u8]) {
        // Implemented in Task 2.
    }

    pub fn finish(&mut self) {
        self.finished = true;
        // Flush implemented in Task 2.
    }

    #[must_use]
    pub fn playlist(&self) -> String {
        // Real generation in Task 2; skeleton returns just the tag so Task 1's
        // test (header present, no segments) passes.
        let mut out = String::from("#EXTM3U\n");
        out.push_str("#EXT-X-VERSION:3\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_vod_session_is_not_ready_and_has_no_segments() {
        let s = HlsSession::new(HlsConfig::vod());
        assert!(!s.is_ready(), "no segments fed yet");
        assert_eq!(s.segment("seg0.ts"), None);
        // Playlist before any segment: a header, no segment lines, no ENDLIST yet.
        let pl = s.playlist();
        assert!(
            pl.starts_with("#EXTM3U"),
            "playlist must start with #EXTM3U"
        );
        assert!(!pl.contains(".ts"), "no segment URIs before any segment");
    }
}
