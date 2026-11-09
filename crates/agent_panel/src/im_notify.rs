//! The text an IM notification carries, and the agent it came from.
//!
//! The panel already knows when an agent needs attention or finishes; this
//! module turns that moment into one message for the IM channel. The channel
//! only ever sees the text.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::client;
use crate::snapshot::{Agent, AgentStatus, Space};

/// Longest reply body put into a message, in characters.
///
/// A chat platform rejects an oversized text, and the end of an answer is what
/// the reader needs, so the tail is kept.
const MAX_REPLY_CHARS: usize = 3000;

/// How long to wait, and how many times to look, for the agent to write the
/// reply of the turn that just finished.
///
/// The status changes before the transcript record is flushed and, for a wait,
/// before the question is drawn. The first read can therefore be empty or hold
/// the previous turn, and only a later one is current.
const REPLY_READ_WAIT: Duration = Duration::from_millis(600);
const REPLY_READ_ATTEMPTS: usize = 8;

/// The last wait announced per pane, so a repeat report of the same wait does
/// not produce a second message.
static ANNOUNCED_BLOCKED_SEQ: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// The agent that produced the most recent IM notification.
///
/// A chat message that names no agent goes to this one. The value is copied
/// from the snapshot; the agent's own pane id is never changed.
static IM_SOURCE_AGENT_PANE_ID: Mutex<Option<String>> = Mutex::new(None);

/// A state transition worth telling the user about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentNotification {
    /// The agent stopped and waits for the user, e.g. a tool approval.
    NeedsAttention,
    /// The agent finished a turn on its own.
    Finished,
}

impl AgentNotification {
    /// Decides whether a transition is an event, given the turn marker.
    ///
    /// `completion_seq` is set by the server on real completions and left
    /// unset when the user interrupted the turn. Both cases arrive as
    /// `Working -> Idle`, so the marker is the only way to tell them apart.
    pub(crate) fn from_transition(
        previous: AgentStatus,
        current: AgentStatus,
        completion_seq: Option<u64>,
    ) -> Option<Self> {
        let kind = match (previous, current) {
            (previous, AgentStatus::Blocked) if previous != AgentStatus::Blocked => {
                Self::NeedsAttention
            }
            (AgentStatus::Working, AgentStatus::Done | AgentStatus::Idle) => Self::Finished,
            (AgentStatus::Blocked, AgentStatus::Done) => Self::Finished,
            _ => return None,
        };
        if kind == Self::Finished && completion_seq.is_none() {
            return None;
        }
        Some(kind)
    }

    /// Wording shared with the toast, so both surfaces read the same.
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::NeedsAttention => localization::localized_str!("needs attention"),
            Self::Finished => localization::localized_str!("finished"),
        }
    }
}

/// Sends one notification about `agent` and remembers it as the default
/// recipient for the next chat message.
pub(crate) fn forward(spaces: &[Space], space: &Space, agent: &Agent, kind: AgentNotification) {
    remember_source_agent(&agent.pane_id);
    let header = format!("[{}] {} \u{00b7} {}", space.label, agent.name, kind.text());
    let pane_id = agent.pane_id.clone();
    let directory = spaces.to_vec();
    let spawn = std::thread::Builder::new()
        .name("im-notify".to_string())
        .spawn(move || {
            // Asked for here rather than carried on the polled snapshot: the
            // daemon parses the transcript per call, so this is the text the
            // agent has written now, including the question it is waiting on.
            let wait_for_new_wait = kind == AgentNotification::NeedsAttention;
            let (reply, blocked_seq) = read_last_reply(&pane_id, wait_for_new_wait);
            if wait_for_new_wait && blocked_seq != 0 && !announce_blocked(&pane_id, blocked_seq) {
                log::debug!("agent panel: wait {blocked_seq} for {pane_id} was already announced");
                return;
            }
            let mut text = header;
            if let Some(reply) = reply {
                let reply = reply.trim();
                if !reply.is_empty() {
                    text.push('\n');
                    text.push_str(&clip_reply(reply));
                }
            }
            append_directory(&mut text, &directory);
            im_channel::transport::send_text(text);
        });
    if let Err(error) = spawn {
        log::warn!("agent panel: cannot start the IM notification task: {error}");
    }
}

/// The pane id of the agent that produced the most recent notification.
pub(crate) fn source_agent_pane() -> Option<String> {
    IM_SOURCE_AGENT_PANE_ID
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
}

fn remember_source_agent(pane_id: &str) {
    match IM_SOURCE_AGENT_PANE_ID.lock() {
        Ok(mut slot) => *slot = Some(pane_id.to_string()),
        Err(_) => log::error!("agent panel: the IM source agent lock is poisoned"),
    }
}

/// The only agent in the snapshot, when there is exactly one.
pub(crate) fn only_agent_pane(spaces: &[Space]) -> Option<String> {
    let mut agents = spaces.iter().flat_map(Space::agents);
    let first = agents.next()?;
    if agents.next().is_some() {
        return None;
    }
    Some(first.pane_id.clone())
}

/// Appends the `@` hint and one identifier per open agent, after a blank line.
pub(crate) fn append_directory(text: &mut String, spaces: &[Space]) {
    if agent_entries(spaces).is_empty() {
        return;
    }
    text.push_str("\n\n");
    text.push_str(localization::localized_str!(
        "To send a message to a specific agent, start with @ and its identifier. Agents now open:"
    ));
    append_entries(text, spaces);
}

/// Appends one identifier per open agent, each on its own line.
pub(crate) fn append_entries(text: &mut String, spaces: &[Space]) {
    for (label, name) in agent_entries(spaces) {
        text.push('\n');
        text.push('[');
        text.push_str(label);
        text.push_str("] ");
        text.push_str(name);
    }
}

fn agent_entries(spaces: &[Space]) -> Vec<(&str, &str)> {
    spaces
        .iter()
        .flat_map(|space| {
            space
                .agents()
                .map(|agent| (space.label.as_str(), agent.name.as_str()))
        })
        .collect()
}

fn clip_reply(reply: &str) -> String {
    let total = reply.chars().count();
    if total <= MAX_REPLY_CHARS {
        return reply.to_string();
    }
    let tail: String = reply.chars().skip(total - MAX_REPLY_CHARS).collect();
    format!("\u{2026}{tail}")
}

/// Reads the agent's last reply, retrying while it is still being written.
///
/// The wait lives here, on the send path, and nowhere near the status poll.
///
/// A finished turn is done once the text is there. A wait is different: the
/// first non-empty read is usually the previous turn, so it keeps looking until
/// the daemon reports a different wait, and returns the last text it saw.
fn read_last_reply(pane_id: &str, wait_for_new_wait: bool) -> (Option<String>, u64) {
    let mut baseline_seq: Option<u64> = None;
    let mut latest_text: Option<String> = None;
    let mut latest_seq = 0;
    for attempt in 0..REPLY_READ_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(REPLY_READ_WAIT);
        }
        match client::fetch_reply(pane_id) {
            Ok(reply) => {
                latest_seq = reply.blocked_seq;
                let Some(text) = reply.text.filter(|text| !text.trim().is_empty()) else {
                    continue;
                };
                if !wait_for_new_wait {
                    return (Some(text), latest_seq);
                }
                match baseline_seq {
                    None => {
                        baseline_seq = Some(latest_seq);
                        latest_text = Some(text);
                    }
                    Some(seq) if latest_seq != seq => return (Some(text), latest_seq),
                    Some(_) => latest_text = Some(text),
                }
            }
            Err(error) => {
                log::warn!("agent panel: cannot read the last reply: {error}");
                break;
            }
        }
    }
    if latest_text.is_none() {
        log::warn!("agent panel: the agent has written no reply for {pane_id} yet");
    }
    (latest_text, latest_seq)
}

/// Records a wait as announced. Returns false when it already was.
fn announce_blocked(pane_id: &str, blocked_seq: u64) -> bool {
    match ANNOUNCED_BLOCKED_SEQ.lock() {
        Ok(mut slot) => {
            let announced = slot.get_or_insert_with(HashMap::new);
            if announced.get(pane_id) == Some(&blocked_seq) {
                return false;
            }
            announced.insert(pane_id.to_string(), blocked_seq);
            true
        }
        Err(_) => {
            log::error!("agent panel: the announced wait lock is poisoned");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPLETED: Option<u64> = Some(7);

    #[test]
    fn entering_blocked_needs_attention() {
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Working, AgentStatus::Blocked, None),
            Some(AgentNotification::NeedsAttention)
        );
    }

    #[test]
    fn repeated_blocked_is_not_an_event() {
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Blocked, AgentStatus::Blocked, None),
            None
        );
    }

    #[test]
    fn completed_turn_finishes() {
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Working, AgentStatus::Idle, COMPLETED),
            Some(AgentNotification::Finished)
        );
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Working, AgentStatus::Done, COMPLETED),
            Some(AgentNotification::Finished)
        );
    }

    #[test]
    fn interrupted_turn_is_not_a_finish() {
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Working, AgentStatus::Idle, None),
            None
        );
    }

    #[test]
    fn blocked_turn_that_completes_finishes() {
        assert_eq!(
            AgentNotification::from_transition(AgentStatus::Blocked, AgentStatus::Done, COMPLETED),
            Some(AgentNotification::Finished)
        );
    }

    #[test]
    fn a_reply_longer_than_the_limit_keeps_its_tail() {
        let reply = "a".repeat(MAX_REPLY_CHARS + 10);
        let clipped = clip_reply(&reply);
        assert_eq!(clipped.chars().count(), MAX_REPLY_CHARS + 1);
        assert!(clipped.starts_with('\u{2026}'));
    }
}
