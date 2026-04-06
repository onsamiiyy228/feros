//! VoiceSession — thin coordinator that spawns the Reactor.
//!
//! One VoiceSession per active call. Creates channels, starts the Reactor,
//! and exposes a minimal API for the transport handler.

use std::sync::Arc;

use agent_kit::swarm::AgentGraphDef;
use tracing::info;
use voice_trace::Tracer;
use voice_transport::TransportHandle;

use crate::audio_ml::denoiser::DenoiserBackend;
use crate::providers::stt::SttProviderConfig;
use crate::providers::tts::TtsProviderConfig;
use crate::reactor::Reactor;
use crate::settings::AgentTaskSettings;
use agent_kit::agent_backends::SharedSecretMap;
use agent_kit::providers::LlmProvider;
use agent_kit::RecordingConfig;
use agent_kit::{
    AgentBackend, AgentBackendConfig, ArtifactInterceptor, ArtifactStore, DefaultAgentBackend,
};

// ── Configuration ───────────────────────────────────────────────

/// Configuration for a voice session.
#[derive(Clone)]
pub struct SessionConfig {
    pub agent_id: String,
    pub system_prompt: String,
    pub voice_id: String,
    pub language: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub input_sample_rate: u32,
    /// Output sample rate for TTS audio (e.g. 24000 for browser, 8000 for telephony).
    pub output_sample_rate: u32,
    pub greeting: Option<String>,
    /// Path to models directory. Default: `./dsp_models`
    pub models_dir: String,
    /// Smart turn threshold. Default: 0.5
    pub smart_turn_threshold: f32,
    /// Enable denoiser. Default: true
    pub denoise_enabled: bool,
    /// Which denoiser backend to use. Default: RNNoise
    pub denoise_backend: DenoiserBackend,
    /// Enable smart turn. Default: true
    pub smart_turn_enabled: bool,

    // ── Turn Wisdom features ────────────────────────────────
    /// Enable LLM turn completion marker detection (✓/○/◐).
    /// When enabled, the Reactor intercepts the first LLM token to detect
    /// turn completion markers and routes accordingly.
    /// Default: true
    pub turn_completion_enabled: bool,

    /// Seconds of user silence before sending a re-engagement prompt.
    /// Set to 0 to disable idle detection. Default: 4.
    pub idle_timeout_secs: u32,

    /// Number of idle nudges to send before hanging up.
    /// Nudge 1, nudge 2, … nudge N → hang up. Default: 2.
    pub idle_max_nudges: u32,

    /// Minimum number of STT words required before a barge-in is
    /// triggered during bot speech. Prevents filler words ("um", "uh")
    /// from interrupting the bot.
    /// Set to 0 to disable (any VAD triggers barge-in).
    /// Default: 2
    pub min_barge_in_words: u32,

    /// Maximum milliseconds to wait after SpeechStarted during bot speech
    /// before triggering barge-in regardless of word count.
    /// If the user is still speaking after this timeout, barge-in fires
    /// immediately without waiting for STT. SpeechEnded before timeout
    /// falls back to full-transcript word count check.
    /// Default: 800
    pub barge_in_timeout_ms: u32,

    /// Expected P99 latency from speech end to final STT transcript (ms).
    /// After the turn-complete decision (SmartTurn prediction, EoT fallback,
    /// or immediate when SmartTurn is disabled), we wait up to this long for
    /// the STT transcript before abandoning the turn.
    /// Applies to **all** turn-commit paths, not just SmartTurn.
    /// Default: 1000 (conservative for Whisper-class STT)
    pub stt_p99_latency_ms: u32,

    // ── Tool execution ──────────────────────────────────────────
    /// Optional agent graph for multi-node / swarm routing.
    /// When set, the Reactor uses swarm mode with `transfer_to` edges.
    /// Tools are defined as QuickJS scripts in the graph's `tools` map.
    pub agent_graph: Option<AgentGraphDef>,

    // ── Observability labels ────────────────────────────────────
    /// STT provider name for observability labels (e.g. "faster-whisper", "deepgram").
    pub stt_provider: String,
    /// STT model name for observability labels (e.g. "large-v3", "nova-3").
    pub stt_model: String,
    /// TTS provider name for observability labels (e.g. "fish-speech", "kokoro").
    pub tts_provider: String,
    /// TTS model name for observability labels (e.g. "aura-asteria-en").
    pub tts_model: String,

    // ── Session Recording ───────────────────────────────────────
    /// Configuration for session recording (audio + transcript capture).
    /// When `enabled`, spawns a recording subscriber on the event bus.
    pub recording: RecordingConfig,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("agent_id", &self.agent_id)
            .field("system_prompt", &"<redacted>")
            .field("voice_id", &self.voice_id)
            .field("language", &self.language)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("input_sample_rate", &self.input_sample_rate)
            .field("output_sample_rate", &self.output_sample_rate)
            .field("greeting", &self.greeting.as_deref().map(|_| "<set>"))
            .field("agent_graph", &self.agent_graph.as_ref().map(|_| "<set>"))
            .finish()
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            system_prompt: "You are a helpful voice assistant.".to_string(),
            voice_id: "default".to_string(),
            language: "en".to_string(),
            temperature: 0.7,
            max_tokens: 32768,
            input_sample_rate: 48_000,
            output_sample_rate: 24_000,
            greeting: None,
            models_dir: "./dsp_models".to_string(),
            smart_turn_threshold: 0.5,
            denoise_enabled: true,
            denoise_backend: DenoiserBackend::default(),
            smart_turn_enabled: true,
            turn_completion_enabled: true,
            idle_timeout_secs: 4,
            idle_max_nudges: 2,
            min_barge_in_words: 2,
            barge_in_timeout_ms: 800,
            stt_p99_latency_ms: 1000,
            agent_graph: None,
            stt_provider: "unknown".to_string(),
            stt_model: String::new(),
            tts_provider: "unknown".to_string(),
            tts_model: String::new(),
            recording: RecordingConfig::default(),
        }
    }
}

impl SessionConfig {
    /// Apply derived mutations that depend on multiple fields.
    ///
    /// Call once after all fields are set, before passing to [`VoiceSession::start`].
    /// Currently injects a language-instruction prefix on `system_prompt` for
    /// non-English sessions so the LLM responds in the configured language.
    pub fn finalize(mut self) -> Self {
        if !self.language.is_empty() && self.language != "en" {
            use crate::language_config::SUPPORTED_LANGUAGES;
            let lang_label = SUPPORTED_LANGUAGES
                .iter()
                .find(|l| l.code == self.language.as_str())
                .map(|l| l.label)
                .unwrap_or(self.language.as_str());
            let instruction = format!(
                "IMPORTANT: Always respond in {lang_label}. \
                 Never switch languages regardless of what language the user speaks.\n\n"
            );
            self.system_prompt = format!("{instruction}{}", self.system_prompt);
        }
        self
    }
}

// ── Voice Session ───────────────────────────────────────────────

/// A single voice conversation session backed by the Reactor.
///
/// This is a thin coordinator that:
/// 1. Accepts a [`TransportHandle`] for transport-agnostic audio I/O
/// 2. Constructs the [`Reactor`] with all stages and a [`Tracer`]
/// 3. Calls `reactor.start()` to load models and connect STT
/// 4. Spawns the Reactor as a single Tokio task
/// 5. Exposes `close()` and `wait_for_completion()` for lifecycle management
pub struct VoiceSession {
    /// Join handle for the Reactor task.
    reactor_handle: Option<tokio::task::JoinHandle<()>>,

    /// Keeps the transport alive (background tasks are aborted on drop).
    _transport: TransportHandle,
}

impl VoiceSession {
    /// Create and start a new voice session.
    ///
    /// The caller must provide:
    /// - A [`TransportHandle`] from the transport layer (WebSocket, WebRTC, etc.)
    /// - A [`Tracer`] for event bus and observability
    /// - Provider URLs and an LLM provider
    ///
    /// **Recording is the caller's responsibility.** Subscribe to the Tracer
    /// (via `tracer.subscribe()` or `voice_trace::spawn_recording_subscriber`)
    /// *before* calling this method. Voice-engine only emits `UserAudio`,
    /// `Audio`, and `SessionEnded` events — it does not write any files.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        _session_id: String,
        config: SessionConfig,
        secrets: SharedSecretMap,
        llm_provider: Box<dyn LlmProvider>,
        stt_config: SttProviderConfig,
        tts_config: TtsProviderConfig,
        tracer: Tracer,
        mut transport: TransportHandle,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if tts_config.voice_id.is_empty() {
            return Err("A voice must be configured to start the session.".into());
        }

        // The transport's audio_rx goes directly to the Reactor.
        // When the transport closes, audio_rx returns None → Reactor stops.
        let audio_in_rx = transport.take_audio_rx();

        let agent_graph = config.agent_graph.clone();
        let task = AgentTaskSettings::get();
        let backend_config = AgentBackendConfig {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            max_tool_rounds: 5,
            secrets,
            tool_summarizer: task.agent_tool_summarizer,
            context_summarizer: task.agent_context_summarizer,
            tool_filler: task.agent_tool_filler,
        };

        // Convert the boxed provider to an Arc so it can be shared with the hooks
        let llm_provider: Arc<dyn LlmProvider> = Arc::from(llm_provider);

        // Build backend with artifact hook.
        // Summarizers are wired automatically by DefaultAgentBackend::new()
        // using the same LLM provider.
        let backend: Box<dyn AgentBackend> = Box::new(
            DefaultAgentBackend::new(llm_provider, agent_graph, backend_config).with_interceptor(
                std::sync::Arc::new(ArtifactInterceptor::new(ArtifactStore::new())),
            ),
        );

        let transport_control_tx = transport.control_tx.clone();

        let mut reactor = Reactor::new(
            config,
            backend,
            audio_in_rx,
            tracer,
            stt_config,
            tts_config,
            transport_control_tx,
        );

        // NOTE: Recording is intentionally NOT started here.
        // Voice-engine only emits audio events on the bus; the embedder
        // (voice-server) is responsible for subscribing to the Tracer and
        // handling recording/storage (filesystem, S3, etc.).
        // Subscribe via tracer.subscribe() BEFORE calling VoiceSession::start.

        // Load ONNX models, connect STT WebSocket, emit SessionReady, play greeting.
        reactor.start().await.map_err(|e| e.to_string())?;

        // Spawn the Reactor
        let reactor_handle = tokio::spawn(async move {
            reactor.run().await;
        });

        info!("Voice session started (Reactor architecture)");

        Ok(Self {
            reactor_handle: Some(reactor_handle),
            _transport: transport,
        })
    }

    /// Close the session cleanly.
    ///
    /// Waits for the Reactor to finish its cleanup (with timeout safety net).
    pub async fn close(&mut self) {
        // Wait for the Reactor to finish its cleanup (with timeout safety net)
        if let Some(handle) = self.reactor_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        info!("Voice session closed");
    }

    /// Wait for the session to complete.
    ///
    /// This blocks until the transport disconnects and the Reactor finishes.
    /// After this returns, call `close()` for final cleanup.
    pub async fn wait_for_completion(&mut self) {
        if let Some(handle) = self.reactor_handle.take() {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    fn clean_for_tts(token: &str) -> String {
        token
            .chars()
            .filter(|c| !c.is_ascii_control() && *c != '*' && *c != '#' && *c != '_')
            .filter(|c| {
                !matches!(c, '\u{1F600}'..='\u{1F6FF}' | '\u{2600}'..='\u{26FF}' | '\u{2700}'..='\u{27BF}')
            })
            .collect()
    }

    #[test]
    fn test_clean_for_tts() {
        assert_eq!(clean_for_tts("Hello **world**!"), "Hello world!");
        assert_eq!(clean_for_tts("## Heading"), " Heading");
    }
}
