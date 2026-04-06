/// Interface for reactor lifecycle events.
///
/// All methods have default no-op implementations — only override the events
/// you care about.
pub trait ReactorHook: Send + 'static {
    /// Called at the start of each LLM turn (before the first token).
    fn on_turn_start(&mut self) {}

    /// Called when a finalized STT transcript is committed for this turn.
    /// `is_final` is `true` for committed transcripts, `false` for partials.
    fn on_stt_transcript(&mut self, _text: &str, _is_final: bool) {}

    /// Called for each token emitted by the LLM.
    fn on_llm_token(&mut self, _token: &str) {}

    /// Called when a synchronous (non-async) tool call starts.
    fn on_tool_call(&mut self, _name: &str, _call_id: &str) {}

    /// Called after each completed LLM turn (text has been sent to TTS).
    fn on_turn_end(&mut self, _spoken_text: &str) {}

    /// Called when a barge-in interruption is detected.
    fn on_barge_in(&mut self) {}

    /// Called when the session is ending.
    fn on_session_end(&mut self) {}
}
