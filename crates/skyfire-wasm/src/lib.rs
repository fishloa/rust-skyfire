//! WASM bindings for Skyfire.
//!
//! Thin boundary over [`skyfire_core`] for the browser shell in `web/`. The
//! `wasm-bindgen` + `web-sys` glue (WebCodecs `VideoDecoder`, `AudioWorklet`,
//! canvas) is added by the WASM-shell epic; this crate compiles natively today
//! so the workspace CI gate (build/clippy/test) stays green before the wasm
//! target is wired.

/// Engine version, surfaced to the JS shell.
#[must_use]
pub fn engine_version() -> &'static str {
    skyfire_core::version()
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_nonempty() {
        assert!(!super::engine_version().is_empty());
    }
}
