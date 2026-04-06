//! Builtin TTS provider — HTTP client to a TTS server.
//!
//! Provider-agnostic: responses can be WAV or raw PCM at any sample rate.
//! Audio is always resampled to the configured `output_sample_rate` before
//! reaching the WebRTC transport, so the frontend never sees a rate mismatch.
//!
//! When the server returns raw PCM without a WAV header, the native sample
//! rate is read from `BuiltinTtsProvider::native_rate` (default: 24 kHz).
//! If the builtin server is ever reconfigured to a different rate, pass the
//! correct value to `BuiltinTtsProvider::with_native_rate()` so the resampler
//! uses the right ratio instead of silently pitch-shifting the audio.

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, warn};

use super::TtsProvider;

// ── Audio decoding helpers ────────────────────────────────────────

/// Parsed audio with its native sample rate.
pub(super) struct DecodedAudio {
    pub(super) pcm: Vec<u8>,     // Raw PCM 16-bit LE mono
    pub(super) sample_rate: u32, // Native rate from the TTS server
}

/// Detect audio format and decode to raw PCM + native sample rate.
///
/// `assumed_rate` is used when the response is raw PCM (no WAV header).
/// WAV responses are self-describing and do not use this value.
fn decode_audio(raw: &[u8], assumed_rate: u32) -> Option<DecodedAudio> {
    if raw.len() > 44 && &raw[..4] == b"RIFF" && &raw[8..12] == b"WAVE" {
        parse_wav(raw)
    } else if !raw.is_empty() {
        debug!(
            "[builtin-tts] Response has no WAV header — assuming {}Hz raw PCM ({} bytes). \
             Set native_rate on BuiltinTtsProvider if the server rate differs.",
            assumed_rate,
            raw.len()
        );
        Some(DecodedAudio {
            pcm: raw.to_vec(),
            sample_rate: assumed_rate,
        })
    } else {
        None
    }
}

pub(super) fn parse_wav(wav: &[u8]) -> Option<DecodedAudio> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }

    let mut pos = 12;
    let mut sample_rate = 0u32;

    while pos + 8 <= wav.len() {
        let chunk_id = &wav[pos..pos + 4];
        let chunk_size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().ok()?) as usize;

        if chunk_id == b"fmt " && chunk_size >= 16 {
            sample_rate = u32::from_le_bytes(wav[pos + 12..pos + 16].try_into().ok()?);
        } else if chunk_id == b"data" {
            if sample_rate == 0 {
                warn!("WAV 'data' chunk found before 'fmt ' — cannot determine sample rate");
                return None;
            }
            let end = (pos + 8 + chunk_size).min(wav.len());
            return Some(DecodedAudio {
                pcm: wav[pos + 8..end].to_vec(),
                sample_rate,
            });
        }

        pos += 8 + chunk_size;
        // WAV chunks are word-aligned
        if !chunk_size.is_multiple_of(2) {
            pos += 1;
        }
    }
    warn!("No 'data' chunk found in WAV");
    None
}

// ── BuiltinTtsProvider ───────────────────────────────────────────

/// HTTP-based TTS client (speech-inference / fish-speech / kokoro).
pub struct BuiltinTtsProvider {
    client: Client,
    base_url: String,
    output_sample_rate: u32,
    /// Sample rate assumed for raw-PCM responses (no WAV header).
    /// Defaults to 24 kHz — the rate used by fish-speech and kokoro.
    /// Override with [`Self::with_native_rate`] if the server is configured differently.
    native_rate: u32,
    resampler: Option<soxr::SoxrStreamResampler>,
}

impl BuiltinTtsProvider {
    /// Create a provider assuming the builtin server returns 24 kHz raw PCM.
    pub fn new(base_url: &str, output_sample_rate: u32) -> Self {
        Self::with_native_rate(base_url, output_sample_rate, 24_000)
    }

    /// Create a provider with an explicit native sample rate for raw-PCM responses.
    ///
    /// Use this when the builtin server is configured to a rate other than 24 kHz
    /// (e.g. 16 kHz for a lightweight kokoro variant).
    pub fn with_native_rate(base_url: &str, output_sample_rate: u32, native_rate: u32) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            output_sample_rate,
            native_rate,
            resampler: None,
        }
    }
}

#[async_trait]
impl TtsProvider for BuiltinTtsProvider {
    fn provider_name(&self) -> &str {
        "builtin"
    }

    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>> {
        let mut payload = serde_json::json!({ "text": text });
        if !voice_id.is_empty() && voice_id != "default" {
            payload["reference_id"] = serde_json::json!(voice_id);
        }

        let response = self
            .client
            .post(format!("{}/v1/tts", self.base_url))
            .json(&payload)
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            warn!("TTS error: status {}", response.status());
            return None;
        }

        let raw = response.bytes().await.ok()?.to_vec();
        debug!(
            "TTS response: {} bytes (starts with {:?})",
            raw.len(),
            &raw[..4.min(raw.len())]
        );
        let decoded = decode_audio(&raw, self.native_rate)?;

        if decoded.sample_rate != self.output_sample_rate {
            let resampler = self.resampler.get_or_insert_with(|| {
                soxr::SoxrStreamResampler::new(decoded.sample_rate, self.output_sample_rate)
                    .expect("Failed to create SoxrStreamResampler for TTS")
            });
            Some(resampler.process(&decoded.pcm))
        } else {
            Some(decoded.pcm)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wav(sample_rate: u32, pcm: &[u8]) -> Vec<u8> {
        let data_size = pcm.len() as u32;
        let file_size = 36 + data_size;
        let mut wav = Vec::with_capacity(44 + pcm.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm);
        wav
    }

    #[test]
    fn test_parse_wav_extracts_sample_rate() {
        let pcm = vec![0u8; 100];
        let wav = make_wav(44100, &pcm);
        let decoded = parse_wav(&wav).unwrap();
        assert_eq!(decoded.sample_rate, 44100);
        assert_eq!(decoded.pcm.len(), 100);
    }

    #[test]
    fn test_parse_wav_24khz() {
        let pcm = vec![0u8; 200];
        let wav = make_wav(24000, &pcm);
        let decoded = parse_wav(&wav).unwrap();
        assert_eq!(decoded.sample_rate, 24000);
        assert_eq!(decoded.pcm.len(), 200);
    }

    #[test]
    fn test_decode_raw_pcm_defaults_to_24khz() {
        let raw = vec![0u8; 100];
        let decoded = decode_audio(&raw, 24_000).unwrap();
        assert_eq!(decoded.sample_rate, 24_000);
        assert_eq!(decoded.pcm.len(), 100);
    }

    #[test]
    fn test_decode_raw_pcm_uses_assumed_rate() {
        let raw = vec![0u8; 100];
        let decoded = decode_audio(&raw, 16_000).unwrap();
        assert_eq!(decoded.sample_rate, 16_000);
    }

    #[test]
    fn test_decode_empty_returns_none() {
        assert!(decode_audio(&[], 24_000).is_none());
    }
}
