//! Recording bridge for voice-server.
//!
//! Voice-trace handles all storage (file I/O via `file://` URIs, and S3 when
//! the `s3` feature is enabled). This module's only job is to wrap
//! `spawn_recording_subscriber` and pass the resulting `storage_uri` directly
//! into the database as `calls.recording_url`.
//!
//! ## What goes into the DB
//!
//! The `calls.recording_url` column stores the **canonical storage URI**
//! produced by voice-trace — never a derived HTTP URL:
//!
//! | Written by voice-trace      | Stored verbatim in DB         |
//! |-----------------------------|-------------------------------|
//! | `file:///abs/path/x.opus`   | `file:///abs/path/x.opus`     |
//! | `s3://bucket/prefix/x.opus` | `s3://bucket/prefix/x.opus`   |
//! | empty (write failed)        | `NULL`                        |
//!

use tokio::sync::broadcast;
use voice_trace::event::Event;
use voice_trace::recording::RecordingConfig;
use voice_trace::{spawn_recording_subscriber, RecordingOutput};

/// Spawn a recording subscriber.
///
/// `on_complete` receives `(recording_uri, audio_duration_secs, transcript_json)`
/// where `recording_uri` is the canonical storage URI written by voice-trace
/// (`file:///abs/path/session.opus`, `s3://…`, or `None` on failure/empty session).
///
/// The caller stores this URI verbatim in the database and lets the API layer
/// translate it to an HTTP URL at query time.
pub fn spawn<F>(
    rx: broadcast::Receiver<Event>,
    session_id: String,
    config: RecordingConfig,
    on_complete: F,
) where
    F: FnOnce(Option<String>, u32, Option<Vec<u8>>) + Send + 'static,
{
    spawn_recording_subscriber(
        rx,
        session_id,
        config,
        Some(move |output: Option<RecordingOutput>| {
            let (recording_uri, duration_secs, transcript) = match output {
                Some(r) => {
                    let dur = r.duration_secs as u32;
                    let transcript = r.transcript_json.clone();
                    // Pass the canonical URI directly — no URL transformation here.
                    let uri = if r.storage_uri.is_empty() {
                        None
                    } else {
                        Some(r.storage_uri)
                    };
                    (uri, dur, transcript)
                }
                None => (None, 0, None),
            };
            on_complete(recording_uri, duration_secs, transcript);
        }),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_uri_passthrough() {
        // voice-server stores the URI verbatim — no /api/... transformation.
        let uri = "file:///recordings/abc-def.opus";
        // No mutation — the test just documents the expected contract.
        assert!(uri.starts_with("file://"));
    }
}
