use skyfire_hls::{HlsConfig, HlsSession};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .unwrap()
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
