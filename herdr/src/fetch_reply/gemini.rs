//! Gemini session: either one JSON document with a messages array or the
//! line-delimited recording current builds write (session-*.jsonl). Assistant
//! turns are typed `gemini`; the newest one carrying text is the reply.
//!
//! Gemini records text in three shapes across versions: a plain string, an
//! array of part objects, and a `{ parts: [...] }` wrapper. All three are
//! accepted so a schema bump never silently empties the reply.

use std::path::Path;

use serde_json::Value;

use super::{text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| FetchError(format!("read {}: {error}", path.display())))?;

    // Document form first: a single object whose messages array holds every
    // turn.
    if let Ok(document) = serde_json::from_str::<Value>(&raw) {
        if let Some(messages) = document.get("messages").and_then(Value::as_array) {
            for message in messages.iter().rev() {
                if let Some(text) = gemini_text(message) {
                    return Ok(text);
                }
            }
        }
    }

    // Recording form: one JSON object per line, newest last.
    for line in raw.lines().rev() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(text) = gemini_text(&record) {
            return Ok(text);
        }
    }

    Err(FetchError(format!("no gemini message in {}", path.display())))
}

/// The visible text of one gemini turn, or None for any other record.
fn gemini_text(record: &Value) -> Option<String> {
    if record.get("type").and_then(Value::as_str) != Some("gemini") {
        return None;
    }
    let content = record.get("content");
    let mut text = text_blocks(content);
    if text.is_empty() {
        if let Some(parts) = content.and_then(|value| value.get("parts")) {
            text = text_blocks(Some(parts));
        }
    }
    if text.is_empty() {
        if let Some(raw) = record.get("text").and_then(Value::as_str) {
            text = raw.trim().to_owned();
        }
    }
    (!text.is_empty()).then_some(text)
}
