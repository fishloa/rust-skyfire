use skyfire_server::manager::Manager;

#[test]
fn live_playlist_grows_then_caps_at_window() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let m = Manager::new(dir, vec!["france2-8s".to_string()]);

    // Before any feed, live slug is not ready.
    assert!(!m.is_ready("france2-8s"));

    // Feed in ~64 KiB steps until segments appear, tracking playlist growth.
    let mut prev_listed = 0usize;
    let mut grew = false;
    for _ in 0..2000 {
        m.feed_live_step("france2-8s", 64 * 1024);
        if let Some(pl) = m.playlist("france2-8s") {
            let listed = pl.lines().filter(|l| l.ends_with(".ts")).count();
            if listed > prev_listed {
                grew = true;
            }
            // Rolling window(6): never exceed 6 listed.
            assert!(
                listed <= 6,
                "live playlist must cap at window=6, got {listed}"
            );
            prev_listed = listed;
        }
        if m.at_eof("france2-8s") {
            break;
        }
    }
    assert!(grew, "live playlist must grow as segments are fed");
    assert!(m.is_ready("france2-8s"));
}
