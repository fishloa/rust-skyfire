//! Cross-codec audio channel-consistency guard (zenith #82/#83/#84).
//!
//! Every stream's audio chunks must have a CONSISTENT channel count (no per-frame
//! variance that the player would drop) and no zero-channel chunks. The MP2 path
//! once passed raw per-frame channels + never set last_audio_channels, so stereo
//! MP2 was heard as mono/silent; AC-3/E-AC-3 already normalised correctly. This
//! feeds each committed fixture through the real bridge and asserts consistency.

fn check(slug: &str) {
    let p = format!(
        "{}/../../fixtures/streams/{}.ts",
        env!("CARGO_MANIFEST_DIR"),
        slug
    );
    let ts = match std::fs::read(&p) {
        Ok(t) => t,
        Err(_) => {
            println!("AUDIT {slug}: (missing)");
            return;
        }
    };
    let mut b = skyfire_wasm::SkyfireBridge::new();
    // Cap per-fixture bytes: enough audio chunks to check consistency, fast enough
    // to stay under the nextest per-test timeout across all fixtures.
    b.feed(&ts[..ts.len().min(2_000_000)]);
    let mut set = std::collections::BTreeSet::new();
    let mut zero = 0;
    let mut n = 0;
    for c in b.take_audio_pcm() {
        n += 1;
        set.insert(c.channels);
        if c.channels == 0 {
            zero += 1;
        }
    }
    println!(
        "AUDIT {slug}: chunks={n} distinctChannelCounts={:?} zeroCh={zero} nativeCh={}",
        set,
        b.audio_native_channels()
    );
    assert!(n == 0 || zero == 0, "{slug}: no zero-channel chunks");
    assert!(
        set.len() <= 1,
        "{slug}: audio channel count must be CONSISTENT across chunks, got {:?}",
        set
    );
}
/// orf-3 is stereo MP2 — must NOT collapse to mono (the #82 regression).
#[test]
fn orf3_stereo_mp2_not_mono() {
    let ts = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/streams/orf-3.ts"
    ))
    .expect("orf-3");
    let mut b = skyfire_wasm::SkyfireBridge::new();
    b.feed(&ts);
    let ch: std::collections::BTreeSet<u16> =
        b.take_audio_pcm().iter().map(|c| c.channels).collect();
    assert_eq!(
        ch,
        std::collections::BTreeSet::from([2]),
        "orf-3 stereo MP2 must stay stereo"
    );
    assert_eq!(b.audio_native_channels(), 2);
}

#[test]
fn audio_channel_consistency_all_fixtures() {
    for s in ["arte", "france-2", "m6", "orf1", "rai-1", "tf-1", "orf-3"] {
        check(s);
    }
}
