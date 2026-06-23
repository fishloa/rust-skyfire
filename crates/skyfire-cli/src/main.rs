//! `skyfire` — native debug CLI. Feed a captured MPEG-TS file; print a PID
//! histogram and flag AC-3/E-AC-3 PES. A native harness for the demux/decode
//! crates that mirrors what the browser receiver does on the raw TS.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;
use serde::Serialize;

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

fn build_histogram(data: &[u8]) -> BTreeMap<u16, u64> {
    let mut hist: BTreeMap<u16, u64> = BTreeMap::new();
    for chunk in data.chunks_exact(skyfire_ts::TS_PACKET_LEN) {
        if let Some(pid) = skyfire_ts::packet_pid(chunk) {
            *hist.entry(pid).or_default() += 1;
        }
    }
    hist
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
    match skyfire_ts::probe(data) {
        Some(map) => {
            println!(
                "Channel map: video PID {vp:#06x} ({vc:?})",
                vp = map.video_pid,
                vc = map.video_codec,
            );
            for a in &map.audio_streams {
                println!(
                    "  audio PID {pid:#06x} ({codec:?})",
                    pid = a.pid,
                    codec = a.codec,
                );
            }
        }
        None => {
            eprintln!("error: no PAT/PMT channel map found in input");
            std::process::exit(1);
        }
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

    let channel_map = skyfire_ts::probe(data).map(|map| ChannelMapJson {
        video_pid: map.video_pid,
        video_codec: format!("{:?}", map.video_codec),
        audio_streams: map
            .audio_streams
            .iter()
            .map(|a| AudioStreamJson {
                pid: a.pid,
                codec: format!("{:?}", a.codec),
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

fn main() {
    let args = Args::parse();

    let path = args.file.to_string_lossy().to_string();
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {path}: {e}");
            std::process::exit(1);
        }
    };

    let hist = build_histogram(&data);

    if args.json {
        print_json(&path, &data, &hist);
    } else {
        print_text(&path, &data, &hist);
    }
}
