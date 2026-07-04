//! WASM bindings for Skyfire — exposes [`skyfire_core::Engine`] to JavaScript.
//!
//! The `wasm-bindgen` boundary:
//! - Construct an engine (probe channel map, init, feed, flush, finalize).
//! - Pull decoded audio PCM (`Uint8Array`), sample rate, channel count.
//! - Pull H.264 video access units (bytes + PTS) and the WebCodecs config
//!   (codec string + `avcC` description).
//!
//! Data-in/data-out only — no `web-sys` DOM/WebCodecs/AudioWorklet calls.
//! The browser shell in `web/` drives those APIs with the data surfaced here.

mod bridge;
mod bridge_dto;
mod helpers;
mod probe;

pub use bridge::SkyfireBridge;
pub use bridge_dto::{
    WasmAudioTrack, WasmMediaSegment, WasmPcmChunk, WasmSubtitleCue, WasmSubtitleRegion,
    WasmSubtitleTrack, WasmTrackList, WasmVideoAu,
};
pub use probe::{ProbeResult, WasmEngine, WasmVideoUnit};

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;
