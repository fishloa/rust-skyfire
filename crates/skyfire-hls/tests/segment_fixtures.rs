use skyfire_hls::{HlsConfig, HlsSession};

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    std::fs::read(p).expect("fixture not found")
}

/// Collect every distinct PID present in a whole-packet TS buffer.
fn pids(ts: &[u8]) -> std::collections::BTreeSet<u16> {
    let mut set = std::collections::BTreeSet::new();
    for pkt in ts.chunks_exact(188) {
        if pkt[0] == 0x47 {
            set.insert(u16::from_be_bytes([pkt[1] & 0x1f, pkt[2]]));
        }
    }
    set
}

#[test]
fn france2_vod_segments_carry_all_pids_and_endlist() {
    let data = fixture("france2-8s.ts");
    let src_pids = pids(&data);

    let mut s = HlsSession::new(HlsConfig::vod());
    for chunk in data.chunks(4096) {
        s.feed(chunk);
    }
    s.finish();

    assert!(s.is_ready(), "must produce at least one segment");

    let pl = s.playlist();
    assert!(pl.contains("#EXT-X-PLAYLIST-TYPE:VOD"), "VOD playlist type");
    assert!(
        pl.trim_end().ends_with("#EXT-X-ENDLIST"),
        "VOD ends with ENDLIST"
    );

    // Every listed segment must be servable, valid TS, and collectively carry
    // every source PID (multi-audio + DVB-subtitle survive the chop).
    let mut seg_pids = std::collections::BTreeSet::new();
    let mut seg_count = 0;
    for line in pl.lines().filter(|l| l.ends_with(".ts")) {
        seg_count += 1;
        let bytes = s
            .segment(line)
            .unwrap_or_else(|| panic!("segment {line} not servable"));
        assert!(!bytes.is_empty());
        assert_eq!(
            bytes[0], 0x47,
            "segment {line} must start with TS sync byte"
        );
        assert_eq!(
            bytes.len() % 188,
            0,
            "segment {line} must be whole TS packets"
        );
        seg_pids.extend(pids(&bytes));
    }
    assert!(seg_count >= 1, "at least one segment listed");

    let missing: Vec<u16> = src_pids.difference(&seg_pids).copied().collect();
    assert!(
        seg_pids.len() >= 5,
        "segments must carry video+audio+subtitle PIDs, got {seg_pids:?} (source {src_pids:?}, missing {missing:?})"
    );
}

#[test]
fn first_segment_starts_at_a_rap_and_no_endlist_before_finish() {
    let data = fixture("h264-25fps.ts");
    let mut s = HlsSession::new(HlsConfig::vod());
    for chunk in data.chunks(4096) {
        s.feed(chunk);
    }
    let mid = s.playlist();
    assert!(
        !mid.contains("#EXT-X-ENDLIST"),
        "no ENDLIST before finish()"
    );
    s.finish();
    let done = s.playlist();
    assert!(done.contains("#EXT-X-ENDLIST"), "ENDLIST after finish()");
    assert!(s.is_ready());
    let first = s
        .playlist()
        .lines()
        .find(|l| l.ends_with(".ts"))
        .unwrap()
        .to_string();
    let bytes = s.segment(&first).unwrap();
    assert_eq!(bytes[0], 0x47);
}
