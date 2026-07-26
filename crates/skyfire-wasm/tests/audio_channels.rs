//! Per-PID channel counts must appear in the track list without decoding
//! anything but the selected PID (spec unit B, 2026-07-26).

/// Feed a fixture through the real bridge and return its track list once the
/// first video track has been seen. Mirrors the fixture-loading + bridge
/// construction pattern in `audio_channel_consistency.rs`'s `check()`, but
/// generalised to fixtures both under `fixtures/streams/` (real broadcast
/// captures, given as `"streams/<name>.ts"`) and directly under `fixtures/`
/// (synthetic single-track captures, given bare).
fn track_list_for(rel: &str) -> skyfire_wasm::WasmTrackList {
    let p = format!("{}/../../fixtures/{}", env!("CARGO_MANIFEST_DIR"), rel);
    let ts = std::fs::read(&p).unwrap_or_else(|e| panic!("read fixture {p}: {e}"));
    let mut b = skyfire_wasm::SkyfireBridge::new();
    // Cap per-fixture bytes like the sibling harness: enough to observe the
    // first frame of every audio PID, fast enough to stay under the nextest
    // per-test timeout across all fixtures.
    b.feed(&ts[..ts.len().min(2_000_000)]);
    b.track_list()
        .unwrap_or_else(|| panic!("{rel}: no track list (no video track seen)"))
}

/// Expected channels per PID, from
/// `ffprobe -select_streams a -show_entries stream=channels:stream_tags=language`
/// (verified 2026-07-26).
fn assert_channels(file: &str, expected: &[(u16, u8)]) {
    let tl = track_list_for(file);
    let got: Vec<(u16, Option<u8>)> = tl.audio.iter().map(|t| (t.pid, t.channels)).collect();
    let want: Vec<(u16, Option<u8>)> = expected.iter().map(|(p, c)| (*p, Some(*c))).collect();
    assert_eq!(got, want, "{file}");
}

#[test]
fn mixed_codec_stream_reports_channels_for_both_pids() {
    // orf1: AC3 5.1 (deu, pid 257) + MP2 stereo (mis, pid 258). Two codecs,
    // two probe paths, and only ONE of them is the selected/decoded PID —
    // so this fails if the probe only runs on the selected track.
    assert_channels("streams/orf1.ts", &[(257, 6), (258, 2)]);
}

#[test]
fn real_broadcast_mono_mp2_is_detected_as_one_channel() {
    // rai-1 pid 259 is genuinely mono MP2 (mode == 0b11) on real broadcast
    // data, beside three stereo tracks. This is the case a hardcoded
    // "MP2 is always stereo" shortcut would get wrong.
    assert_channels(
        "streams/rai-1.ts",
        &[(257, 2), (258, 2), (259, 1), (260, 2)],
    );
}

#[test]
fn five_one_fixtures_report_six_channels() {
    // ac3-51.ts and eac3-51.ts are 5.1(side) per ffprobe: acmod 7 + lfeon.
    for f in ["ac3-51.ts", "eac3-51.ts"] {
        let tl = track_list_for(f);
        assert_eq!(tl.audio[0].channels, Some(6), "{f} should be 5.1");
    }
}

#[test]
fn mp2_fixture_reports_stereo() {
    let tl = track_list_for("mp2-tone.ts");
    assert_eq!(tl.audio[0].channels, Some(2));
}
