//! G.711 audio codec — μ-law (PCMU) and A-law (PCMA) encode/decode.
//!
//! Both Twilio and Telnyx use 8 kHz G.711 audio over their WebSocket
//! Media Streams protocol. This module wraps the `audio-codec-algorithms`
//! crate, which is verified against the ITU G.191 software tools.
//!
//! Reference: ITU-T G.711, ITU-T G.191

use audio_codec_algorithms::{decode_alaw, decode_ulaw, encode_alaw, encode_ulaw};

// ── Public batch API ────────────────────────────────────────────

/// Decode a μ-law byte slice to PCM16 samples (little-endian byte output).
pub fn ulaw_to_pcm16(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() * 2);
    for &byte in input {
        let sample = decode_ulaw(byte);
        output.extend_from_slice(&sample.to_le_bytes());
    }
    output
}

/// Encode PCM16 samples (little-endian bytes) to μ-law.
pub fn pcm16_to_ulaw(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() / 2);
    for chunk in input.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        output.push(encode_ulaw(sample));
    }
    output
}

/// Decode an A-law byte slice to PCM16 samples (little-endian byte output).
pub fn alaw_to_pcm16(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() * 2);
    for &byte in input {
        let sample = decode_alaw(byte);
        output.extend_from_slice(&sample.to_le_bytes());
    }
    output
}

/// Encode PCM16 samples (little-endian bytes) to A-law.
pub fn pcm16_to_alaw(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len() / 2);
    for chunk in input.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        output.push(encode_alaw(sample));
    }
    output
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_ulaw_roundtrip() {
        let pcm: Vec<u8> = vec![0x00, 0x04, 0x00, 0xFC]; // Two samples: 1024, -1024
        let ulaw = pcm16_to_ulaw(&pcm);
        let decoded = ulaw_to_pcm16(&ulaw);
        assert_eq!(decoded.len(), pcm.len());
    }

    #[test]
    fn batch_alaw_roundtrip() {
        let pcm: Vec<u8> = vec![0x00, 0x04, 0x00, 0xFC];
        let alaw = pcm16_to_alaw(&pcm);
        let decoded = alaw_to_pcm16(&alaw);
        assert_eq!(decoded.len(), pcm.len());
    }
}
