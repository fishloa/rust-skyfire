use broadcast_common::traits::Parse;
use skyfire_ts::{AudioCodec, DemuxEvent, SubtitleKind, TrackKind, TrackMeta};
use skyfire_ts::{audio_codec_str, video_codec_str};
use wasm_bindgen::prelude::*;

use crate::bridge_dto::{
    CachedVideoConfig, WasmAudioTrack, WasmMediaSegment, WasmPcmChunk, WasmSubtitleCue,
    WasmSubtitleRegion, WasmSubtitleTrack, WasmTrackList, WasmVideoAu,
};
use crate::helpers::{
    lang_bytes_to_string, parse_base_media_decode_time, parse_sample_count_from_segment,
};

/// Streaming WASM bridge between the browser and the Skyfire demux engine.
///
/// Unlike [`WasmEngine`](crate::probe::WasmEngine) (which requires probe→init→feed→finalize), this
/// struct is designed for real-time streaming:
///
/// 1. Construct with `SkyfireBridge::new()`.
/// 2. Call `feed(chunk)` repeatedly as TS data arrives over `fetch()`.
/// 3. Poll `track_list()` until it becomes `Some` (PAT+PMT have been parsed).
/// 4. Call `take_video_aus()` each tick to drain pending video access units.
/// 5. Use `pcr_pts()` for the A/V sync clock.
#[wasm_bindgen]
pub struct SkyfireBridge {
    demux: skyfire_ts::TsDemux,
    /// track_id → TrackMeta
    tracks: std::collections::HashMap<u32, TrackMeta>,
    /// Track ID of the first video track seen.
    video_track_id: Option<u32>,
    /// Cached WebCodecs video config.
    cached_video_config: Option<CachedVideoConfig>,
    /// Selected audio PID.
    selected_audio_pid: Option<u16>,
    /// Selected subtitle PID.
    selected_subtitle_pid: Option<u16>,
    /// Play/pause state.
    playing: bool,
    /// Accumulated video AUs (already AVCC length-prefixed from transmux).
    video_aus: Vec<WasmVideoAu>,
    /// MSE segmenter (lazy-built on first video TrackAdded).
    segmenter: Option<transmux::Segmenter>,
    /// Ready media segment bytes, pending drain.
    ready_segments: std::collections::VecDeque<Vec<u8>>,
    /// AC-3/E-AC-3 incremental decoder.
    audio_decoder: skyfire_ac3::IncrementalDecoder,
    /// MPEG-1/2 Layer II incremental decoder.
    mpa_decoder: skyfire_mpa::IncrementalMpaDecoder,
    /// PCM chunks pending drain.
    audio_pcm_pending: Vec<WasmPcmChunk>,
    /// Subtitle compositor.
    subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState,
    /// Subtitle cues pending drain.
    subtitle_cues_pending: Vec<WasmSubtitleCue>,
    /// Latest PCR/PTS value in 90 kHz ticks.
    latest_pcr: Option<i64>,
    /// Whether the stream has ended (flush() was called).
    ended: bool,
    /// When true (default), downmix multichannel to stereo.
    downmix_audio: bool,
    /// Native channel count of last decoded audio (before downmix).
    last_audio_channels: u16,
    /// Number of audio decode errors since construction (JS-observable).
    audio_decode_error_count: u64,
    /// Number of segmenter errors since construction (JS-observable).
    segmenter_error_count: u64,
}

#[wasm_bindgen]
impl SkyfireBridge {
    /// Create a new, empty bridge.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        let audio_decoder = skyfire_ac3::IncrementalDecoder::new()
            .unwrap_or_else(|e| wasm_bindgen::throw_str(&format!("build ac3 decoder: {e}")));
        Self {
            demux: skyfire_ts::TsDemux::new(),
            tracks: std::collections::HashMap::new(),
            video_track_id: None,
            cached_video_config: None,
            selected_audio_pid: None,
            selected_subtitle_pid: None,
            playing: false,
            video_aus: Vec::new(),
            segmenter: None,
            ready_segments: std::collections::VecDeque::new(),
            audio_decoder,
            mpa_decoder: skyfire_mpa::IncrementalMpaDecoder::new(),
            audio_pcm_pending: Vec::new(),
            subtitle_compositor: skyfire_ts::subtitle_compositor::CompositorState::new(),
            subtitle_cues_pending: Vec::new(),
            latest_pcr: None,
            ended: false,
            downmix_audio: true,
            last_audio_channels: 0,
            audio_decode_error_count: 0,
            segmenter_error_count: 0,
        }
    }

    /// Native channel count of the most recently decoded audio (before any downmix), or 0 if none yet.
    #[wasm_bindgen]
    #[must_use]
    pub fn audio_native_channels(&self) -> u16 {
        self.last_audio_channels
    }

    /// Enable (default) or disable the WASM stereo downmix.
    #[wasm_bindgen]
    pub fn set_audio_downmix(&mut self, enabled: bool) {
        self.downmix_audio = enabled;
    }

    /// Number of audio decode errors since construction.
    #[wasm_bindgen]
    #[must_use]
    pub fn audio_decode_error_count(&self) -> u64 {
        self.audio_decode_error_count
    }

    /// Number of segmenter errors since construction.
    #[wasm_bindgen]
    #[must_use]
    pub fn segmenter_error_count(&self) -> u64 {
        self.segmenter_error_count
    }

    /// Push a raw TS chunk into the bridge.
    #[wasm_bindgen]
    pub fn feed(&mut self, bytes: &[u8]) {
        self.demux.feed(bytes);
        self.drain_events();
    }

    /// Select which audio PID to route and decode.
    #[wasm_bindgen]
    pub fn select_audio(&mut self, pid: u16) {
        if self.selected_audio_pid != Some(pid) {
            self.audio_decoder.reset();
            self.mpa_decoder.reset();
        }
        self.selected_audio_pid = Some(pid);
    }

    /// Select a subtitle PID, or `None` to disable subtitles.
    #[wasm_bindgen]
    pub fn select_subtitle(&mut self, pid: Option<u16>) {
        if self.selected_subtitle_pid != pid {
            self.subtitle_cues_pending.clear();
        }
        self.selected_subtitle_pid = pid;
    }

    /// Set the play/pause state.
    #[wasm_bindgen]
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Returns the track list once at least one video track has been seen, or `None`.
    #[wasm_bindgen]
    pub fn track_list(&self) -> Option<WasmTrackList> {
        let video_id = self.video_track_id?;
        let video_meta = self.tracks.get(&video_id)?;
        let video_pid = video_meta.pid.unwrap_or(0);
        let video_codec = match video_meta.kind {
            TrackKind::Video(c) => video_codec_str(c),
            _ => "H264",
        };

        let mut audio: Vec<WasmAudioTrack> = self
            .tracks
            .values()
            .filter(|m| matches!(m.kind, TrackKind::Audio(_)))
            .map(|m| WasmAudioTrack {
                pid: m.pid.unwrap_or(0),
                codec: match m.kind {
                    TrackKind::Audio(c) => audio_codec_str(c).to_string(),
                    _ => "EAC3".to_string(),
                },
                language: m.language.map(|l| lang_bytes_to_string(&l)),
            })
            .collect();
        audio.sort_by_key(|a| a.pid);

        let mut subtitles: Vec<WasmSubtitleTrack> = self
            .tracks
            .values()
            .filter(|m| matches!(m.kind, TrackKind::Subtitle(_)))
            .map(|m| WasmSubtitleTrack {
                pid: m.pid.unwrap_or(0),
                kind: match m.kind {
                    TrackKind::Subtitle(SubtitleKind::DvbSubtitles) => "DvbSubtitles",
                    TrackKind::Subtitle(SubtitleKind::Teletext) => "Teletext",
                    _ => "DvbSubtitles",
                }
                .to_string(),
                language: m.language.map(|l| lang_bytes_to_string(&l)),
            })
            .collect();
        subtitles.sort_by_key(|s| s.pid);

        Some(WasmTrackList {
            video_pid,
            video_codec: video_codec.to_string(),
            audio,
            subtitles,
        })
    }

    /// CMAF initialization segment (`ftyp` + `moov`). Empty until a video track has been seen.
    #[wasm_bindgen]
    pub fn video_init_segment(&self) -> Vec<u8> {
        self.segmenter
            .as_ref()
            .and_then(|s| s.init_segment().ok())
            .unwrap_or_default()
    }

    /// WebCodecs codec string (e.g. `"avc1.640028"`), or `None` if not yet available.
    #[wasm_bindgen]
    pub fn video_codec(&self) -> Option<String> {
        self.cached_video_config.as_ref().map(|c| c.codec.clone())
    }

    /// WebCodecs `avcC` description bytes, or empty if not yet available.
    #[wasm_bindgen]
    pub fn video_config_description(&self) -> Vec<u8> {
        self.cached_video_config
            .as_ref()
            .map(|c| c.description.clone())
            .unwrap_or_default()
    }

    /// Drain all completed video access units since the last call.
    #[wasm_bindgen]
    pub fn take_video_aus(&mut self) -> Vec<WasmVideoAu> {
        std::mem::take(&mut self.video_aus)
    }

    /// Drain the next complete media segment. Returns `None` until a full segment is ready.
    #[wasm_bindgen]
    pub fn take_video_media_segment(&mut self) -> Option<WasmMediaSegment> {
        if let Some(ref mut seg) = self.segmenter {
            for bytes in seg.take_ready() {
                self.ready_segments.push_back(bytes);
            }
        }
        let bytes = self.ready_segments.pop_front()?;
        let sample_count = parse_sample_count_from_segment(&bytes);
        let base_media_decode_time = parse_base_media_decode_time(&bytes);
        Some(WasmMediaSegment {
            base_media_decode_time,
            bytes,
            sample_count,
        })
    }

    /// Drain all decoded PCM chunks produced since the last call.
    #[wasm_bindgen]
    pub fn take_audio_pcm(&mut self) -> Vec<WasmPcmChunk> {
        std::mem::take(&mut self.audio_pcm_pending)
    }

    /// Drain all composited subtitle cues since the last call.
    #[wasm_bindgen]
    pub fn take_subtitle_cues(&mut self) -> Vec<WasmSubtitleCue> {
        for cue in self.subtitle_compositor.take_cues() {
            let regions = cue
                .regions
                .into_iter()
                .map(|r| WasmSubtitleRegion {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                    rgba: r.rgba,
                })
                .collect();
            self.subtitle_cues_pending.push(WasmSubtitleCue {
                start_pts: cue.start_pts,
                end_pts: cue.end_pts,
                regions,
            });
        }
        std::mem::take(&mut self.subtitle_cues_pending)
    }

    /// Latest PCR-derived clock value in 90 kHz ticks.
    #[wasm_bindgen]
    pub fn pcr_pts(&self) -> Option<i64> {
        self.latest_pcr
    }

    /// Signal end-of-stream: flush any partial access units and emit the final segment.
    #[wasm_bindgen]
    pub fn flush(&mut self) {
        self.demux.finish();
        self.drain_events();
        if let Some(ref mut seg) = self.segmenter
            && let Err(e) = seg.flush()
        {
            self.segmenter_error_count += 1;
            std::eprintln!("[skyfire-wasm] segmenter flush error: {e}");
        }
        self.ended = true;
    }

    // ── internal ────────────────────────────────────────────────────────────

    fn drain_events(&mut self) {
        while let Some(ev) = self.demux.poll_event() {
            match ev {
                DemuxEvent::TrackAdded(track) => self.on_track_added(track),
                DemuxEvent::TrackUpdated(track) => self.on_track_updated(track),
                DemuxEvent::Sample { track_id, sample } => self.on_sample(track_id, sample),
                DemuxEvent::Pcr(pcr) => {
                    // pcr_27mhz is 27 MHz; convert to 90 kHz by dividing by 300.
                    self.latest_pcr = Some((pcr.pcr_27mhz / 300) as i64);
                }
                DemuxEvent::Discontinuity { .. } => {
                    if let Some(ref mut seg) = self.segmenter {
                        seg.mark_discontinuity();
                    }
                    // Reset AC-3/E-AC-3 and MP2 decoder IMDCT state at HLS splices
                    // to avoid glitched PCM.  Mirror the same resets used by
                    // select_audio() on a PID change.
                    self.audio_decoder.reset();
                    self.mpa_decoder.reset();
                }
                _ => {}
            }
        }
    }

    fn on_track_added(&mut self, track: transmux::Track) {
        let meta = skyfire_ts::track_meta(&track.spec);
        let track_id = track.spec.track_id;

        if matches!(meta.kind, TrackKind::Video(_)) && self.video_track_id.is_none() {
            self.video_track_id = Some(track_id);

            if let transmux::CodecConfig::Avc { ref config, .. } = track.spec.config {
                let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
                self.cached_video_config = Some(CachedVideoConfig { codec, description });
            }

            let ts = transmux::TrackSpec::new(track_id, 90_000, track.spec.config.clone());
            if let Ok(seg) = transmux::Segmenter::new(vec![ts], 90_000, 2.0) {
                self.segmenter = Some(seg);
            }
        }

        if matches!(meta.kind, TrackKind::Audio(_))
            && self.selected_audio_pid.is_none()
            && let Some(pid) = meta.pid
        {
            self.selected_audio_pid = Some(pid);
        }

        self.tracks.insert(track_id, meta);
    }

    fn on_track_updated(&mut self, track: transmux::Track) {
        let meta = skyfire_ts::track_meta(&track.spec);
        // If the video track's config changed (e.g. in-band SPS update), rebuild
        // cached_video_config — mirrors what skyfire-core's Engine already does.
        if Some(track.spec.track_id) == self.video_track_id
            && let transmux::CodecConfig::Avc { ref config, .. } = track.spec.config
        {
            let (codec, description) = skyfire_ts::build_avcc_config(&config.config);
            self.cached_video_config = Some(CachedVideoConfig { codec, description });
        }
        self.tracks.insert(track.spec.track_id, meta);
    }

    fn on_sample(&mut self, track_id: u32, sample: transmux::Sample) {
        let meta = match self.tracks.get(&track_id).cloned() {
            Some(m) => m,
            None => return,
        };

        match meta.kind {
            TrackKind::Video(_) if Some(track_id) == self.video_track_id => {
                // Seed latest_pcr from video PTS only before the first real PCR
                // event arrives.  DemuxEvent::Pcr is the authoritative source and
                // must not be overwritten by every video sample.
                if self.latest_pcr.is_none()
                    && let Some(ref st) = sample.source_timing
                {
                    self.latest_pcr = Some(st.pts as i64);
                }
                let pts = sample.source_timing.as_ref().map(|t| t.pts);
                let dts = sample.source_timing.as_ref().map(|t| t.dts);
                // transmux 0.14 (rust-broadcast#595) sets is_sync on open-GOP
                // random-access points (IDR / recovery-point SEI / SPS-led AU),
                // so no client-side keyframe re-derivation is needed.
                self.video_aus.push(WasmVideoAu {
                    pts_ticks: pts,
                    dts_ticks: dts,
                    is_keyframe: sample.is_sync,
                    bytes: sample.data.clone(),
                });
                if let Some(ref mut seg) = self.segmenter
                    && let Err(e) = seg.push(track_id, sample)
                {
                    self.segmenter_error_count += 1;
                    std::eprintln!("[skyfire-wasm] segmenter push error: {e}");
                }
            }
            TrackKind::Audio(codec) if meta.pid == self.selected_audio_pid => {
                let pts_ticks = sample.source_timing.as_ref().map(|t| t.pts);
                self.decode_audio(codec, pts_ticks, &sample.data);
            }
            TrackKind::Subtitle(_) if meta.pid == self.selected_subtitle_pid => {
                let pid = meta.pid.unwrap_or(0);
                let pts_ticks = sample.source_timing.as_ref().map(|t| t.pts);
                if sample.data.first() == Some(&dvb_subtitle::DataIdentifier)
                    && let Ok(field) = dvb_subtitle::PesDataField::parse(&sample.data)
                {
                    self.subtitle_compositor.feed_pes(pid, pts_ticks, &field);
                }
            }
            _ => {}
        }
    }

    fn decode_audio(&mut self, codec: AudioCodec, pts_ticks: Option<u64>, data: &[u8]) {
        match codec {
            AudioCodec::Mp2 => match self.mpa_decoder.decode_au(data) {
                Ok(Some(decoded)) => {
                    let samples_f32: Vec<f32> = decoded
                        .pcm_s16le
                        .chunks_exact(2)
                        .map(|b| {
                            let s = i16::from_le_bytes([b[0], b[1]]);
                            f32::from(s) / 32_768.0_f32
                        })
                        .collect();
                    self.audio_pcm_pending.push(WasmPcmChunk {
                        pts_ticks,
                        sample_rate: decoded.sample_rate,
                        channels: decoded.channels,
                        samples: samples_f32,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    self.audio_decode_error_count += 1;
                    std::eprintln!("[skyfire-wasm] mp2 decode error: {e}");
                }
            },
            _ => match self.audio_decoder.decode_au(data) {
                Ok(Some(decoded)) if decoded.sample_rate > 0 && decoded.channels > 0 => {
                    self.last_audio_channels = decoded.channels;
                    let (channels, samples_f32) = if self.downmix_audio || decoded.channels <= 2 {
                        (
                            2u16,
                            skyfire_ac3::downmix::downmix_s16le_to_stereo_f32(
                                &decoded.pcm_s16le,
                                decoded.channels,
                            ),
                        )
                    } else {
                        let native = skyfire_ac3::downmix::s16le_slice_to_f32(&decoded.pcm_s16le);
                        (decoded.channels, native)
                    };
                    self.audio_pcm_pending.push(WasmPcmChunk {
                        pts_ticks,
                        sample_rate: decoded.sample_rate,
                        channels,
                        samples: samples_f32,
                    });
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => {
                    self.audio_decode_error_count += 1;
                    std::eprintln!("[skyfire-wasm] ac3/eac3 decode error: {e}");
                }
            },
        }
    }
}

impl Default for SkyfireBridge {
    fn default() -> Self {
        Self::new()
    }
}
