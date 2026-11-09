//! The panel's side of the IM channel.
//!
//! The channel hands over plain text only: a message typed in the chat, and a
//! text to send back. Choosing the agent, and knowing what an agent is, lives
//! here.

use std::sync::Arc;

use crate::client;
use crate::im_notify;
use crate::snapshot::{Agent, Snapshot, Space};

/// Installs the inbound handler and resumes the stored IM channel.
pub fn install() {
    im_channel::transport::set_inbound_handler(Arc::new(handle_inbound));
    im_channel::manager::resume();
}

/// Delivers one chat message to the agent it addresses.
///
/// The rules, in order: a leading `@[space] name` picks that agent; otherwise the
/// agent that produced the most recent notification; otherwise the only agent
/// when exactly one is open. Anything else is answered with the list.
fn handle_inbound(text: String) {
    let snapshot = match client::fetch() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!("agent panel: cannot read the agent list: {error}");
            return;
        }
    };
    let (target, body) = match parse_mention(&text) {
        Some((space_label, agent_name, body)) => {
            match find_agent(&snapshot.spaces, &space_label, &agent_name) {
                Some(agent) => (agent.pane_id.clone(), body),
                None => {
                    log::warn!("agent panel: no agent matches [{space_label}] {agent_name}");
                    ask_for_target(&snapshot);
                    return;
                }
            }
        }
        None => {
            let target = im_notify::source_agent_pane()
                .filter(|pane_id| has_pane(&snapshot.spaces, pane_id))
                .or_else(|| im_notify::only_agent_pane(&snapshot.spaces));
            match target {
                Some(pane_id) => (pane_id, text.trim().to_string()),
                None => {
                    ask_for_target(&snapshot);
                    return;
                }
            }
        }
    };
    if body.is_empty() {
        return;
    }
    if let Err(error) = client::prompt_agent(&target, &body) {
        log::warn!("agent panel: cannot deliver an IM message: {error}");
    }
}

/// Splits `@[space] name text` into its parts.
///
/// The `@` is optional: a chat client can treat it as a mention and drop it,
/// and a pasted identifier has none to begin with.
fn parse_mention(text: &str) -> Option<(String, String, String)> {
    let rest = text.trim_start();
    let rest = rest.strip_prefix('@').unwrap_or(rest);
    let rest = rest.strip_prefix('[')?;
    let (space_label, rest) = rest.split_once(']')?;
    let (agent_name, body) = rest.trim_start().split_once(char::is_whitespace)?;
    let body = body.trim();
    if space_label.is_empty() || agent_name.is_empty() || body.is_empty() {
        return None;
    }
    Some((
        space_label.to_string(),
        agent_name.to_string(),
        body.to_string(),
    ))
}

fn find_agent<'a>(spaces: &'a [Space], space_label: &str, agent_name: &str) -> Option<&'a Agent> {
    spaces
        .iter()
        .find(|space| space.label == space_label)
        .and_then(|space| space.agents().find(|agent| agent.name == agent_name))
}

fn has_pane(spaces: &[Space], pane_id: &str) -> bool {
    spaces
        .iter()
        .flat_map(Space::agents)
        .any(|agent| agent.pane_id == pane_id)
}

/// Tells the user to name an agent, and lists the ones that are open.
fn ask_for_target(snapshot: &Snapshot) {
    let mut text = localization::localized_str!(
        "Please specify the agent that should receive the message. Agents now open:"
    )
    .to_string();
    im_notify::append_entries(&mut text, &snapshot.spaces);
    im_channel::transport::send_text(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_is_split_into_its_parts() {
        let (space, name, body) =
            parse_mention("@[hicodeer] w5-codebuddy-4 summarize this").expect("a mention");
        assert_eq!(space, "hicodeer");
        assert_eq!(name, "w5-codebuddy-4");
        assert_eq!(body, "summarize this");
    }

    #[test]
    fn text_without_a_mention_is_plain() {
        assert!(parse_mention("what changed?").is_none());
    }

    #[test]
    fn mention_is_found_without_the_at_sign() {
        let (space, name, body) =
            parse_mention("[hicodeer] w5-claude-6 test this").expect("a mention");
        assert_eq!(space, "hicodeer");
        assert_eq!(name, "w5-claude-6");
        assert_eq!(body, "test this");
    }

    #[test]
    fn mention_without_a_body_is_plain() {
        assert!(parse_mention("@[hicodeer] w5-codebuddy-4").is_none());
    }
}
