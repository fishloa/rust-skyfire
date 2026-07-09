//! Regression guard (zenith #82/#83/#84): the bridge must NOT collapse stereo MP2
//! to mono. skyfire-mpa decodes orf-3's MP2 as stereo; the bridge previously passed
//! raw per-frame channels and never set last_audio_channels, so the player locked to
//! the first frame and dropped mismatched frames (stereo heard as mono / silent).
//! Fixed by normalising the MP2 path to stereo like the AC-3 path.

#[test]
fn orf3_mp2_chunks_are_stereo() {
    let ts = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/streams/orf-3.ts"
    ))
    .expect("orf-3");
    let mut b = skyfire_wasm::SkyfireBridge::new();
    b.feed(&ts);
    let mut chunks = 0;
    let mut mono = 0;
    let mut stereo = 0;
    let mut total = 0;
    for c in b.take_audio_pcm() {
        chunks += 1;
        total += c.samples.len();
        match c.channels {
            1 => mono += 1,
            2 => stereo += 1,
            _ => {}
        }
    }
    println!(
        "MP2CH orf-3: chunks={chunks} stereo={stereo} mono={mono} nativeCh={} samples={total}",
        b.audio_native_channels()
    );
    assert!(chunks > 0, "must produce audio chunks");
    assert_eq!(
        mono, 0,
        "NO chunk should be mono (stereo MP2 must stay stereo)"
    );
    assert!(stereo > 0, "chunks must be stereo");
    assert_eq!(
        b.audio_native_channels(),
        2,
        "audio_native_channels must report 2 for stereo MP2"
    );
}
