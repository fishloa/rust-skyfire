//! `skyfire` — native debug CLI. Feed a captured MPEG-TS file; print a PID
//! histogram and flag AC-3/E-AC-3 PES. A native harness for the demux/decode
//! crates that mirrors what the browser receiver does on the raw TS.

use std::collections::BTreeMap;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: skyfire <file.ts>");
            std::process::exit(2);
        }
    };
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {path}: {e}");
            std::process::exit(1);
        }
    };

    let mut hist: BTreeMap<u16, u64> = BTreeMap::new();
    for chunk in data.chunks_exact(skyfire_ts::TS_PACKET_LEN) {
        if let Some(pid) = skyfire_ts::packet_pid(chunk) {
            *hist.entry(pid).or_default() += 1;
        }
    }
    println!(
        "{}: {} packets, {} distinct PIDs",
        path,
        data.len() / skyfire_ts::TS_PACKET_LEN,
        hist.len()
    );
    for (pid, n) in &hist {
        println!("  PID {pid:#06x}: {n}");
    }
}
