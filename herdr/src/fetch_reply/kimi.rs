//! Kimi wire log: `agent.message.appended` wraps a nested `message.message`
//! object whose `content` array holds the assistant text.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let records = jsonl_records(&read_tail(path)?);
    for record in records.iter().rev() {
        if record.get("type").and_then(Value::as_str) != Some("agent.message.appended") {
            continue;
        }
        let message = record
            .get("message")
            .and_then(|wrapper| wrapper.get("message"));
        let is_assistant = message
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            == Some("assistant");
        if is_assistant {
            let text = text_blocks(message.and_then(|value| value.get("content")));
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }
    Err(FetchError(format!(
        "no assistant message in {}",
        path.display()
    )))
}
