//! Skyfire engine core.
//!
//! Wires the receiver together: [`ts`] demuxes the MPEG-TS into elementary
//! streams + PTS, [`ac3`] decodes AC-3/E-AC-3 audio to PCM, and [`sync`] runs
//! the audio-master clock that the (browser-side WebCodecs) video pipeline
//! chases. The WebCodecs video decode, `AudioWorklet`, and canvas render live
//! in the `web/` shell and are driven via the `skyfire-wasm` bindings.

pub use skyfire_ac3 as ac3;
pub use skyfire_sync as sync;
pub use skyfire_ts as ts;

/// Engine build identifier (crate version).
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn reexports_present() {
        assert_eq!(super::ts::TS_PACKET_LEN, 188);
        assert_eq!(super::ac3::AC3_SYNCWORD, 0x0B77);
    }
}
