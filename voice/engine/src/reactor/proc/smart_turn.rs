//! SmartTurn stage — wraps SmartTurnAnalyzer for inline (non-actor) use.
//!
//! Called synchronously on SpeechEnded to decide whether the user has
//! finished their turn. Returns `true` if the turn is complete (start pipeline),
//! `false` if incomplete (wait for more speech).

use crate::audio_ml::smart_turn::SmartTurnAnalyzer;
use tracing::info;

pub struct SmartTurnStage {
    inner: SmartTurnAnalyzer,
}

impl SmartTurnStage {
    pub fn new(model_path: &str, threshold: f32) -> Self {
        Self {
            inner: SmartTurnAnalyzer::new(model_path, threshold),
        }
    }

    /// Initialize ONNX model.
    pub fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.initialize()
    }

    /// Predict whether the user's turn is complete.
    ///
    /// Returns `true` if the user finished speaking (start pipeline),
    /// `false` if they are likely still formulating their thought.
    pub fn predict(&mut self, speech_audio: &[u8]) -> bool {
        match self.inner.predict(speech_audio) {
            Ok((complete, prob)) => {
                info!(
                    "[smart_turn] {} prob={:.3}",
                    if complete { "COMPLETE" } else { "INCOMPLETE" },
                    prob
                );
                complete
            }
            Err(e) => {
                // On inference error, fall back to treating turn as complete
                // so the pipeline isn't permanently stalled.
                tracing::warn!(
                    "[smart_turn] predict error: {} — falling back to complete",
                    e
                );
                true
            }
        }
    }
}
