//! The tree the Agent panel draws: the spaces the agent server reports, each
//! with the agents running inside it, joined from `workspace.list` and
//! `agent.list`.

use serde_json::Value;

use crate::client::ClientError;

/// Agent states, spelled exactly as the server's API spells them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentStatus {
    Working,
    Blocked,
    Done,
    Idle,
    Unknown,
}

impl AgentStatus {
    fn parse(value: Option<&Value>) -> Self {
        match value.and_then(Value::as_str) {
            Some("working") => Self::Working,
            Some("blocked") => Self::Blocked,
            Some("done") => Self::Done,
            Some("idle") => Self::Idle,
            _ => Self::Unknown,
        }
    }
}

/// One space: what the server calls a workspace.
pub(crate) struct Space {
    pub id: String,
    pub label: String,
    pub agents: Vec<Agent>,
}

/// One agent, named the way the server names it in its own sidebar.
pub(crate) struct Agent {
    /// The server's terminal id. Stable for the life of the terminal, and unique
    /// across the server, so it identifies the row.
    pub id: String,
    /// The pane this agent is drawn in, as the server's public pane id
    /// (`w_3:p2`). This is the only identifier `agent.focus` resolves: it
    /// accepts a public pane id or an agent name, never a terminal id.
    pub pane_id: String,
    pub name: String,
    pub status: AgentStatus,
    pub focused: bool,
}

/// Everything the panel draws in one frame.
pub(crate) struct Snapshot {
    pub spaces: Vec<Space>,
}

impl Snapshot {
    /// Joins the two API payloads. Agents keep the server's order inside their
    /// space, so the panel nests them the way the TUI lists them.
    pub(crate) fn from_api(workspaces: &Value, agents: &Value) -> Result<Self, ClientError> {
        let workspace_entries = workspaces
            .get("workspaces")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::Failed("workspace.list carried no workspaces".into()))?;
        let agent_entries = agents
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::Failed("agent.list carried no agents".into()))?;

        let mut spaces: Vec<Space> = workspace_entries
            .iter()
            .map(|entry| Space {
                id: text(entry, "workspace_id"),
                label: text(entry, "label"),
                agents: Vec::new(),
            })
            .collect();

        for entry in agent_entries {
            let workspace_id = text(entry, "workspace_id");
            // An agent whose workspace is missing still belongs somewhere; it
            // goes last rather than being dropped from the list entirely.
            let index = spaces
                .iter()
                .position(|space| space.id == workspace_id)
                .or_else(|| spaces.len().checked_sub(1));
            let Some(index) = index else {
                continue;
            };
            spaces[index].agents.push(Agent {
                id: text(entry, "terminal_id"),
                pane_id: text(entry, "pane_id"),
                name: agent_name(entry),
                status: AgentStatus::parse(entry.get("agent_status")),
                focused: flag(entry, "focused"),
            });
        }

        Ok(Self { spaces })
    }
}

/// Best available display name, in the order the server's sidebar prefers.
fn agent_name(entry: &Value) -> String {
    for key in ["display_agent", "name", "agent", "title", "terminal_title"] {
        let value = text(entry, key);
        if !value.is_empty() {
            return value;
        }
    }
    text(entry, "terminal_id")
}

fn flag(entry: &Value, key: &str) -> bool {
    entry.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn text(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
