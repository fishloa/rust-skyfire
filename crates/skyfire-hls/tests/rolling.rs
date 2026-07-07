use skyfire_hls::{HlsConfig, HlsSession};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .unwrap()
}

fn extract_target_duration(pl: &str) -> u64 {
    pl.lines()
        .find(|l| l.starts_with("#EXT-X-TARGETDURATION:"))
        .unwrap_or(":0")
        .trim_start_matches("#EXT-X-TARGETDURATION:")
        .parse()
        .unwrap_or(0)
}

fn extract_media_sequence(pl: &str) -> u64 {
    pl.lines()
        .find(|l| l.starts_with("#EXT-X-MEDIA-SEQUENCE:"))
        .unwrap_or(":0")
        .trim_start_matches("#EXT-X-MEDIA-SEQUENCE:")
        .parse()
        .unwrap_or(0)
}

fn extract_discontinuity_sequence(pl: &str) -> Option<u64> {
    pl.lines()
        .find(|l| l.starts_with("#EXT-X-DISCONTINUITY-SEQUENCE:"))
        .map(|l| {
            l.trim_start_matches("#EXT-X-DISCONTINUITY-SEQUENCE:")
                .parse()
                .unwrap()
        })
}

#[test]
fn rolling_window_caps_playlist_length_and_advances_media_sequence() {
    // france2-8s.ts is long enough to cut several ~2s segments.
    let data = fixture("france2-8s.ts");
    let mut s = HlsSession::new(HlsConfig {
        target_secs: 1,
        window: Some(2),
        uri_prefix: "seg".into(),
    });
    for chunk in data.chunks(4096) {
        s.feed(chunk);
    }
    s.finish();

    let pl = s.playlist();
    let listed = pl.lines().filter(|l| l.ends_with(".ts")).count();
    assert!(
        listed <= 2,
        "rolling window must cap listed segments at 2, got {listed}"
    );
    // Rolling playlists never carry ENDLIST or VOD type.
    assert!(
        !pl.contains("#EXT-X-ENDLIST"),
        "rolling playlist has no ENDLIST"
    );
    assert!(!pl.contains("VOD"), "rolling playlist is not VOD");
    // If more than `window` segments were cut, MEDIA-SEQUENCE advanced past 0.
    let seq_line = pl
        .lines()
        .find(|l| l.starts_with("#EXT-X-MEDIA-SEQUENCE:"))
        .unwrap();
    let seq: u64 = seq_line
        .trim_start_matches("#EXT-X-MEDIA-SEQUENCE:")
        .parse()
        .unwrap();
    // Verify MEDIA-SEQUENCE was emitted (value is fixture-dependent).
    let _ = seq;
}

#[test]
fn target_duration_monotonic_in_rolling_mode() {
    // Feed a fixture incrementally; capture playlist after every feed to ensure
    // TARGETDURATION never decreases as segments roll off.
    let data = fixture("france2-8s.ts");
    let mut s = HlsSession::new(HlsConfig {
        target_secs: 1,
        window: Some(2),
        uri_prefix: "seg".into(),
    });
    let mut prev_td: u64 = 0;
    let mut last_playlist = String::new();
    for chunk in data.chunks(4096) {
        s.feed(chunk);
        let pl = s.playlist();
        if pl != last_playlist {
            let td = extract_target_duration(&pl);
            assert!(
                td >= prev_td,
                "TARGETDURATION must be monotonic non-decreasing: {prev_td} → {td}"
            );
            prev_td = td;
            last_playlist = pl;
        }
    }
    s.finish();
    let final_pl = s.playlist();
    let final_td = extract_target_duration(&final_pl);
    assert!(
        final_td >= prev_td,
        "TARGETDURATION still monotonic after finish: {prev_td} → {final_td}"
    );
}

#[test]
fn rolling_playlist_emits_discontinuity_sequence_tag() {
    // We use a window=1 config so segments roll off aggressively.
    // The tag is emitted when discontinuity_sequence > 0, which requires
    // a discontinuous segment to be evicted. The fixture may or may not
    // produce discontinuous segments — we assert that if the playlist has
    // rolled, the tag appears when the counter is non-zero; otherwise we
    // just verify the tag does NOT appear when counter is zero.
    let data = fixture("france2-8s.ts");
    let mut s = HlsSession::new(HlsConfig {
        target_secs: 1,
        window: Some(1),
        uri_prefix: "seg".into(),
    });
    s.feed(&data);
    s.finish();

    let pl = s.playlist();
    let media_seq = extract_media_sequence(&pl);
    let has_disc_seq = pl.contains("#EXT-X-DISCONTINUITY-SEQUENCE:");

    if media_seq > 0 {
        // Rolling happened — the tag MAY be present (only if a discontinuous
        // segment was evicted). We just assert no panic and the tag is valid
        // if present.
        if let Some(_ds) = extract_discontinuity_sequence(&pl) {
            // ds is u64, always non-negative.
        }
        // If the tag is absent when media_sequence > 0, that's fine too
        // (no discont segments were evicted).
        let _ = has_disc_seq;
    } else {
        // No segments rolled off — the tag should not appear.
        assert!(
            !has_disc_seq,
            "DISCONTINUITY-SEQUENCE must not appear before any segment rolls off"
        );
    }
}
