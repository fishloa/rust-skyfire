//! AC-3 / E-AC-3 decoder for Skyfire.
//!
//! `WebCodecs` has no AC-3/E-AC-3 audio decoder (this is the gap that killed the
//! old MSE attempt). Audio is light, so a pure-Rust decoder compiled to WASM is
//! cheap: decode to interleaved PCM and push through a `WebAudio` `AudioWorklet`.
//!
//! Powered by [`oxideav-ac3`](https://crates.io/crates/oxideav-ac3) (MIT).

use oxideav_ac3::{decoder, eac3, syncinfo};
use oxideav_core::{CodecId, CodecParameters, Decoder, Error, Frame, Packet, TimeBase};

pub mod downmix;
pub mod header;

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
    /// Sample rate in Hz (e.g., `48_000`).
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
}

// ---------------------------------------------------------------------------
// Incremental decoder
// ---------------------------------------------------------------------------

/// Stateful **AC-3 + E-AC-3** decoder for incremental (per-access-unit) use.
///
/// Wraps `oxideav_ac3::decoder::make_decoder` — the unified decoder that
/// dispatches base AC-3 (bsid ≤ 8) and E-AC-3 (Annex E, bsid 11–16) per
/// syncframe, so both codecs decode. Holds decode state across
/// calls; use one per audio PID and [`reset`](Self::reset) when switching PIDs.
pub struct IncrementalDecoder {
    dec: Box<dyn Decoder>,
}

impl IncrementalDecoder {
    /// Create a new decoder with fresh state.
    ///
    /// # Errors
    ///
    /// Returns an error if the oxideav decoder cannot be constructed (e.g.
    /// unsupported codec parameters).
    pub fn new() -> Result<Self, String> {
        // Codec "ac3": the unified `Ac3Decoder` inspects each packet's bsid and
        // routes base AC-3 vs E-AC-3 itself, so this one decoder handles both.
        let params = CodecParameters::audio(CodecId::new("ac3"));
        let dec =
            decoder::make_decoder(&params).map_err(|e| format!("build ac3/eac3 decoder: {e}"))?;
        Ok(Self { dec })
    }

    /// Reset decode state (call when switching to a new stream / PID).
    pub fn reset(&mut self) {
        // Best-effort rebuild: if the new decoder fails, leave the old one in place.
        if let Ok(dec) = Self::new() {
            *self = dec;
        }
    }

    /// Decode all AC-3 / E-AC-3 syncframes in one access unit's ES bytes.
    ///
    /// Returns the concatenated interleaved-S16LE PCM for all syncframes found,
    /// plus the sample rate and (native) channel count. Returns `None` if
    /// `data` contains no valid syncframes. Bytes that don't form a complete
    /// syncframe are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error string if a syncframe fails to decode.
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
            // bsid (byte 5, top 5 bits) selects the layout: ≤10 = base AC-3,
            // ≥11 = E-AC-3 (Annex E). `syncinfo::parse` is the AC-3 parser and
            // misreads E-AC-3's fscod/frmsiz positions, so branch.
            if offset + 6 > data.len() {
                break;
            }
            let bsid = data[offset + 5] >> 3;
            // bsid ≤ 10 is base AC-3 (A/52 §E.2.3.1.6); 11–16 is Annex E (E-AC-3).
            // NOTE: `header::channels_from_syncframe` (the channel-count probe)
            // deliberately uses a stricter `bsid <= 8` threshold and returns
            // `None` for bsid 9/10, unlike this decode path. That's not a bug —
            // see the cross-reference comment on that function.
            let (frame_len, frame_rate) = if bsid <= 10 {
                if let Ok(si) = syncinfo::parse(&data[offset..]) {
                    (si.frame_length as usize, si.sample_rate)
                } else {
                    offset += 1;
                    continue;
                }
            } else {
                // E-AC-3: frmsiz = byte2[2:0]<<8 | byte3; length = (frmsiz+1)·2.
                // fscod is byte4 top 2 bits (48/44.1/32 kHz; 3 ⇒ fscod2, 48 kHz fallback).
                let frmsiz =
                    ((usize::from(data[offset + 2]) & 0x07) << 8) | usize::from(data[offset + 3]);
                let flen = (frmsiz + 1) * 2;
                let sr = match data[offset + 4] >> 6 {
                    0 => 48_000,
                    1 => 44_100,
                    2 => 32_000,
                    _ => 48_000,
                };
                (flen, sr)
            };
            if frame_len == 0 || offset + frame_len > data.len() {
                break;
            }

            let pkt = Packet::new(
                0,
                TimeBase::new(1, 48_000),
                data[offset..offset + frame_len].to_vec(),
            );
            self.dec.send_packet(&pkt).map_err(|e| e.to_string())?;
            loop {
                match self.dec.receive_frame() {
                    Ok(Frame::Audio(af)) => {
                        // Plane 0 is interleaved S16LE (packed output).
                        if let Some(plane) = af.data.into_iter().next() {
                            if af.samples > 0 {
                                let ch = plane.len() / (af.samples as usize * 2);
                                if ch > 0 {
                                    channels = Some(ch as u16);
                                }
                            }
                            combined_pcm.extend_from_slice(&plane);
                        }
                        sample_rate = Some(frame_rate);
                    }
                    Ok(_) => {}
                    Err(Error::NeedMore | Error::Eof) => break,
                    Err(e) => return Err(e.to_string()),
                }
            }
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
        // Must return some Result (Ok or Err) but never panic.
        assert!(
            result.is_ok() || result.is_err(),
            "truncated frame must not panic"
        );
    }

    #[test]
    fn incremental_decoder_no_panic_on_garbage_ac3() {
        let mut dec = IncrementalDecoder::new().expect("build decoder");
        // Garbage that looks like a base AC-3 frame (bsid ≤ 10).
        let garbage: Vec<u8> = vec![0x0B, 0x77, 0x00, 0x00, 0x00, 0x00];
        let result = dec.decode_au(&garbage);
        assert!(result.is_ok(), "garbage AC-3 must not panic: {result:?}");
    }

    #[test]
    fn incremental_decoder_no_panic_on_truncated_eac3() {
        let mut dec = IncrementalDecoder::new().expect("build decoder");
        // Valid syncword + E-AC-3 bsid (≥11): byte5 top 5 bits = bsid 16 = 0x80
        // but frame length would be bogus.
        let truncated: Vec<u8> = vec![0x0B, 0x77, 0x00, 0x10, 0x00, 0x80];
        let result = dec.decode_au(&truncated);
        assert!(
            result.is_ok(),
            "truncated E-AC-3 must not panic: {result:?}"
        );
    }

    #[test]
    fn incremental_decoder_no_panic_empty_input() {
        let mut dec = IncrementalDecoder::new().expect("build decoder");
        let result = dec.decode_au(&[]);
        assert!(result.is_ok(), "empty input must not panic: {result:?}");
        assert!(result.unwrap().is_none(), "empty input must return None");
    }
}
