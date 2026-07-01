# Adopt transmux Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace skyfire's hand-rolled H.264 container code with the `transmux` crate, and add an fMP4/CMAF MSE video fallback for browsers without WebCodecs H.264 (notably iOS Safari).

**Architecture:** `transmux` is a samples-in, `no_std`+alloc, `forbid(unsafe)` container-layer crate (TS→CMAF/fMP4). skyfire keeps `h264_reader` for SPS RBSP decode (transmux does not parse SPS), feeds SPS/PPS + geometry into `transmux::AVCDecoderConfigurationRecord` for the avcC, and uses `transmux::{build_init_segment, build_media_segment}` in the WASM bridge to emit CMAF segments for an MSE `SourceBuffer`. WebCodecs stays primary; MSE activates only when `VideoDecoder.isConfigSupported` fails. Audio (AC-3→WASM→WebAudio) and the audio-master clock are untouched.

**Tech Stack:** Rust (workspace, edition 2021, rustc 1.81), `transmux 0.1`, `broadcast-common 8`, `h264_reader 0.8`, `wasm-bindgen`, WebCodecs + MSE (JS), `cargo nextest`, Playwright.

## Global Constraints

- No `unsafe` anywhere (spec + workspace rule).
- Dual licence: MIT OR Apache-2.0.
- **No `Co-Authored-By` lines in commits.**
- CI gate must be green before every commit: `cargo fmt --all --check` / `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings) / `cargo build --workspace` / `cargo nextest run --workspace`.
- Touch only the crates a task needs; keep everything that passes green.
- rustc floor 1.81 (`workspace.package.rust-version`).
- Branch: `feat/adopt-transmux` (already created; design doc committed at `f1c1335`).
- TS fixtures live in `fixtures/`; reuse, don't fetch.

Reference spec: `docs/superpowers/specs/2026-07-01-adopt-transmux-design.md`.

## File Structure

- `crates/skyfire-ts/Cargo.toml` — add `broadcast-common = "8"`, `transmux = "0.1"`; bump `dvb-pes`/`dvb-subtitle` (already locked).
- `crates/skyfire-ts/src/lib.rs` — trait-scope imports for `broadcast-common` migration (2 call sites).
- `crates/skyfire-ts/src/subtitle_compositor.rs` — trait-scope imports (8 call sites).
- `crates/skyfire-ts/src/h264_config.rs` — swap `annexb_to_avcc` + `build_avcc_description` to transmux; extend `VideoConfig` with `width`/`height`/`avcc_box`.
- `crates/skyfire-wasm/Cargo.toml` — add `transmux = "0.1"`.
- `crates/skyfire-wasm/src/lib.rs` — new bridge API `video_init_segment()`, `take_video_media_segment()`, struct `WasmMediaSegment`.
- `crates/skyfire-wasm/tests/transmux_segments.rs` — native fixture test (init+media round-trip).
- `web/player.js` — capability gate, MSE path, A/V drift corrector.
- `web/tests/*` — Playwright MSE verification (path per existing e2e harness).

---

### Task 1: `broadcast-common 8` trait migration (restore green)

**Files:**
- Modify: `crates/skyfire-ts/Cargo.toml`
- Modify: `crates/skyfire-ts/src/lib.rs:398` and top-of-file imports
- Modify: `crates/skyfire-ts/src/subtitle_compositor.rs` (call sites 514,517,520,523,555,559,569,580,765) and top-of-file imports

**Interfaces:**
- Consumes: `dvb_subtitle` segment types now impl `broadcast_common::traits::{Parse, Serialize}` instead of inherent `parse`/`to_bytes`.
- Produces: green workspace build (no API change to skyfire).

- [ ] **Step 1: Add the dependency**

In `crates/skyfire-ts/Cargo.toml`, under `[dependencies]`, add above `dvb-common`:

```toml
broadcast-common = "8"
```

And confirm these lines read (already locked by `cargo update`):

```toml
dvb-pes = "0.1.2"
dvb-subtitle = "0.1"
```

- [ ] **Step 2: Run the build to see the failing state**

Run: `cargo build -p skyfire-ts 2>&1 | grep -c E0599`
Expected: non-zero (10 errors: `parse`/`to_bytes` not found).

- [ ] **Step 3: Bring the traits into scope in `lib.rs`**

At the top of `crates/skyfire-ts/src/lib.rs` (with the other `use` lines), add:

```rust
use broadcast_common::traits::Parse;
```

This covers `PesDataField::parse` (line 398) and any `PesPacket::parse` that resolves through the same trait. (If `PesPacket` comes from `dvb-pes` and still exposes inherent `parse`, this import is harmless — `cargo clippy` will flag it as unused only if truly unused; keep it only if the build needs it.)

- [ ] **Step 4: Bring the traits into scope in `subtitle_compositor.rs`**

At the top of `crates/skyfire-ts/src/subtitle_compositor.rs`, add:

```rust
use broadcast_common::traits::{Parse, Serialize};
```

`Parse` covers the `*Segment::parse` / `PesDataField::parse` calls; `Serialize` covers the `.to_bytes()` calls on `PageCompositionSegment` / `RegionCompositionSegment` / `ClutDefinitionSegment` / `ObjectDataSegment`.

> Note: if `broadcast_common` renamed `to_bytes` (e.g. to `serialize`/`to_vec`), grep the trait: `gh api repos/fishloa/rust-broadcast/contents/broadcast-common/src/traits.rs -H "Accept: application/vnd.github.raw" | grep "fn "`. Use the real method name at each `.to_bytes()` site.

- [ ] **Step 5: Verify the CI gate is green**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo build --workspace && cargo nextest run --workspace`
Expected: all pass, zero warnings. In particular `cargo nextest run -p skyfire-ts` stays green (existing subtitle + h264 golden tests unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/skyfire-ts/Cargo.toml crates/skyfire-ts/src/lib.rs crates/skyfire-ts/src/subtitle_compositor.rs Cargo.lock
git commit -m "fix(ts): migrate to broadcast-common 8 Parse/Serialize traits"
```

---

### Task 2: Swap Annex B→AVCC converter to transmux

**Files:**
- Modify: `crates/skyfire-ts/Cargo.toml` (add `transmux = "0.1"`)
- Modify: `crates/skyfire-ts/src/h264_config.rs:161-210` (`annexb_to_avcc`)

**Interfaces:**
- Consumes: `transmux::annexb_to_length_prefixed(annexb: &[u8]) -> Vec<u8>` (4-byte length prefix, identical semantics to current `annexb_to_avcc`).
- Produces: `annexb_to_avcc` retained as a thin public wrapper (call sites in `skyfire-wasm` keep compiling) delegating to transmux.

- [ ] **Step 1: Add the dependency**

In `crates/skyfire-ts/Cargo.toml` `[dependencies]`:

```toml
transmux = "0.1"
```

- [ ] **Step 2: Keep the existing golden test as the regression guard**

The existing `annexb`-shaped behaviour is exercised transitively by `golden_gulli_15s` / `golden_h264_25fps` (avcC) and by `skyfire-wasm` drain tests. Add a focused equivalence test in `crates/skyfire-ts/src/h264_config.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn annexb_to_avcc_matches_transmux() {
    // Two NALs: 4-byte then 3-byte start code.
    let annexb: &[u8] = &[0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0xCC];
    let got = annexb_to_avcc(annexb);
    let want = transmux::annexb_to_length_prefixed(annexb);
    assert_eq!(got, want);
    // Explicit expected: [len=2][67 AA][len=3][68 BB CC]
    assert_eq!(got, vec![0, 0, 0, 2, 0x67, 0xAA, 0, 0, 0, 3, 0x68, 0xBB, 0xCC]);
}
```

- [ ] **Step 3: Run it against the current hand-rolled impl to verify equivalence**

Run: `cargo nextest run -p skyfire-ts annexb_to_avcc_matches_transmux`
Expected: PASS (confirms transmux output equals the current converter on this vector before we swap).

- [ ] **Step 4: Replace the body with the transmux delegate**

Replace the whole `pub fn annexb_to_avcc` body (lines 161-210) with:

```rust
/// Convert an Annex-B NAL stream to AVCC length-prefixed format
/// (4-byte big-endian length before each NAL), suitable for
/// `EncodedVideoChunk` when the `VideoDecoder` has an avcC `description`.
///
/// Thin wrapper over `transmux::annexb_to_length_prefixed` (ISO/IEC 14496-15
/// length-prefixed mdat form). Retained so existing call sites keep compiling.
pub fn annexb_to_avcc(annexb: &[u8]) -> Vec<u8> {
    transmux::annexb_to_length_prefixed(annexb)
}
```

- [ ] **Step 5: Verify the CI gate is green**

Run: `cargo clippy -p skyfire-ts --all-targets -- -D warnings && cargo nextest run -p skyfire-ts && cargo build --workspace`
Expected: all pass. The equivalence test now compares transmux to itself → still PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/skyfire-ts/Cargo.toml crates/skyfire-ts/src/h264_config.rs Cargo.lock
git commit -m "refactor(ts): use transmux::annexb_to_length_prefixed for AVCC conversion"
```

---

### Task 3: Swap avcC builder to `transmux::AVCDecoderConfigurationRecord` + extend `VideoConfig`

**Files:**
- Modify: `crates/skyfire-ts/src/h264_config.rs` (`VideoConfig`, `build_video_config`, delete `build_avcc_description`, update golden tests)

**Interfaces:**
- Consumes: `h264_reader` `SeqParameterSet` fields (`profile_idc`, `constraint_flags`, `level_idc`, chroma/bit-depth, picture dimensions); `transmux::{AVCDecoderConfigurationRecord, AVCConfigurationBox, nalu_types::{AvcSps, AvcPps}}`; `broadcast_common::traits::Serialize` for `serialized_len`/`serialize_into`.
- Produces: extended `VideoConfig`:
  ```rust
  pub struct VideoConfig {
      pub codec: String,          // RFC 6381, e.g. "avc1.640028"
      pub description: Vec<u8>,    // avcC record bytes (no box header) for WebCodecs
      pub interlaced: bool,
      pub width: u16,             // coded width (px)
      pub height: u16,            // coded height (px)
      pub avcc_box: transmux::AVCConfigurationBox, // for MSE TrackSpec (Task 4)
  }
  ```
  `pub(crate) fn avc_record(...) -> transmux::AVCDecoderConfigurationRecord` helper shared by description + box.

> **Behavioural change (flagged in spec §Part 2):** transmux emits the ISO/IEC 14496-15 §5.3.3.1.2 high-profile extension fields (chroma_format, bit depths, sps_ext) for High-family profiles. skyfire's current builder omits them, so the golden avcC bytes for High-profile fixtures WILL grow. This is more spec-conformant. The golden tests below are re-baselined to transmux output, and WebCodecs decode is re-verified in Task 8.

- [ ] **Step 1: Compute coded dimensions helper**

Add to `crates/skyfire-ts/src/h264_config.rs`:

```rust
/// Coded luma dimensions from an h264_reader SPS (§7.4.2.1.1, accounting for
/// frame_cropping and frame_mbs_only). Returns (width, height) in pixels.
fn sps_dimensions(sps: &h264_reader::nal::sps::SeqParameterSet) -> (u16, u16) {
    let (w, h) = sps.pixel_dimensions().unwrap_or((0, 0));
    (w as u16, h as u16)
}
```

(`SeqParameterSet::pixel_dimensions()` exists in `h264_reader 0.8`; it applies crop + interlace factors.)

- [ ] **Step 2: Write the shared record builder**

Add:

```rust
use broadcast_common::traits::Serialize;
use transmux::nalu_types::{AvcPps, AvcSps};
use transmux::{AVCConfigurationBox, AVCDecoderConfigurationRecord};

/// Build the transmux AVCDecoderConfigurationRecord from raw SPS/PPS NAL bytes
/// and the decoded SPS (for the high-profile extension fields).
fn avc_record(
    sps: &h264_reader::nal::sps::SeqParameterSet,
    sps_bytes: &[u8],
    pps_bytes: &[u8],
) -> AVCDecoderConfigurationRecord {
    let profile = sps.profile_idc.into();
    let high = matches!(profile, 100 | 110 | 122 | 144 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135 | 244);
    let (chroma_format, luma8, chroma8) = if high {
        (
            Some(sps.chroma_info.chroma_format.to_chroma_format_idc()),
            Some((sps.chroma_info.bit_depth_luma_minus8) as u8),
            Some((sps.chroma_info.bit_depth_chroma_minus8) as u8),
        )
    } else {
        (None, None, None)
    };
    AVCDecoderConfigurationRecord {
        configuration_version: 1,
        profile_indication: profile,
        profile_compatibility: sps.constraint_flags.into(),
        level_indication: sps.level_idc,
        length_size_minus_one: 3,
        sps: vec![AvcSps(sps_bytes.to_vec())],
        pps: vec![AvcPps(pps_bytes.to_vec())],
        chroma_format,
        bit_depth_luma_minus8: luma8,
        bit_depth_chroma_minus8: chroma8,
        sps_ext: vec![],
    }
}
```

> Plan-time verify against the real struct: `gh api repos/fishloa/rust-broadcast/contents/transmux/src/avc_config.rs -H "Accept: application/vnd.github.raw" | sed -n '1,80p'`. Match exact field names (`bit_depth_luma_minus8` etc.) and the `has_high_profile_ext` profile list; copy that list verbatim rather than the guess above. Likewise confirm the h264_reader accessor names for chroma/bit-depth and adjust.

- [ ] **Step 3: Rewrite `build_video_config` to use the record and populate the new fields**

Replace `build_video_config` (lines 69-96) with:

```rust
fn build_video_config(sps_bytes: &[u8], pps_bytes: &[u8]) -> Option<VideoConfig> {
    let sps_nal = RefNal::new(sps_bytes, &[], true);
    let sps = SeqParameterSet::from_bits(sps_nal.rbsp_bits()).ok()?;

    let codec = sps.rfc6381().to_string();
    let interlaced = matches!(
        sps.frame_mbs_flags,
        h264_reader::nal::sps::FrameMbsFlags::Fields { .. }
    );
    let (width, height) = sps_dimensions(&sps);

    let record = avc_record(&sps, sps_bytes, pps_bytes);
    // WebCodecs `description` = the raw record bytes (no box header).
    let mut description = vec![0u8; record.serialized_len()];
    let n = record.serialize_into(&mut description).ok()?;
    description.truncate(n);

    let avcc_box = AVCConfigurationBox::new(record);

    Some(VideoConfig { codec, description, interlaced, width, height, avcc_box })
}
```

Then **delete** the old `build_avcc_description` fn (lines 114-151) entirely.

- [ ] **Step 4: Update the `VideoConfig` struct**

Replace the struct (lines 6-16) with the extended definition from the Interfaces block above. Note `transmux::AVCConfigurationBox` must derive `Clone`+`Debug` (it does) for `VideoConfig`'s existing derives; drop `PartialEq, Eq` from `VideoConfig`'s derive if the box does not impl `Eq` (use `#[derive(Debug, Clone)]`).

- [ ] **Step 5: Re-baseline the golden tests**

The two golden tests assert exact avcC bytes. Run them to capture the new transmux bytes:

Run: `cargo nextest run -p skyfire-ts golden_gulli_15s golden_h264_25fps 2>&1 | tail -40`
Expected: FAIL, with the assertion printing actual vs expected.

Copy the *actual* bytes into `expected_avcc` in each test, and update the structural assertions that indexed into the old layout (bytes after the SPS/PPS now include the ext fields). Keep the `codec` assertions (`avc1.640028`, `avc1.F4000C`) unchanged. Add one assertion documenting the ext presence:

```rust
// High profile ⇒ ISO 14496-15 §5.3.3.1.2 ext fields present after PPS.
assert!(config.description.len() > pps_offset + 3 + pps_len,
        "high-profile ext bytes must follow the PPS");
```

- [ ] **Step 6: Verify round-trip**

Add a test proving transmux can parse back what it wrote:

```rust
#[test]
fn avcc_record_roundtrips() {
    let video_units = video_access_units("gulli-15s.ts", 0x0100);
    let cfg = h264_decoder_config(&video_units).unwrap();
    let reparsed = transmux::AVCDecoderConfigurationRecord::parse(&cfg.description)
        .expect("record must reparse");
    assert_eq!(reparsed.profile_indication, 100);
    assert_eq!(reparsed.level_indication, 40);
}
```

(If `AVCDecoderConfigurationRecord::parse` is not public, parse via `AVCConfigurationBox::parse_body` on the box body, or serialise the box and `parse_box`; adjust to the public surface.)

- [ ] **Step 7: Verify the CI gate is green**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace`
Expected: all pass. `skyfire-wasm` still builds (it consumes `video_config_description()` unchanged; the new `VideoConfig` fields are additive).

- [ ] **Step 8: Commit**

```bash
git add crates/skyfire-ts/src/h264_config.rs
git commit -m "refactor(ts): build avcC via transmux record; add width/height/avcc_box to VideoConfig"
```

---

### Task 4: Bridge — `video_init_segment()`

**Files:**
- Modify: `crates/skyfire-wasm/Cargo.toml` (add `transmux = "0.1"`)
- Modify: `crates/skyfire-wasm/src/lib.rs` (SkyfireBridge impl)

**Interfaces:**
- Consumes: `self.cached_video_config: Option<skyfire_ts::h264_config::VideoConfig>` (now carries `width`/`height`/`avcc_box`); `transmux::{TrackSpec, CodecConfig, build_init_segment}`.
- Produces: `pub fn video_init_segment(&self) -> Vec<u8>` — CMAF init (ftyp+moov) once SPS/PPS seen, else empty. Video track_id = 1, timescale = 90_000.

- [ ] **Step 1: Add the dependency**

In `crates/skyfire-wasm/Cargo.toml` `[dependencies]`:

```toml
transmux = "0.1"
```

- [ ] **Step 2: Write the failing native test**

In a new file `crates/skyfire-wasm/tests/transmux_segments.rs`:

```rust
//! Native (non-wasm) test of the CMAF segment builders on the bridge.
//! Runs on the host — the SkyfireBridge logic is target-independent.
use skyfire_wasm::SkyfireBridge;

fn feed_fixture(bridge: &mut SkyfireBridge, name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures").join(name);
    let data = std::fs::read(path).expect("fixture");
    for chunk in data.chunks(4096) {
        bridge.feed(&chunk);
    }
    bridge.flush();
}

#[test]
fn init_segment_is_valid_ftyp_moov() {
    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");
    let init = bridge.video_init_segment();
    assert!(!init.is_empty(), "init segment must be produced once SPS seen");
    // ftyp is the first box.
    assert_eq!(&init[4..8], b"ftyp");
    // moov appears somewhere after ftyp.
    assert!(init.windows(4).any(|w| w == b"moov"), "moov must be present");
}
```

> If `SkyfireBridge::feed` / `new` are not `pub` for host builds, gate the test with the crate's existing native-test pattern (check how current `skyfire-wasm` tests, if any, invoke the bridge; mirror it). The bridge logic must be reachable without `wasm-bindgen` codegen — it is plain Rust.

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo nextest run -p skyfire-wasm init_segment_is_valid_ftyp_moov`
Expected: FAIL — `video_init_segment` not defined.

- [ ] **Step 4: Implement `video_init_segment`**

In the `#[wasm_bindgen] impl SkyfireBridge` block (near `video_config_description`), add:

```rust
/// CMAF initialization segment (`ftyp` + fragmented-init `moov`) for the
/// video track, for MSE playback. Empty until SPS/PPS have been seen.
/// Track id = 1, timescale = 90_000 (matches TS PTS).
#[wasm_bindgen]
pub fn video_init_segment(&self) -> Vec<u8> {
    let Some(cfg) = self.cached_video_config.as_ref() else {
        return Vec::new();
    };
    let track = transmux::TrackSpec {
        track_id: 1,
        timescale: 90_000,
        config: transmux::CodecConfig::Avc {
            config: cfg.avcc_box.clone(),
            width: cfg.width,
            height: cfg.height,
        },
    };
    transmux::build_init_segment(&[track], 90_000).unwrap_or_default()
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo nextest run -p skyfire-wasm init_segment_is_valid_ftyp_moov`
Expected: PASS.

- [ ] **Step 6: Verify the CI gate is green**

Run: `cargo clippy -p skyfire-wasm --all-targets -- -D warnings && cargo build --workspace`
Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/skyfire-wasm/Cargo.toml crates/skyfire-wasm/src/lib.rs crates/skyfire-wasm/tests/transmux_segments.rs Cargo.lock
git commit -m "feat(wasm): video_init_segment() emits CMAF ftyp+moov for MSE"
```

---

### Task 5: Bridge — `take_video_media_segment()` (one CMAF segment per GOP)

**Files:**
- Modify: `crates/skyfire-wasm/src/lib.rs` (add `WasmMediaSegment`, `take_video_media_segment`, a `media_seq: u32` counter, and pending-AU buffering that carries dts/keyframe)

**Interfaces:**
- Consumes: pending video AUs (Annex B bytes + `pts_ticks`/`dts_ticks`/`is_keyframe`); `transmux::{Sample, FragmentTrackData, build_media_segment}`.
- Produces:
  ```rust
  #[wasm_bindgen]
  pub struct WasmMediaSegment {
      pub base_media_decode_time: u64, // 90 kHz ticks, first sample DTS
      #[wasm_bindgen(getter_with_clone)]
      pub bytes: Vec<u8>,              // styp + moof + mdat
      pub sample_count: u32,
  }
  ```
  `pub fn take_video_media_segment(&mut self) -> Option<WasmMediaSegment>` — returns the next complete GOP (from one keyframe up to, exclusive, the next keyframe) as one media segment, or `None` if no full GOP is buffered yet. Increments `media_seq` (moof sequence_number).

- [ ] **Step 1: Write the failing test**

Append to `crates/skyfire-wasm/tests/transmux_segments.rs`:

```rust
#[test]
fn media_segments_cover_all_samples() {
    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");
    assert!(!bridge.video_init_segment().is_empty());

    let mut total = 0u32;
    let mut seg_count = 0u32;
    while let Some(seg) = bridge.take_video_media_segment() {
        assert_eq!(&seg.bytes[4..8], b"styp", "media segment starts with styp");
        assert!(seg.bytes.windows(4).any(|w| w == b"moof"));
        assert!(seg.bytes.windows(4).any(|w| w == b"mdat"));
        assert!(seg.sample_count > 0);
        total += seg.sample_count;
        seg_count += 1;
    }
    assert!(seg_count > 0, "at least one GOP segment");
    assert!(total > 0, "samples must be emitted");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo nextest run -p skyfire-wasm media_segments_cover_all_samples`
Expected: FAIL — `take_video_media_segment` not defined.

- [ ] **Step 3: Add the segment struct**

Add near `WasmVideoAu`:

```rust
/// One CMAF media segment (`styp` + `moof` + `mdat`) for the video track.
#[wasm_bindgen]
pub struct WasmMediaSegment {
    /// Decode time of the first sample, 90 kHz ticks.
    pub base_media_decode_time: u64,
    /// Serialized segment bytes.
    #[wasm_bindgen(getter_with_clone)]
    pub bytes: Vec<u8>,
    /// Number of samples in the segment.
    pub sample_count: u32,
}
```

- [ ] **Step 4: Add a sequence counter field**

In the `SkyfireBridge` struct definition add `media_seq: u32,` and initialise it to `1` in the constructor (find `Self { ... }` in `SkyfireBridge::new`).

- [ ] **Step 5: Implement `take_video_media_segment`**

Add to the impl block:

```rust
/// Drain the next complete GOP (keyframe → just before the next keyframe)
/// as a CMAF media segment. Returns `None` until a full GOP is buffered.
/// Sample durations are the DTS deltas (90 kHz); composition offset = pts−dts.
#[wasm_bindgen]
pub fn take_video_media_segment(&mut self) -> Option<WasmMediaSegment> {
    // self.video_aus holds AUs in decode order (Annex B, with pts/dts/keyframe).
    // Find the GOP: first AU must be a keyframe; end at the next keyframe.
    if self.video_aus.is_empty() || !self.video_aus[0].is_keyframe {
        // Drop leading non-keyframe AUs (can't start a segment mid-GOP).
        while self.video_aus.first().map(|a| !a.is_keyframe).unwrap_or(false) {
            self.video_aus.remove(0);
        }
    }
    if self.video_aus.is_empty() {
        return None;
    }
    let gop_end = self.video_aus.iter().skip(1)
        .position(|a| a.is_keyframe)
        .map(|p| p + 1);
    // Without a following keyframe we cannot yet know the GOP is closed,
    // unless the stream has ended (flush drained everything). Emit only when
    // a boundary is known OR the buffer is the tail after flush.
    let end = match gop_end {
        Some(e) => e,
        None if self.ended => self.video_aus.len(),
        None => return None,
    };

    let gop: Vec<_> = self.video_aus.drain(0..end).collect();
    let dts: Vec<u64> = gop.iter()
        .map(|a| a.dts_ticks.or(a.pts_ticks).unwrap_or(0)).collect();
    let base_media_decode_time = dts[0];

    let mut samples = Vec::with_capacity(gop.len());
    for (i, au) in gop.iter().enumerate() {
        let duration = if i + 1 < dts.len() {
            (dts[i + 1].saturating_sub(dts[i])) as u32
        } else {
            // last sample: reuse previous delta, else a 25 fps default (3600).
            if i > 0 { (dts[i].saturating_sub(dts[i - 1])) as u32 } else { 3600 }
        };
        let pts = au.pts_ticks.unwrap_or(dts[i]);
        let composition_offset = (pts as i64 - dts[i] as i64) as i32;
        samples.push(transmux::Sample::from_annexb(
            &au.bytes, duration, au.is_keyframe, composition_offset,
        ));
    }

    let sample_count = samples.len() as u32;
    let seq = self.media_seq;
    self.media_seq += 1;
    let bytes = transmux::build_media_segment(seq, &[transmux::FragmentTrackData {
        track_id: 1,
        base_media_decode_time,
        samples: &samples,
    }]).unwrap_or_default();

    Some(WasmMediaSegment { base_media_decode_time, bytes, sample_count })
}
```

> Confirm two things against the source before finalising: (a) `SkyfireBridge` has an `ended: bool` set by `flush()` — if not, add one (`self.ended = true` in `flush`); (b) `build_media_segment`'s first parameter is the `u32` sequence number (README shows `build_media_segment(1, &[...])`). Adjust if the real signature differs.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo nextest run -p skyfire-wasm media_segments_cover_all_samples`
Expected: PASS.

- [ ] **Step 7: Verify the CI gate is green**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace && cargo build --workspace`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/skyfire-wasm/src/lib.rs crates/skyfire-wasm/tests/transmux_segments.rs
git commit -m "feat(wasm): take_video_media_segment() emits per-GOP CMAF segments"
```

---

### Task 6: Native end-to-end fixture assertion (init+media parse + sample accounting)

**Files:**
- Modify: `crates/skyfire-wasm/tests/transmux_segments.rs`

**Interfaces:**
- Consumes: `video_init_segment()`, `take_video_media_segment()`, `transmux::{box_iter, parse_box}` for structural validation.
- Produces: a deterministic golden proving the muxed output is well-formed ISOBMFF and accounts for every demuxed AU.

- [ ] **Step 1: Write the accounting test**

Append:

```rust
#[test]
fn init_and_media_reparse_and_account_all_aus() {
    // Baseline AU count straight from the demux.
    let expected_aus = {
        use skyfire_ts::{EsDemux};
        use dvb_si::resync::TsResync;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures").join("gulli-15s.ts");
        let data = std::fs::read(path).unwrap();
        let (mut d, mut r) = (EsDemux::new(), TsResync::new());
        for c in data.chunks(4096) { for p in r.feed(c) { d.feed_packet(&p); } }
        d.flush();
        d.drain().into_iter().filter(|a| a.pid == 0x0100).count()
    };

    let mut bridge = SkyfireBridge::new();
    feed_fixture(&mut bridge, "gulli-15s.ts");

    // init segment: every top-level box parses via transmux.
    let init = bridge.video_init_segment();
    for b in transmux::box_iter(&init) { let _ = b.expect("init box parses"); }

    let mut muxed = 0usize;
    while let Some(seg) = bridge.take_video_media_segment() {
        for b in transmux::box_iter(&seg.bytes) { let _ = b.expect("media box parses"); }
        muxed += seg.sample_count as usize;
    }
    // Every video AU is carried as exactly one sample (leading pre-keyframe
    // AUs, if any, are legitimately dropped — allow a small slack).
    assert!(muxed > 0 && muxed <= expected_aus,
        "muxed {muxed} samples vs {expected_aus} demuxed AUs");
    assert!(expected_aus - muxed <= 1, "at most the pre-keyframe AU may be dropped");
}
```

> Adjust `box_iter`/`parse_box` usage to the real API (Task-time: `gh api repos/fishloa/rust-broadcast/contents/transmux/src/box_types.rs -H "Accept: application/vnd.github.raw" | grep "pub fn box_iter\|pub fn parse_box"`). `dvb_si` is a dev-dep here — add `dvb-si` / `skyfire-ts` under `[dev-dependencies]` of `skyfire-wasm` if not already present.

- [ ] **Step 2: Add dev-deps if needed**

If the test fails to resolve `dvb_si`/`skyfire_ts`, add to `crates/skyfire-wasm/Cargo.toml`:

```toml
[dev-dependencies]
dvb-si = "7.9"
```

(`skyfire-ts` is already a normal dep of `skyfire-wasm`.)

- [ ] **Step 3: Run it**

Run: `cargo nextest run -p skyfire-wasm init_and_media_reparse_and_account_all_aus`
Expected: PASS.

- [ ] **Step 4: Verify the CI gate is green**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run --workspace`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/skyfire-wasm/tests/transmux_segments.rs crates/skyfire-wasm/Cargo.toml Cargo.lock
git commit -m "test(wasm): CMAF init+media reparse and sample-account against demux"
```

---

### Task 7: Browser — capability gate + MSE fallback + A/V drift corrector

**Files:**
- Modify: `web/player.js` (add MSE path alongside `ensureDecoder`/`pumpVideoInner`)

**Interfaces:**
- Consumes: `bridge.video_codec()`, `bridge.video_config_description()`, new `bridge.video_init_segment()`, `bridge.take_video_media_segment()`; the existing audio clock (`clockUs` used by the subtitle/present loop) as sync master.
- Produces: `chooseVideoPath()` returning `'webcodecs' | 'mse'`; `pumpVideoMse()`; a drift corrector on the `<video>` element.

- [ ] **Step 1: Write the capability gate**

Add near the top of the video section in `web/player.js`:

```js
// Decide the video path once the codec string is known.
let videoPath = null; // 'webcodecs' | 'mse'
async function chooseVideoPath(codec) {
  if (videoPath) return videoPath;
  const cfg = { codec, optimizeForLatency: true };
  let webcodecsOk = false;
  try {
    webcodecsOk = ("VideoDecoder" in window)
      && (await VideoDecoder.isConfigSupported(cfg)).supported === true;
  } catch { webcodecsOk = false; }
  videoPath = webcodecsOk ? "webcodecs" : "mse";
  // Allow forcing MSE for testing: ?video=mse
  if (new URLSearchParams(location.search).get("video") === "mse") videoPath = "mse";
  status(`video path: ${videoPath}`);
  return videoPath;
}
```

- [ ] **Step 2: Implement the MSE path**

Add:

```js
let mediaSource = null, sourceBuffer = null, mseInitAppended = false;
const mseQueue = [];
let videoEl = null; // the <video> element (already in the page for MSE)

function ensureMse(codec) {
  if (mediaSource) return;
  videoEl = document.getElementById("video") || (() => {
    const v = document.createElement("video");
    v.id = "video"; v.muted = true; v.playsInline = true;
    document.body.appendChild(v); return v;
  })();
  mediaSource = new MediaSource();
  videoEl.src = URL.createObjectURL(mediaSource);
  mediaSource.addEventListener("sourceopen", () => {
    const mime = `video/mp4; codecs="${codec}"`;
    if (!MediaSource.isTypeSupported(mime)) { fatal("MSE unsupported", mime); return; }
    sourceBuffer = mediaSource.addSourceBuffer(mime);
    sourceBuffer.mode = "segments";
    sourceBuffer.addEventListener("updateend", flushMseQueue);
    flushMseQueue();
  }, { once: true });
}

function flushMseQueue() {
  if (!sourceBuffer || sourceBuffer.updating) return;
  if (!mseInitAppended) {
    const init = bridge.video_init_segment();
    if (init.length === 0) return;
    mseInitAppended = true;
    sourceBuffer.appendBuffer(init);
    return;
  }
  const seg = mseQueue.shift();
  if (seg) sourceBuffer.appendBuffer(seg);
}

function pumpVideoMse() {
  const cs = bridge.video_codec();
  if (!cs) return;
  ensureMse(cs);
  let seg;
  while ((seg = bridge.take_video_media_segment())) {
    mseQueue.push(seg.bytes);
  }
  flushMseQueue();
  if (videoEl && videoEl.paused && mseInitAppended) videoEl.play().catch(() => {});
}
```

- [ ] **Step 3: Add the A/V drift corrector (audio remains master)**

```js
// Slave the muted <video> timeline to the WASM audio clock.
// clockUs is the audio-master clock in microseconds (same source the
// present loop + subtitles already use).
function correctMseDrift(clockUs) {
  if (videoPath !== "mse" || !videoEl || videoEl.readyState < 2) return;
  const clockS = clockUs / 1e6;
  const drift = videoEl.currentTime - clockS; // +ve → video ahead
  const abs = Math.abs(drift);
  if (abs > 0.25) {           // gross desync → hard seek
    videoEl.currentTime = clockS;
    videoEl.playbackRate = 1.0;
  } else if (abs > 0.05) {    // small drift → gentle rate nudge
    videoEl.playbackRate = drift > 0 ? 0.98 : 1.02;
  } else {
    videoEl.playbackRate = 1.0;
  }
}
```

Call `correctMseDrift(clockUs)` from the existing per-frame/rAF loop that already computes `clockUs` (the same place `updateSubtitles(clockUs)` is called).

- [ ] **Step 4: Route `pumpVideo` through the chosen path**

Change `pumpVideoInner`'s entry so it branches once the codec is known:

```js
async function pumpVideoInner() {
  const cs = bridge.video_codec();
  if (!cs) return;
  const path = await chooseVideoPath(cs);
  if (path === "mse") { pumpVideoMse(); return; }
  // …existing WebCodecs body unchanged below…
  if (!ensureDecoder(cs)) return;
  /* existing take_video_aus loop */
}
```

(Keep the existing WebCodecs loop verbatim after the branch.)

- [ ] **Step 5: Manual smoke (local dev server)**

Run the existing dev server (per repo README / `web/` tooling) and load with `?video=mse`. Confirm in the console: `video path: mse`, no fatal, `<video>` advances. This is a manual check; the automated gate is Task 8.

- [ ] **Step 6: Commit**

```bash
git add web/player.js
git commit -m "feat(web): MSE fMP4 fallback video path + audio-master drift corrector"
```

---

### Task 8: Playwright verification — WebCodecs regression + MSE plays france-2

**Files:**
- Create/Modify: `web/tests/` Playwright spec (mirror the repo's existing e2e harness; see `skyfire-client-built` run-e2e command)

**Interfaces:**
- Consumes: the running web app with a france-2 fixture; `?video=mse` flag.
- Produces: two assertions — WebCodecs path still decodes (regression), MSE path advances `<video>` with bounded A/V drift.

- [ ] **Step 1: Locate the existing e2e harness**

Run: `ls web/tests 2>/dev/null; grep -rn "playwright" web package.json 2>/dev/null | head`
Expected: find the current Playwright config + how france-2 is served. Reuse it; do not invent a new harness.

- [ ] **Step 2: WebCodecs regression test**

Add a spec that loads the app (default path) with the france-2 fixture and asserts frames decode (e.g. `stats.decoded > 0` exposed on `window`, or a canvas pixel check already used by the repo). Expected: PASS — proves Task 3's avcC change did not break WebCodecs decode.

- [ ] **Step 3: MSE path test**

Add a spec that loads with `?video=mse`, waits for `video path: mse` in console, then asserts:
- `document.querySelector('video').currentTime` increases across two samples ~1s apart;
- drift between `<video>.currentTime` and the reported audio clock stays under ~0.25s.

- [ ] **Step 4: Run the suite**

Run the repo's e2e command (from the `skyfire-client-built` memory / `web` scripts). Expected: both specs PASS on desktop Chromium.

- [ ] **Step 5: Record iOS as external-blocked**

Real iOS-17 Safari verification needs a device and is out of CI. Note it in the PR / `docs/OBJECTIVES.md` as external-blocked, consistent with the existing e2e story. Do **not** claim iOS verified.

- [ ] **Step 6: Commit**

```bash
git add web/tests
git commit -m "test(web): playwright — WebCodecs regression + MSE plays france-2"
```

---

## Self-Review

**Spec coverage:**
- Part 1 (broadcast-common migration) → Task 1. ✓
- Part 2 Scope A (annexb + avcC swap; keep h264_reader) → Tasks 2, 3. ✓ (open question 1 resolved: keep h264_reader — transmux does not decode SPS.)
- Part 3 Scope B (init/media bridge API, MSE gate, drift corrector) → Tasks 4, 5, 7. ✓
- Part 4 verification (native fixture, Playwright, iOS external-blocked) → Tasks 6, 8. ✓
- Out-of-scope items (HEVC, AAC, HLS, media-doctor/ts-fix) → not tasked. ✓

**Placeholder scan:** No "TBD"/"handle edge cases". The `>` "plan-time verify" notes are explicit source-check instructions with exact `gh api` commands, not deferred work — each has a concrete fallback.

**Type consistency:** `VideoConfig` fields (`codec`, `description`, `interlaced`, `width`, `height`, `avcc_box`) are defined in Task 3 and consumed in Task 4. `WasmMediaSegment` (`base_media_decode_time`, `bytes`, `sample_count`) defined + consumed in Task 5, reused in Task 6. `video_init_segment`/`take_video_media_segment` names consistent across Tasks 4–8. `transmux` API names (`annexb_to_length_prefixed`, `AVCDecoderConfigurationRecord`, `AVCConfigurationBox`, `TrackSpec`, `CodecConfig::Avc`, `Sample::from_annexb`, `FragmentTrackData`, `build_init_segment`, `build_media_segment`) match the fetched crate source.

**Residual risk (flagged, not a gap):** Task 3 changes verified avcC golden bytes (high-profile ext). Mitigated by re-golden + round-trip (Task 3) and WebCodecs regression (Task 8). Several signatures carry a plan-time source-verify note because docs.rs was not yet built at planning time.
