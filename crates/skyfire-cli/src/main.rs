//! `skyfire` — native debug CLI. Feed a captured MPEG-TS file; print a PID
//! histogram and flag AC-3/E-AC-3 PES. A native harness for the demux/decode
//! crates that mirrors what the browser receiver does on the raw TS.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;
use serde::Serialize;
use skyfire_ts::{
    AudioCodec, DemuxEvent, TrackKind, TsDemux, VideoCodec, audio_codec_str, track_meta,
    video_codec_str,
};
use transmux::pipeline::CodecConfig;

#[derive(Parser)]
#[command(
    name = "skyfire",
    version,
    about = "Inspect a captured MPEG-TS file — print PID histogram and channel map"
)]
struct Args {
    /// Path to the MPEG-TS file to inspect
    file: PathBuf,

    /// Output histogram and channel map as JSON
    #[arg(short = 'j', long)]
    json: bool,

    /// Output compact probe JSON (video, audio, subtitle tracks, langs, dims)
    #[arg(long)]
    probe: bool,

    /// Output DVB-subtitle page-composition timestamps (JSON array)
    #[arg(long = "sub-activity")]
    sub_activity: bool,
}

/// Serializable representation of the probe output for --json.
#[derive(Serialize)]
struct JsonOutput {
    path: String,
    total_packets: usize,
    distinct_pids: usize,
    pid_histogram: Vec<PidEntry>,
    channel_map: Option<ChannelMapJson>,
}

#[derive(Serialize)]
struct PidEntry {
    pid: u16,
    count: u64,
}

#[derive(Serialize)]
struct ChannelMapJson {
    video_pid: u16,
    video_codec: String,
    audio_streams: Vec<AudioStreamJson>,
}

#[derive(Serialize)]
struct AudioStreamJson {
    pid: u16,
    codec: String,
}

struct ProbeResult {
    video_pid: u16,
    video_codec: VideoCodec,
    audio_streams: Vec<(u16, AudioCodec)>,
}

fn build_histogram(data: &[u8]) -> BTreeMap<u16, u64> {
    let mut hist: BTreeMap<u16, u64> = BTreeMap::new();
    for chunk in data.chunks_exact(skyfire_ts::TS_PACKET_LEN) {
        if chunk[0] == 0x47 {
            let pid = u16::from_be_bytes([chunk[1] & 0x1f, chunk[2]]);
            *hist.entry(pid).or_default() += 1;
        }
    }
    hist
}

fn probe(data: &[u8]) -> Option<ProbeResult> {
    let mut demux = TsDemux::new();
    demux.feed(data);
    demux.finish();

    let mut video: Option<(u16, VideoCodec)> = None;
    let mut audio: Vec<(u16, AudioCodec)> = Vec::new();

    while let Some(ev) = demux.poll_event() {
        if let DemuxEvent::TrackAdded(track) = ev {
            let meta = track_meta(&track);
            let pid = meta.pid.unwrap_or(0);
            match meta.kind {
                TrackKind::Video(vc) if video.is_none() => {
                    video = Some((pid, vc));
                }
                TrackKind::Audio(ac) => {
                    audio.push((pid, ac));
                }
                _ => {}
            }
        }
    }

    let (video_pid, video_codec) = video?;
    Some(ProbeResult {
        video_pid,
        video_codec,
        audio_streams: audio,
    })
}

fn print_text(path: &str, data: &[u8], hist: &BTreeMap<u16, u64>) {
    println!(
        "{}: {} packets, {} distinct PIDs",
        path,
        data.len() / skyfire_ts::TS_PACKET_LEN,
        hist.len()
    );
    for (pid, n) in hist {
        println!("  PID {pid:#06x}: {n}");
    }

    println!();
    if let Some(map) = probe(data) {
        println!(
            "Channel map: video PID {vp:#06x} ({vc:?})",
            vp = map.video_pid,
            vc = map.video_codec,
        );
        for (pid, codec) in &map.audio_streams {
            println!(
                "  audio PID {pid:#06x} ({codec:?})",
                pid = pid,
                codec = codec,
            );
        }
    } else {
        eprintln!("error: no PAT/PMT channel map found in input");
        std::process::exit(1);
    }
}

fn print_json(path: &str, data: &[u8], hist: &BTreeMap<u16, u64>) {
    let total_packets = data.len() / skyfire_ts::TS_PACKET_LEN;
    let pid_histogram: Vec<PidEntry> = hist
        .iter()
        .map(|(pid, count)| PidEntry {
            pid: *pid,
            count: *count,
        })
        .collect();

    let channel_map = probe(data).map(|map| ChannelMapJson {
        video_pid: map.video_pid,
        video_codec: format!("{:?}", map.video_codec),
        audio_streams: map
            .audio_streams
            .iter()
            .map(|(pid, codec)| AudioStreamJson {
                pid: *pid,
                codec: format!("{codec:?}"),
            })
            .collect(),
    });

    let output = JsonOutput {
        path: path.to_string(),
        total_packets,
        distinct_pids: hist.len(),
        pid_histogram,
        channel_map,
    };

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

fn lang_str(m: &skyfire_ts::TrackMeta) -> Option<String> {
    m.language.map(|b| String::from_utf8_lossy(&b).to_string())
}

#[derive(Serialize)]
struct ProbeJson {
    video: Option<VideoJson>,
    audio: Vec<TrackJson>,
    subtitle: Vec<TrackJson>,
    default_audio_pid: Option<u16>,
}

#[derive(Serialize)]
struct VideoJson {
    codec: String,
    width: u16,
    height: u16,
}

#[derive(Serialize)]
struct TrackJson {
    pid: u16,
    codec: String,
    lang: Option<String>,
}

fn probe_full(data: &[u8]) -> ProbeJson {
    let mut demux = TsDemux::new();
    demux.feed(data);
    demux.finish();
    let mut video = None;
    let mut audio = Vec::new();
    let mut subtitle = Vec::new();
    let mut default_audio_pid = None;
    while let Some(ev) = demux.poll_event() {
        if let DemuxEvent::TrackAdded(track) = ev {
            let meta = track_meta(&track);
            let pid = meta.pid.unwrap_or(0);
            match meta.kind {
                TrackKind::Video(vc) if video.is_none() => {
                    let (width, height) = match &track.config {
                        CodecConfig::Avc { width, height, .. }
                        | CodecConfig::Hevc { width, height, .. } => (*width, *height),
                        _ => (0, 0),
                    };
                    video = Some(VideoJson {
                        codec: video_codec_str(vc).into(),
                        width,
                        height,
                    });
                }
                TrackKind::Audio(ac) => {
                    default_audio_pid.get_or_insert(pid);
                    audio.push(TrackJson {
                        pid,
                        codec: audio_codec_str(ac).into(),
                        lang: lang_str(&meta),
                    });
                }
                TrackKind::Subtitle(_) => {
                    subtitle.push(TrackJson {
                        pid,
                        codec: "DVBSUB".into(),
                        lang: lang_str(&meta),
                    });
                }
                _ => {}
            }
        }
    }
    ProbeJson {
        video,
        audio,
        subtitle,
        default_audio_pid,
    }
}

#[derive(Serialize)]
struct SubActivityJson {
    activity: Vec<SubActivity>,
}

#[derive(Serialize)]
struct SubActivity {
    pid: u16,
    pts_ticks: u64,
}

fn sub_activity(data: &[u8]) -> SubActivityJson {
    use skyfire_ts::TrackKind;
    let mut demux = TsDemux::new();
    demux.feed(data);
    demux.finish();
    // Map subtitle track_id → pid.
    let mut sub_ids: std::collections::HashMap<u32, u16> = std::collections::HashMap::new();
    let mut activity = Vec::new();
    while let Some(ev) = demux.poll_event() {
        match ev {
            DemuxEvent::TrackAdded(track) => {
                let meta = track_meta(&track);
                if matches!(meta.kind, TrackKind::Subtitle(_)) {
                    sub_ids.insert(track.track_id, meta.pid.unwrap_or(0));
                }
            }
            DemuxEvent::Sample {
                track_id, sample, ..
            } => {
                if let Some(&pid) = sub_ids.get(&track_id)
                    && payload_has_page_composition(&sample.data)
                    && let Some(pts_ticks) = skyfire_ts::checked_ticks(sample.pts)
                {
                    activity.push(SubActivity { pid, pts_ticks });
                }
            }
            _ => {}
        }
    }
    SubActivityJson { activity }
}

/// True if a DVB-subtitle PES payload contains a page-composition segment
/// (0x10) with a non-empty region list (ETSI EN 300 743 §7.2.2).
fn payload_has_page_composition(data: &[u8]) -> bool {
    // Expect data_identifier 0x20, subtitle_stream_id 0x00, then 0x0f-framed segments.
    if data.len() < 2 || data[0] != 0x20 || data[1] != 0x00 {
        return false;
    }
    let mut i = 2;
    while i + 6 <= data.len() && data[i] == 0x0f {
        let segment_type = data[i + 1];
        let segment_len = u16::from_be_bytes([data[i + 4], data[i + 5]]) as usize;
        // page composition (0x10) with a payload that includes at least one region.
        if segment_type == 0x10 && segment_len > 2 {
            return true;
        }
        i += 6 + segment_len;
    }
    false
}

fn main() {
    let args = Args::parse();
    let path = args.file.display().to_string();

    let data = match std::fs::read(&args.file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    if args.sub_activity {
        println!(
            "{}",
            serde_json::to_string_pretty(&sub_activity(&data)).unwrap()
        );
        return;
    }

    if args.probe {
        println!(
            "{}",
            serde_json::to_string_pretty(&probe_full(&data)).unwrap()
        );
        return;
    }

    let hist = build_histogram(&data);

    if args.json {
        print_json(&path, &data, &hist);
    } else {
        print_text(&path, &data, &hist);
    }
}
