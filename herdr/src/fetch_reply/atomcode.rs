//! AtomCode session: `source_path` points at the `.index` metadata file, while
//! the messages live in the sibling `.events` log. `assistant_message` events
//! carry the text; a `user_message` marks the start of the last turn.

use std::path::Path;

use serde_json::Value;

use super::{jsonl_records, read_tail, FetchError};

pub(super) fn extract(path: &Path) -> Result<String, FetchError> {
    let events_path = path.with_extension("events");
    let records = jsonl_records(&read_tail(&events_path)?);
    let mut blocks: Vec<String> = Vec::new();
    for record in records.iter().rev() {
        let event = record.get("event");
        match event
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str)
        {
            Some("user_message") => break,
            Some("assistant_message") => {
                if let Some(text) = event
                    .and_then(|value| value.get("text"))
                    .and_then(Value::as_str)
                {
                    if !text.is_empty() {
                        blocks.push(text.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    blocks.reverse();
    if blocks.is_empty() {
        return Err(FetchError(format!(
            "no assistant message in {}",
            events_path.display()
        )));
    }
    Ok(blocks.join("\n"))
}
