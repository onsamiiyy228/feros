//! Recording configuration types.
//!
//! Pure data types for session recording configuration. These live in
//! `common` because they are needed by both `agent-kit` (graph definition
//! deserialization) and `voice-trace` (recording subscriber logic).

use serde::{Deserialize, Serialize};

/// Audio channel layout for session recordings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioLayout {
    /// Stereo: left channel = user input, right channel = agent TTS.
    Stereo,
    /// Mono: user and agent audio mixed into a single channel.
    Mono,
}

impl Default for AudioLayout {
    fn default() -> Self { Self::Stereo }
}

/// Audio output format for session recordings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// OGG/Opus — modern, patent-free codec. ~10–20× smaller than WAV.
    Opus,
    /// WAV (uncompressed PCM16) — lossless, larger files.
    Wav,
}

impl Default for AudioFormat {
    fn default() -> Self { Self::Opus }
}

/// Configuration for session recording.
///
/// Recording is **opt-out**: sessions are recorded by default.
/// Set `enabled = false` in the agent graph to disable recording for a
/// specific agent.
///
/// ## Storage URI
///
/// `output_uri` controls where audio is written. Two schemes are supported:
///
/// | URI                  | Behavior         | Resulting `storage_uri`                      |
/// |----------------------|------------------|----------------------------------------------|
/// | `file:///abs/path`   | Local filesystem | `file:///abs/path/{id}.opus`                 |
/// | `file://./relative`  | Relative to CWD  | `file:///abs/path/{id}.opus` (canonicalized) |
/// | `s3://bucket/prefix` | S3 object store  | `s3://bucket/prefix/{id}.opus`               |
///
/// Voice-trace writes the session audio to the designated sink and exposes the 
/// **absolute canonical URI** of the asset in `RecordingOutput::storage_uri` 
/// for the caller to consume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Master switch — disable session recording.
    /// Defaults to `true` (recording is opt-out).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Destination URI for the recording output.
    ///
    /// Supported schemes:
    /// - `file:///absolute/path`  — write to absolute filesystem path
    /// - `file://./relative/path` — write relative to the working directory
    /// - `s3://bucket/prefix`     — upload to S3 (requires `s3` Cargo feature)
    ///
    /// Defaults to `file://./recordings`.
    #[serde(default = "default_output_uri")]
    pub output_uri: String,

    /// Audio channel layout.
    #[serde(default)]
    pub audio_layout: AudioLayout,

    /// Output sample rate for the audio file (both channels resampled).
    /// Default: `16000` (matches internal pipeline rate).
    #[serde(default = "default_recording_sample_rate")]
    pub sample_rate: u32,

    /// Audio output format.
    #[serde(default)]
    pub audio_format: AudioFormat,

    /// Maximum recording duration in seconds. Once exceeded, audio capture
    /// stops but transcript logging continues. `0` = unlimited (default).
    ///
    /// Use this to bound memory usage on long sessions:
    /// - 30 min stereo at 16 kHz ≈ 115 MB
    /// - 60 min stereo at 16 kHz ≈ 230 MB
    #[serde(default)]
    pub max_duration_secs: u32,

    /// Save the transcript JSON alongside the audio.
    #[serde(default = "default_true")]
    pub save_transcript: bool,

    /// Include detailed tool call information in the transcript.
    #[serde(default = "default_true")]
    pub include_tool_details: bool,

    /// Include LLM metadata (model, tokens, latency) in the transcript.
    #[serde(default)]
    pub include_llm_metadata: bool,
}

pub fn default_output_uri() -> String { "file://./recordings".to_string() }
fn default_recording_sample_rate() -> u32 { 16000 }
fn default_true() -> bool { true }

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_uri: default_output_uri(),
            audio_layout: AudioLayout::default(),
            sample_rate: default_recording_sample_rate(),
            audio_format: AudioFormat::default(),
            max_duration_secs: 0,
            save_transcript: true,
            include_tool_details: true,
            include_llm_metadata: false,
        }
    }
}
