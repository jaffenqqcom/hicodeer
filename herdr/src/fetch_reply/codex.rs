//! Codex CLI rollout: `task_complete.last_agent_message` holds the final text.
//! Interrupted turns leave it null, so fall back to the last assistant
//! `response_item` content.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, text_blocks, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let records = jsonl_records(&read_tail(path)?);

    for record in records.iter().rev() {
        let payload = record.get("payload");
        if payload
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            == Some("task_complete")
        {
            if let Some(text) = payload
                .and_then(|value| value.get("last_agent_message"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                return Ok(text.to_owned());
            }
        }
    }

    for record in records.iter().rev() {
        let payload = record.get("payload");
        if payload
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            == Some("assistant")
        {
            let text = text_blocks(payload.and_then(|value| value.get("content")));
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
