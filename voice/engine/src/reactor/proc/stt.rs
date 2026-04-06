//! STT stage — wraps a `SttProvider` as a pollable async I/O handle.
//!
//! The Reactor feeds raw 16 kHz PCM frames via `feed()` and polls for
//! finalized transcripts in its central `select!` via `recv()`.

use crate::providers::stt::SttProvider;
use crate::types::SttEvent;
use tokio::sync::mpsc;
use tracing::info;

pub struct SttStage {
    provider: Box<dyn SttProvider>,
    /// Receives STT events (FirstTextReceived, PartialTranscript, Transcript)
    /// from the provider's reader task.
    result_rx: Option<mpsc::Receiver<SttEvent>>,
}

impl SttStage {
    pub fn new(provider: Box<dyn SttProvider>) -> Self {
        Self {
            provider,
            result_rx: None,
        }
    }

    /// Connect to the STT backend.
    /// Must be called before `feed()` or `recv()`.
    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.provider.connect().await?;
        self.result_rx = self.provider.take_result_rx();
        info!("[stt_stage] Connected ({})", self.provider.provider_name());
        Ok(())
    }

    /// Feed a 16 kHz PCM-16 LE audio frame to the STT backend.
    /// Called inline by the Reactor on every audio frame.
    pub fn feed(&self, audio: &[u8]) {
        self.provider.feed_audio(audio);
    }

    /// Ask the STT backend to finalise the current utterance.
    /// Called by the Reactor when VAD `SpeechEnded` fires.
    pub fn finalize(&self) {
        self.provider.finalize();
    }

    /// Poll for the next [`SttEvent`] from the STT backend.
    /// Intended for use in the Reactor's `tokio::select!`.
    ///
    /// Returns `None` when the connection is closed.
    pub async fn recv(&mut self) -> Option<SttEvent> {
        let rx = self.result_rx.as_mut()?;
        rx.recv().await
    }

    /// Close the connection cleanly.
    pub fn close(&self) {
        self.provider.close();
    }
}
