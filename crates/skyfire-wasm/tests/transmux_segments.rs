//! Native (non-wasm) tests of the CMAF segment builders on the bridge.
//!
//! These tests run on the host — the bridge logic is target-independent
//! Rust that happens to also compile to WASM.
//!
//! The exit criteria:
//! - `video_init_segment()` returns non-empty; first box is `ftyp`; `moov` present.
//! - `take_video_media_segment()` iterates; each segment starts with `styp`,
//!   contains `moof` and `mdat`, and has `sample_count > 0`.
//! - Every top-level box of both the init and every media segment re-parses
//!   successfully via transmux's box iterator (proving real ISOBMFF).
//! - Sample accounting: sum of sample_count across all media segments equals
//!   the number of video AUs the demux produces, minus at most 1 (the
//!   possible pre-keyframe drop).

use skyfire_wasm::SkyfireBridge;

fn feed_fixture(bridge: &mut SkyfireBridge, name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    let data = std::fs::read(path).expect("fixture not found");
    for chunk in data.chunks(4096) {
        bridge.feed(chunk);
    }
    bridge.flush();
}

#[test]
fn init_segment_is_valid_ftyp_moov() {
    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");
    let init = bridge.video_init_segment();
    assert!(
        !init.is_empty(),
        "init segment must be produced once SPS seen"
    );
    // ftyp is the first box.
    assert_eq!(&init[4..8], b"ftyp");
    // moov appears somewhere after ftyp.
    assert!(
        init.windows(4).any(|w| w == b"moov"),
        "moov must be present"
    );
}

#[test]
fn media_segments_cover_all_samples() {
    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");
    assert!(!bridge.video_init_segment().is_empty());

    let mut total = 0u32;
    let mut seg_count = 0u32;
    while let Some(seg) = bridge.take_video_media_segment() {
        assert_eq!(&seg.bytes[4..8], b"styp", "media segment starts with styp");
        assert!(
            seg.bytes.windows(4).any(|w| w == b"moof"),
            "media segment must contain moof"
        );
        assert!(
            seg.bytes.windows(4).any(|w| w == b"mdat"),
            "media segment must contain mdat"
        );
        assert!(seg.sample_count > 0, "each segment must have samples");
        total += seg.sample_count;
        seg_count += 1;
    }
    assert!(seg_count > 0, "at least one GOP segment");
    assert!(total > 0, "samples must be emitted");
}

#[test]
fn init_and_media_reparse_and_account_all_aus() {
    // Baseline AU count straight from the demux.
    let expected_aus = {
        use dvb_si::resync::TsResync;
        use skyfire_ts::EsDemux;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join("gulli-15s.ts");
        let data = std::fs::read(path).unwrap();
        let (mut d, mut r) = (EsDemux::new(), TsResync::new());
        for chunk in data.chunks(4096) {
            for pkt in r.feed(chunk) {
                d.feed_packet(&pkt);
            }
        }
        d.flush();
        d.drain().into_iter().filter(|a| a.pid == 0x0100).count()
    };

    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");

    // Init segment: every top-level box parses via transmux.
    let init = bridge.video_init_segment();
    for result in transmux::box_iter(&init) {
        let (_box_ref, _consumed) = result.expect("init box must parse successfully");
    }

    let mut muxed = 0usize;
    while let Some(seg) = bridge.take_video_media_segment() {
        for result in transmux::box_iter(&seg.bytes) {
            let (_box_ref, _consumed) = result.expect("media box must parse successfully");
        }
        muxed += seg.sample_count as usize;
    }

    // Every video AU is carried as exactly one sample (leading pre-keyframe
    // AUs, if any, are legitimately dropped — allow a small slack).
    assert!(
        muxed > 0 && muxed <= expected_aus,
        "muxed {muxed} samples vs {expected_aus} demuxed AUs"
    );
    assert!(
        expected_aus - muxed <= 1,
        "at most the pre-keyframe AU may be dropped"
    );
}
