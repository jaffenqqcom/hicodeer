//! The part of Codex's store that FAD's connector does not apply: the id its
//! `resume` subcommand takes.
//!
//! Codex writes one rollout per session and names it
//! `rollout-<timestamp>-<uuid>.jsonl`, so the row's id would otherwise be the
//! file name. `codex resume` wants the bare uuid, and the file name is the only
//! place the scanner sees it: FAD's connector puts nothing but `source` in the
//! row's metadata.

use std::path::Path;

use franken_agent_detection::NormalizedConversation;

/// Replaces the row id with the uuid Codex resumes by.
pub(super) fn adjust(conversations: &mut Vec<NormalizedConversation>) {
    for conversation in conversations.iter_mut() {
        let Some(uuid) = session_uuid(&conversation.source_path) else {
            continue;
        };
        if let Some(metadata) = conversation.metadata.as_object_mut() {
            metadata.insert("session_id".to_owned(), uuid.into());
        }
    }
}

/// The uuid at the end of a rollout file name.
///
/// A name this does not recognise yields `None`, which leaves the row's id as
/// the connector reported it — wrong for resuming, but never worse than
/// inventing an id.
fn session_uuid(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("rollout-") {
        return None;
    }
    // Right to left: twelve, four, four, four, eight, and everything before
    // (which holds the timestamp's own dashes).
    let mut parts: Vec<&str> = stem.rsplitn(6, '-').collect();
    if parts.len() < 6 {
        return None;
    }
    parts.reverse();
    let candidate = parts[1..].join("-");
    is_uuid(&candidate).then_some(candidate)
}

/// Eight, four, four, four, twelve hexadecimal digits.
fn is_uuid(value: &str) -> bool {
    let mut groups = value.split('-');
    let lengths = [8, 4, 4, 4, 12];
    for expected in lengths {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != expected || !group.chars().all(|character| character.is_ascii_hexdigit())
        {
            return false;
        }
    }
    groups.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rollout name carries its timestamp and the uuid, and only the uuid is
    /// what `codex resume` accepts.
    #[test]
    fn session_uuid_reads_the_uuid_out_of_a_rollout_name() {
        let path = Path::new(
            "/sessions/2026/10/27/rollout-2026-10-27T17-37-01-01a17cb8-65d1-7070-a8b8-ec0504f7704e.jsonl",
        );
        assert_eq!(
            session_uuid(path).as_deref(),
            Some("01a17cb8-65d1-7070-a8b8-ec0504f7704e")
        );
    }

    /// Anything that is not a rollout name, or does not end in a uuid, yields
    /// nothing so the connector's own id stays in place.
    #[test]
    fn session_uuid_ignores_names_it_cannot_trust() {
        assert_eq!(session_uuid(Path::new("/sessions/other.jsonl")), None);
        assert_eq!(session_uuid(Path::new("/sessions/rollout-2026-10-27.jsonl")), None);
        assert_eq!(
            session_uuid(Path::new("/sessions/rollout-2026-10-27T17-37-01-not-a-uuid.jsonl")),
            None
        );
    }

    /// The adjust step rewrites the row id, which is what the resume command
    /// reads.
    #[test]
    fn adjust_puts_the_uuid_into_the_row_metadata() {
        let mut conversations = vec![NormalizedConversation {
            agent_slug: "codex".to_owned(),
            external_id: Some(
                "rollout-2026-10-27T17-37-01-01a17cb8-65d1-7070-a8b8-ec0504f7704e".to_owned(),
            ),
            title: None,
            workspace: None,
            source_path: Path::new(
                "/sessions/2026/10/27/rollout-2026-10-27T17-37-01-01a17cb8-65d1-7070-a8b8-ec0504f7704e.jsonl",
            )
            .to_path_buf(),
            started_at: None,
            ended_at: None,
            metadata: serde_json::json!({"source": "rollout"}),
            messages: Vec::new(),
        }];

        adjust(&mut conversations);

        assert_eq!(
            conversations[0].metadata["session_id"],
            serde_json::json!("01a17cb8-65d1-7070-a8b8-ec0504f7704e"),
            "the uuid has to reach metadata, which is where the row id is read from"
        );
        assert_eq!(
            conversations[0].metadata["source"],
            serde_json::json!("rollout"),
            "the connector's own metadata survives"
        );
    }
}
