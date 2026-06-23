//! MPEG-1/2 Layer II audio decoder for Skyfire.
//!
//! DVB-SD channels carry MPEG-1/2 Layer II (mp2) audio.  This crate provides
//! an incremental (per-access-unit) decoder via the `symphonia` crate's `mpa`
//! feature, mirroring the API shape of `skyfire-ac3::IncrementalDecoder`.
//!
//! Powered by [`symphonia`](https://crates.io/crates/symphonia) (MPL-2.0).

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, CODEC_TYPE_MP2};
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;

/// Decoded MPEG audio: interleaved PCM samples, sample rate, and channel count.
#[derive(Clone, Debug)]
pub struct DecodedMpa {
    /// Interleaved 16-bit signed little-endian PCM samples.
    /// Length = `samples * channels * 2` bytes.
    pub pcm_s16le: Vec<u8>,
    /// Sample rate in Hz (e.g., 48_000).
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
}

/// Stateful MPEG-1/2 Layer II decoder for incremental (per-access-unit) use.
///
/// Holds a `symphonia` MPA decoder instance across calls.  Use one
/// `IncrementalMpaDecoder` per audio PID; call [`reset`](Self::reset) when
/// switching PIDs.
pub struct IncrementalMpaDecoder {
    /// symphonia's MPA decoder (handles MPEG-1/2 Layer I/II/III).
    decoder: Option<Box<dyn Decoder>>,
    /// Cached sample rate from the last decoded frame.
    sample_rate: u32,
    /// Cached channel count from the last decoded frame.
    channels: u16,
}

impl Default for IncrementalMpaDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalMpaDecoder {
    /// Create a new decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            decoder: None,
            sample_rate: 0,
            channels: 0,
        }
    }

    /// Reset the decoder state (call when switching to a new stream / PID).
    pub fn reset(&mut self) {
        self.decoder = None;
        self.sample_rate = 0;
        self.channels = 0;
    }

    /// Decode one access unit's MPEG audio bytes into interleaved S16LE PCM.
    ///
    /// Returns `None` if the access unit does not contain a complete,
    /// decodable mp2 syncframe.  Multiple syncframes in one access unit are
    /// concatenated into the returned PCM.
    ///
    /// # Errors
    ///
    /// Returns an error string if the decoder encounters a malformed frame.
    pub fn decode_au(&mut self, data: &[u8]) -> Result<Option<DecodedMpa>, String> {
        // Lazily initialise decoder on first use.
        if self.decoder.is_none() {
            let codec_registry = symphonia::default::get_codecs();
            let opts = symphonia::core::codecs::DecoderOptions::default();
            let mut params = symphonia::core::codecs::CodecParameters::new();
            params.for_codec(CODEC_TYPE_MP2);
            let decoder = codec_registry
                .make(&params, &opts)
                .map_err(|e| format!("symphonia codec init: {e}"))?;
            self.decoder = Some(decoder);
        }

        let decoder = self.decoder.as_mut().unwrap();

        // Wrap the AU bytes in an MpaReader to demux frames.
        let owned_data = data.to_vec();
        let src = MediaSourceStream::new(
            Box::new(std::io::Cursor::new(owned_data)),
            Default::default(),
        );
        let fmt_opts = FormatOptions::default();
        let mut reader = match symphonia::default::formats::MpaReader::try_new(src, &fmt_opts) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        let mut combined_pcm: Vec<u8> = Vec::new();
        let mut got_audio = false;

        loop {
            let packet = match reader.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    break;
                }
                Err(_) => break,
            };

            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let n_frames = decoded.frames();
                    if n_frames == 0 {
                        continue;
                    }

                    let sr = spec.rate;
                    let ch = spec.channels.count() as u16;

                    self.sample_rate = sr;
                    self.channels = ch;

                    let mut sample_buf = SampleBuffer::<i16>::new(n_frames as u64, spec);
                    sample_buf.copy_interleaved_ref(decoded);
                    let samples = sample_buf.samples();

                    for &s in samples {
                        combined_pcm.extend_from_slice(&s.to_le_bytes());
                    }
                    got_audio = true;
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => {
                    continue;
                }
                Err(e) => return Err(format!("symphonia mpa decode: {e}")),
            }
        }

        if !got_audio {
            return Ok(None);
        }

        Ok(Some(DecodedMpa {
            pcm_s16le: combined_pcm,
            sample_rate: self.sample_rate,
            channels: self.channels,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real MPEG-1 Layer II first frame header extracted from a ffmpeg-generated
    /// 48 kHz, 192 kbps, mono sine tone.  The header is 0xFF FDA4 C4 (MPEG-1 Layer II,
    /// 48 kHz, 192 kbps, mono, no CRC, no padding).
    fn make_mp2_frame() -> Vec<u8> {
        let mut frame = vec![0xFF, 0xFD, 0xA4, 0xC4];
        // Frame size = 576 (4 header + 572 body).
        frame.resize(576, 0u8);
        frame
    }

    #[test]
    fn decode_single_mp2_frame() {
        let mut dec = IncrementalMpaDecoder::new();
        let frame = make_mp2_frame();
        let result = dec.decode_au(&frame).expect("must decode ok");
        assert!(result.is_some(), "must produce PCM");
        let decoded = result.unwrap();
        assert_eq!(decoded.sample_rate, 48000, "sample_rate must be 48000");
        assert_eq!(decoded.channels, 1, "must be mono for this test frame");
        assert!(!decoded.pcm_s16le.is_empty(), "PCM must be non-empty");
        assert_eq!(
            decoded.pcm_s16le.len() % 2,
            0,
            "PCM len must be multiple of 2 (mono S16LE)"
        );
        eprintln!(
            "mp2 test: {} bytes PCM, {} Hz, {} ch",
            decoded.pcm_s16le.len(),
            decoded.sample_rate,
            decoded.channels
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut dec = IncrementalMpaDecoder::new();
        let frame = make_mp2_frame();
        let _ = dec.decode_au(&frame);
        dec.reset();
        let frame2 = make_mp2_frame();
        let result = dec.decode_au(&frame2).expect("decode after reset");
        assert!(result.is_some(), "must decode after reset");
    }
}
