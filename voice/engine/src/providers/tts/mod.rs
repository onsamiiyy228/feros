//! TTS providers — text-to-speech backends.
//!
//! # Architecture
//!
//! Two provider models are supported:
//!
//! **HTTP pull** (`TtsProvider`): `TtsStage` calls `synthesize_chunk()` per sentence,
//! receives PCM bytes in return.  Simple but pays a new TCP/TLS per sentence.
//!
//! **WebSocket streaming** (`TtsStreamingProvider`): one persistent WS per session.
//! `TtsStage` sends text batches via `send_text()` and receives audio frames
//! back through a channel.  Latency savings: ~50–200 ms per sentence avoided.
//!
//! `TtsStage::start()` accepts a `TtsMode` enum that selects the path.
//!
//! # Providers
//!
//! | Provider | Mode | Protocol |
//! |----------|------|----------|
//! | `builtin`     | HTTP  | speech-inference HTTP POST `/v1/tts` |
//! | `cartesia`    | HTTP  | Cartesia REST |
//! | `cartesia-ws` | WS    | Cartesia WS (context-id multiplexing, base64 audio) |
//! | `deepgram-ws` | WS    | Deepgram Aura WS (binary PCM frames, Flush/Clear) |
//! | `elevenlabs`  | HTTP  | ElevenLabs REST |
//! | `elevenlabs-ws` | WS  | ElevenLabs multi-stream-input WS (token streaming) |
//! | `openai`      | HTTP  | OpenAI TTS REST |
//! | `deepgram`    | HTTP  | Deepgram Aura REST |
//! | `groq`        | HTTP  | Groq PlayAI TTS REST |

pub mod builtin;
pub mod cartesia;
pub mod cartesia_ws;
pub mod deepgram;
pub mod deepgram_ws;
pub mod elevenlabs;
pub mod elevenlabs_ws;
pub mod groq;
pub mod openai;

use async_trait::async_trait;
use tokio::sync::mpsc;

// ── HTTP pull trait ───────────────────────────────────────────────────────────

/// A pluggable HTTP-based TTS backend (one request per sentence).
///
/// Each call to `synthesize_chunk` makes an HTTP request and blocks until
/// the audio bytes are available.  Latency-sensitive paths should prefer
/// `TtsStreamingProvider` (persistent WebSocket) instead.
///
/// Only `Send` is required: the provider is moved into and exclusively
/// owned by the `TtsStage` batching task.
#[async_trait]
pub trait TtsProvider: Send {
    fn provider_name(&self) -> &str;

    /// Synthesize `text` and return resampled PCM-16 LE mono bytes.
    ///
    /// Returns `None` on transient errors — the stage logs and continues.
    async fn synthesize_chunk(&mut self, text: &str, voice_id: &str) -> Option<Vec<u8>>;
}

// ── WebSocket streaming trait ─────────────────────────────────────────────────

/// An audio chunk from the TTS WebSocket: raw PCM-16 LE, already at the
/// pipeline sample rate.
#[derive(Debug)]
pub struct TtsAudioChunk {
    pub pcm: Vec<u8>,
    /// True when this is the last chunk for this turn (flush complete or error).
    pub is_final: bool,
    /// Context ID for multiplexed providers (Cartesia, ElevenLabs).
    /// Empty string for providers that don't multiplex contexts (Deepgram).
    /// The reactor uses this to route audio to the correct active turn.
    pub context_id: String,
}

/// A pluggable WebSocket-based TTS backend (one persistent connection per session).
///
/// The stage calls `connect()` once, then streams text via `send_text()` +
/// `flush()`.  Audio arrives asynchronously through the `audio_rx` channel.
/// `cancel()` sends a provider-specific interrupt signal (barge-in).
#[async_trait]
pub trait TtsStreamingProvider: Send {
    fn provider_name(&self) -> &str;

    /// Set the voice ID for this session.  Called once before `send_text()`.
    ///
    /// Providers that encode the voice per-message (e.g. Cartesia WS) use this;
    /// providers that encode it in the URL at connect time may ignore it.
    /// Default: no-op.
    fn set_voice(&mut self, _voice_id: &str) {}

    /// Open the persistent WebSocket connection.
    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Send a text chunk for the given context (a UUID string for this turn).
    ///
    /// Non-blocking: enqueues into the writer task.
    async fn send_text(&mut self, text: &str, context_id: &str);

    /// Signal end-of-turn for `context_id` — ask the server to flush all
    /// remaining audio.
    async fn flush(&mut self, context_id: &str);

    /// Cancel (interrupt / barge-in) the given context.
    async fn cancel(&mut self, context_id: &str);

    /// Close the WebSocket cleanly.
    async fn close(&mut self);

    /// Take the audio receiver out of the provider (called once after `connect`).
    fn take_audio_rx(&mut self) -> Option<mpsc::Receiver<TtsAudioChunk>>;
}

// ── Provider mode ─────────────────────────────────────────────────────────────

/// Selects which TTS path `TtsStage` uses.
pub enum TtsMode {
    /// Legacy HTTP pull — one request per sentence batch.
    Http(Box<dyn TtsProvider>),
    /// WebSocket streaming — persistent connection, lower latency.
    Streaming(Box<dyn TtsStreamingProvider>),
}

// ── Factory ──────────────────────────────────────────────────────

/// Configuration for building a TTS provider.
///
/// `Debug` is manually implemented to redact `api_key`.
#[derive(Clone)]
pub struct TtsProviderConfig {
    /// Provider tag: `"builtin"`, `"cartesia"`, `"elevenlabs"`, `"openai"`, `"deepgram"`, `"groq"`.
    pub provider: String,
    /// Base URL (for builtin HTTP provider).
    pub base_url: String,
    /// API Key (for cloud providers).
    pub api_key: String,
    /// Model name (e.g. `"sonic-english"`, `"tts-1"`, `"eleven_turbo_v2_5"`).
    pub model: String,
    /// Output sample rate the pipeline expects (e.g. 24000, 8000).
    pub output_sample_rate: u32,
    /// ISO 639-1 base language code (e.g. `"en"`, `"es"`, `"zh"`).
    ///
    /// Sourced from `SessionConfig.language` which comes from the agent graph.
    /// Passed through to providers that support language parameters:
    ///   - ElevenLabs WS/HTTP: `&language_code=<lang>` on multilingual models only
    ///   - Cartesia WS/HTTP: `"language"` field in JSON payload
    pub language: String,
    /// Voice ID for providers where voice and model are separate.
    ///
    /// For Deepgram Aura the voice IS the API model parameter
    /// (e.g. `"aura-2-thalia-en"`). When set, it overrides `model` for the
    /// Deepgram providers. For all others it is forwarded via `synthesize_chunk`.
    pub voice_id: String,
}

impl std::fmt::Debug for TtsProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TtsProviderConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("output_sample_rate", &self.output_sample_rate)
            .field("language", &self.language)
            .finish()
    }
}

/// Build the correct provider from config, returning a `TtsMode`.
pub fn build_tts_provider(cfg: &TtsProviderConfig) -> TtsMode {
    match cfg.provider.as_str() {
        // ── WebSocket streaming providers ─────────────────────────────────────
        "cartesia-ws" => TtsMode::Streaming(Box::new(cartesia_ws::CartesiaWsTtsProvider::new(
            &cfg.api_key,
            &cfg.model,
            cfg.output_sample_rate,
            &cfg.language,
        ))),
        "deepgram-ws" => TtsMode::Streaming(Box::new(deepgram_ws::DeepgramWsTtsProvider::new(
            &cfg.api_key,
            // For Deepgram, voice_id IS the API model param (e.g. "aura-2-thalia-en").
            // Fall back to model, then to the hardcoded default inside the constructor.
            if !cfg.voice_id.is_empty() {
                &cfg.voice_id
            } else {
                &cfg.model
            },
            cfg.output_sample_rate,
        ))),
        "elevenlabs-ws" => {
            TtsMode::Streaming(Box::new(elevenlabs_ws::ElevenLabsWsTtsProvider::new(
                &cfg.api_key,
                &cfg.model,
                cfg.output_sample_rate,
                &cfg.language,
            )))
        }
        // ── HTTP pull providers ───────────────────────────────────────────────
        "cartesia" => TtsMode::Http(Box::new(cartesia::CartesiaTtsProvider::new(
            &cfg.api_key,
            &cfg.model,
            cfg.output_sample_rate,
            &cfg.language,
        ))),
        "elevenlabs" => TtsMode::Http(Box::new(elevenlabs::ElevenLabsTtsProvider::new(
            &cfg.api_key,
            &cfg.model,
            cfg.output_sample_rate,
            &cfg.language,
        ))),
        "openai" => TtsMode::Http(Box::new(openai::OpenAiTtsProvider::new(
            &cfg.api_key,
            &cfg.model,
            cfg.output_sample_rate,
        ))),
        "deepgram" => TtsMode::Http(Box::new(deepgram::DeepgramAuraTtsProvider::new(
            &cfg.api_key,
            // For Deepgram, voice_id IS the API model param (e.g. "aura-2-thalia-en").
            // Fall back to model, then to the hardcoded default inside the constructor.
            if !cfg.voice_id.is_empty() {
                &cfg.voice_id
            } else {
                &cfg.model
            },
            cfg.output_sample_rate,
        ))),
        "groq" => TtsMode::Http(Box::new(groq::GroqTtsProvider::new(
            &cfg.api_key,
            &cfg.model,
            cfg.output_sample_rate,
        ))),
        _ => {
            // "builtin" or any unrecognised tag → speech-inference HTTP provider
            TtsMode::Http(Box::new(builtin::BuiltinTtsProvider::new(
                &cfg.base_url,
                cfg.output_sample_rate,
            )))
        }
    }
}
