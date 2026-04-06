//! STT providers — speech-to-text backends.
//!
//! # Architecture
//!
//! All providers implement [`SttProvider`], a trait that speaks the same
//! internal protocol regardless of the underlying wire format:
//!
//! ```text
//! reactor ─► feed_audio(pcm) ─► [provider] ─► SttEvent channel ─► reactor
//!                │                                                     │
//!            finalize()                               FirstTextReceived│Transcript
//!                │                                                     │
//!             close()                                               result
//! ```
//!
//! # Providers
//!
//! | Provider | Type | Protocol |
//! |----------|------|----------|
//! | `builtin`          | Streaming WS  | speech-inference WS (protobuf-JSON) |
//! | `deepgram`         | Streaming WS  | Deepgram (`nova-3`, binary PCM) |
//! | `cartesia`         | Streaming WS  | Cartesia Live (`ink-whisper`, binary PCM) |
//! | `openai-realtime`  | Streaming WS  | OpenAI Realtime (`gpt-4o-transcribe`, base64 JSON) |
//! | `openai-whisper`   | Segmented HTTP | OpenAI Whisper (`gpt-4o-transcribe`, multipart) |
//! | `groq`             | Segmented HTTP | Groq Whisper (`whisper-large-v3-turbo`) |
//! | `elevenlabs`       | Segmented HTTP | ElevenLabs (`scribe_v2`, multipart) |

pub mod builtin;
pub mod cartesia;
pub mod deepgram;
pub mod elevenlabs;
pub mod groq;
pub mod openai_realtime;
pub mod openai_whisper;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::types::SttEvent;

// ── SttProvider trait ────────────────────────────────────────────

/// A pluggable STT backend that streams recognition results.
///
/// Implementors spawn their own read/write background tasks on `connect()`
/// and communicate with the Reactor through a `mpsc::Receiver<SttEvent>`.
/// The Reactor polls that receiver in its central `tokio::select!` loop.
#[async_trait]
pub trait SttProvider: Send {
    /// Human-readable provider name for logging / Langfuse labels.
    fn provider_name(&self) -> &str;

    /// Connect to the STT service.
    ///
    /// Spawns background reader/writer tasks and initialises internal channels.
    /// Must be called before `feed_audio()`, `finalize()`, or `recv()`.
    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Feed raw 16 kHz PCM-16 LE audio bytes to the STT backend.
    ///
    /// Called inline on every audio frame — must be non-blocking.
    fn feed_audio(&self, audio: &[u8]);

    /// Ask the backend to finalise the current utterance.
    ///
    /// Non-blocking: the final transcript arrives later via `recv()`.
    fn finalize(&self);

    /// Close the connection / clear buffers.
    ///
    /// - **WS providers**: may send a provider-specific close frame (e.g.
    ///   Deepgram `CloseStream`) or simply drop the channel (Cartesia,
    ///   OpenAI Realtime).
    /// - **Segmented HTTP providers**: drain any buffered audio with a
    ///   warning (audio discarded without transcription).
    fn close(&self);

    /// Take the result receiver out of the provider.
    ///
    /// Called once by `SttStage::connect()` so the Reactor's `select!` loop
    /// can poll it directly without holding a reference to the provider.
    fn take_result_rx(&mut self) -> Option<mpsc::Receiver<SttEvent>>;
}

// ── Factory ──────────────────────────────────────────────────────

/// Configuration for building an STT provider.
///
/// `Debug` is manually implemented to redact `api_key`.
#[derive(Clone)]
pub struct SttProviderConfig {
    /// Provider tag: `"builtin"`, `"deepgram"`, `"cartesia"`, `"openai-realtime"`, `"groq"`.
    pub provider: String,
    /// Base URL (for builtin WebSocket provider).
    pub base_url: String,
    /// API Key (for cloud providers).
    pub api_key: String,
    /// Language code (e.g. `"en"`, `"es"`).
    pub language: String,
    /// Model name (e.g. `"nova-3-general"`, `"whisper-large-v3-turbo"`).
    pub model: String,
}

impl std::fmt::Debug for SttProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SttProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("language", &self.language)
            .field("model", &self.model)
            .finish()
    }
}

/// Build the correct [`SttProvider`] from config.
///
/// Language normalization is applied per-provider:
/// - Deepgram, Cartesia, builtin: receive the full dialect code (e.g. `zh-TW`, `pt-BR`)
///   because their APIs support regional variants natively.
/// - Groq, OpenAI Whisper, OpenAI Realtime, ElevenLabs: receive only the base
///   ISO-639-1 code (e.g. `zh`, `pt`) — these APIs reject or silently ignore
///   dialect suffixes.
pub fn build_stt_provider(cfg: &SttProviderConfig) -> Box<dyn SttProvider> {
    // Base ISO-639-1 code (strips region suffix like "zh-TW" → "zh")
    let base_lang = crate::language_config::normalize_to_base_code(&cfg.language);

    match cfg.provider.as_str() {
        "deepgram" => Box::new(deepgram::DeepgramSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            &cfg.language, // supports full dialect codes
        )),
        "cartesia" => Box::new(cartesia::CartesiaSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            &cfg.language, // supports full dialect codes
        )),
        "openai-realtime" => Box::new(openai_realtime::OpenAIRealtimeSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            base_lang, // ISO-639-1 only
        )),
        "groq" => Box::new(groq::GroqWhisperSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            base_lang, // ISO-639-1 only
        )),
        "openai-whisper" => Box::new(openai_whisper::OpenAiWhisperSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            base_lang, // ISO-639-1 only
        )),
        "elevenlabs" => Box::new(elevenlabs::ElevenLabsSttProvider::new(
            &cfg.api_key,
            &cfg.model,
            base_lang, // ISO-639-1 only
        )),
        _ => {
            // "builtin" or any unrecognised tag → speech-inference WS provider
            Box::new(builtin::BuiltinSttProvider::new(
                &cfg.base_url,
                &cfg.language,
            ))
        }
    }
}

/// Returns the expected P99 latency from `finalize()` to transcript (milliseconds)
/// for a given STT provider.
///
/// Used to set [`SessionConfig::stt_p99_latency_ms`] automatically based on the
/// provider's known characteristics, replacing the single hardcoded default with
/// per-provider conservative estimates of tail latency.
///
/// # Provider characteristics
///
/// Values are conservative engineering estimates based on provider architecture
/// and observed behaviour in production. Adjust if your deployment data differs.
///
/// | Category              | Providers                                   | P99     |
/// |-----------------------|---------------------------------------------|---------|
/// | Streaming WS          | `deepgram`, `cartesia`, `openai-realtime`   | 600 ms  |
/// | Segmented HTTP (fast) | `groq`                                      | 1000 ms |
/// | Segmented HTTP (med)  | `openai-whisper`                            | 1200 ms |
/// | Segmented HTTP (slow) | `elevenlabs`                                | 1500 ms |
/// | builtin / unknown     | all others                                  | 1000 ms |
///
/// Streaming WS providers process audio incrementally and deliver the final
/// transcript quickly after `finalize()`. Segmented HTTP providers upload the
/// full audio chunk and wait for a synchronous inference round-trip, which has
/// noticeably higher tail latency — especially ElevenLabs Scribe which is
/// optimised for accuracy over speed.
pub fn default_stt_p99_latency_ms(provider: &str) -> u32 {
    match provider {
        // Streaming WS — incremental processing, fast finalize → transcript
        "deepgram" | "cartesia" | "openai-realtime" => 600,
        // Segmented HTTP — upload + inference round-trip
        "groq" => 1000,
        "openai-whisper" => 1200,
        "elevenlabs" => 1500,
        // builtin (speech-inference) or unrecognised
        _ => 1000,
    }
}

#[cfg(test)]
mod tests {
    use super::default_stt_p99_latency_ms;

    // Streaming WS providers — fast incremental path.
    #[test]
    fn streaming_ws_providers_are_600ms() {
        for provider in ["deepgram", "cartesia", "openai-realtime"] {
            assert_eq!(
                default_stt_p99_latency_ms(provider),
                600,
                "expected 600ms for {provider}"
            );
        }
    }

    // Segmented HTTP providers — each has its own measured estimate.
    #[test]
    fn segmented_http_providers_match_table() {
        assert_eq!(default_stt_p99_latency_ms("groq"), 1000);
        assert_eq!(default_stt_p99_latency_ms("openai-whisper"), 1200);
        assert_eq!(default_stt_p99_latency_ms("elevenlabs"), 1500);
    }

    // Unknown / builtin providers fall back to the conservative default.
    #[test]
    fn unknown_provider_falls_back_to_1000ms() {
        assert_eq!(default_stt_p99_latency_ms("builtin"), 1000);
        assert_eq!(default_stt_p99_latency_ms(""), 1000);
        assert_eq!(default_stt_p99_latency_ms("some-future-provider"), 1000);
    }

    // Ensure case-sensitive matching — provider names are always lowercase.
    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(default_stt_p99_latency_ms("ElevenLabs"), 1000); // fallback, not 1500
        assert_eq!(default_stt_p99_latency_ms("Deepgram"), 1000); // fallback, not 600
    }
}
