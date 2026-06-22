//! AC-3 / E-AC-3 decoder for Skyfire.
//!
//! WebCodecs has no AC-3/E-AC-3 audio decoder (this is the gap that killed the
//! old MSE attempt). Audio is light, so a pure-Rust decoder compiled to WASM is
//! cheap: decode to interleaved PCM and push through a WebAudio `AudioWorklet`.
//!
//! Powered by [`oxideav-ac3`](https://crates.io/crates/oxideav-ac3) (MIT).

use oxideav_ac3::eac3;

/// AC-3 / E-AC-3 sync word (`0x0B77`).
pub const AC3_SYNCWORD: u16 = 0x0B77;

/// True if the buffer begins with an AC-3 / E-AC-3 sync frame.
#[must_use]
pub fn is_ac3_syncframe(buf: &[u8]) -> bool {
    buf.len() >= 2 && (u16::from(buf[0]) << 8 | u16::from(buf[1])) == AC3_SYNCWORD
}

/// Decoded E-AC-3 audio: interleaved PCM samples, sample rate, and channel count.
#[derive(Clone, Debug)]
pub struct DecodedAudio {
    /// Interleaved 16-bit signed little-endian PCM samples.
    /// Length = `samples * channels * 2` bytes.
    pub pcm_s16le: Vec<u8>,
    /// Sample rate in Hz (e.g., 48_000).
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
}

/// Decode a single E-AC-3 syncframe (packet) into interleaved PCM.
///
/// `data` must contain one or more concatenated E-AC-3 syncframes
/// starting with the `0x0B77` syncword.  The `state` persists
/// IMDCT overlap-add history across calls.
///
/// # Errors
///
/// Returns an error if the packet is malformed or the decoder hits an
/// unsupported feature.
pub fn decode_eac3_packet(
    state: &mut eac3::Eac3DecoderState,
    data: &[u8],
) -> Result<DecodedAudio, String> {
    let frame = eac3::decode_eac3_packet(state, data).map_err(|e| e.to_string())?;
    Ok(DecodedAudio {
        pcm_s16le: frame.pcm_s16le,
        sample_rate: frame.sample_rate,
        channels: frame.channels,
    })
}

// ---------------------------------------------------------------------------
// Incremental decoder
// ---------------------------------------------------------------------------

/// Stateful E-AC-3/AC-3 decoder for incremental (per-access-unit) use.
///
/// Holds the IMDCT overlap-add state across calls so that the codec has
/// correct history at AU boundaries.  Use one `IncrementalDecoder` per audio
/// PID; call [`reset`](Self::reset) when switching PIDs.
pub struct IncrementalDecoder {
    state: eac3::Eac3DecoderState,
}

impl Default for IncrementalDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalDecoder {
    /// Create a new decoder with a fresh state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: eac3::Eac3DecoderState::default(),
        }
    }

    /// Reset the IMDCT history (call when switching to a new stream / PID).
    pub fn reset(&mut self) {
        self.state = eac3::Eac3DecoderState::default();
    }

    /// Decode all E-AC-3 syncframes in one access unit's ES bytes.
    ///
    /// Returns the concatenated PCM for all syncframes found, plus the sample
    /// rate and channel count (constant within a stream, taken from the last
    /// syncframe decoded).  Returns `None` if `data` contains no valid
    /// syncframes.
    ///
    /// Any bytes that don't form a complete syncframe are silently skipped
    /// (consistent with [`decode_all_eac3`]).
    ///
    /// # Errors
    ///
    /// Returns an error string if any syncframe fails to decode.
    pub fn decode_au(&mut self, data: &[u8]) -> Result<Option<DecodedAudio>, String> {
        let mut combined_pcm: Vec<u8> = Vec::new();
        let mut sample_rate: Option<u32> = None;
        let mut channels: Option<u16> = None;

        let mut offset = 0;
        while offset + 6 <= data.len() {
            if !is_ac3_syncframe(&data[offset..]) {
                offset += 1;
                continue;
            }
            let b2 = u16::from(data[offset + 2]);
            let b3 = u16::from(data[offset + 3]);
            let frmsiz = ((b2 & 0x07) << 8) | b3;
            let frame_len = ((frmsiz as usize) + 1) * 2;
            if offset + frame_len > data.len() {
                break;
            }
            let frame =
                eac3::decode_eac3_packet(&mut self.state, &data[offset..offset + frame_len])
                    .map_err(|e| e.to_string())?;
            sample_rate = Some(frame.sample_rate);
            channels = Some(frame.channels);
            combined_pcm.extend_from_slice(&frame.pcm_s16le);
            offset += frame_len;
        }

        if combined_pcm.is_empty() {
            return Ok(None);
        }

        Ok(Some(DecodedAudio {
            pcm_s16le: combined_pcm,
            sample_rate: sample_rate.unwrap_or(0),
            channels: channels.unwrap_or(0),
        }))
    }
}

/// Decode all E-AC-3 syncframes in `data` and return the concatenated
/// interleaved PCM.
///
/// Convenience wrapper — creates a fresh decoder state, walks the input
/// as individual syncframes located by the BSI `frame_bytes` field, and
/// concatenates output.  Any trailing bytes that don't form a complete
/// syncframe are silently dropped (no panic).
///
/// # Errors
///
/// Returns an error if any syncframe fails to decode.
pub fn decode_all_eac3(data: &[u8]) -> Result<DecodedAudio, String> {
    let mut state = eac3::Eac3DecoderState::default();
    let mut combined_pcm: Vec<u8> = Vec::new();
    let mut sample_rate: Option<u32> = None;
    let mut channels: Option<u16> = None;

    let mut offset = 0;
    while offset + 6 <= data.len() {
        // Must start with syncword
        if !is_ac3_syncframe(&data[offset..]) {
            offset += 1;
            continue;
        }
        // Read frmsiz from E-AC-3 header: byte 2 low 3 bits << 8 | byte 3
        let b2 = u16::from(data[offset + 2]);
        let b3 = u16::from(data[offset + 3]);
        let frmsiz = ((b2 & 0x07) << 8) | b3;
        let frame_len = ((frmsiz as usize) + 1) * 2;
        if offset + frame_len > data.len() {
            // Truncated final frame — stop gracefully
            break;
        }
        let frame = eac3::decode_eac3_packet(&mut state, &data[offset..offset + frame_len])
            .map_err(|e| e.to_string())?;
        sample_rate = Some(frame.sample_rate);
        channels = Some(frame.channels);
        combined_pcm.extend_from_slice(&frame.pcm_s16le);
        offset += frame_len;
    }

    Ok(DecodedAudio {
        pcm_s16le: combined_pcm,
        sample_rate: sample_rate.unwrap_or(0),
        channels: channels.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_syncword() {
        assert!(is_ac3_syncframe(&[0x0B, 0x77, 0x00]));
        assert!(!is_ac3_syncframe(&[0x47, 0x00]));
    }

    #[test]
    fn decode_gulli_eac3_fixture() {
        const FIXTURE: &[u8] = include_bytes!("../../../fixtures/gulli.eac3");

        // The fixture starts with 0x0B77
        assert!(is_ac3_syncframe(FIXTURE));

        let audio = decode_all_eac3(FIXTURE).expect("decode gulli.eac3");

        // 48 kHz stereo
        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.channels, 2);

        // PCM must exist and have the right shape
        let bytes_per_sample = 2u16; // S16LE
        assert!(audio.pcm_s16le.len() >= 2);
        let total_bytes = audio.pcm_s16le.len();
        assert_eq!(
            total_bytes % (bytes_per_sample as usize * audio.channels as usize),
            0,
            "PCM buffer length must be a multiple of channels × bytes_per_sample"
        );

        let sample_count = total_bytes / (bytes_per_sample as usize * audio.channels as usize);

        // ~15 s of 48 kHz stereo → ~720,000 samples per channel; set a
        // conservative lower bound that would catch a trivial one-frame decode.
        assert!(
            sample_count >= 150_000,
            "expected >= 150_000 samples per channel for ~15 s, got {sample_count}"
        );

        // Interpret as i16 interleaved (safe byte-level decode)
        let pcm_i16: Vec<i16> = audio
            .pcm_s16le
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        // i16 can't represent NaN/Inf — the check here is about
        // whether the decode produced non-zero samples.
        let all_silent = pcm_i16.iter().all(|&s| s == 0);
        assert!(
            !all_silent,
            "decoded PCM must not be all-silence — decoder may have failed silently"
        );

        // At least 1% of samples must be non-zero
        let non_silent = pcm_i16.iter().filter(|&&s| s != 0).count();
        assert!(
            non_silent > sample_count / 100,
            "too few non-silent samples: {non_silent} / {sample_count}"
        );
    }

    #[test]
    fn truncated_frame_no_panic() {
        // A valid syncword followed by garbage that's too short to form
        // a complete frame — the decoder must not panic.
        let truncated = [0x0B, 0x77, 0x00, 0xFF, 0x3F, 0xC1, 0x02];
        let result = decode_all_eac3(&truncated);
        // Should either succeed (if the truncated data happens to look
        // like a valid frame) or return an error — but never panic.
        let _ = result;
    }
}
