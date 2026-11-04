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

/// One entry in a space's flat list: a running agent, or a tab that runs none.
/// The list is in tab order, so the panel does not have to merge two lists.
#[derive(Clone)]
pub(crate) enum SpaceEntry {
    Agent(Agent),
    Terminal(TerminalEntry),
}

/// One space: what the server calls a workspace.
#[derive(Clone)]
pub(crate) struct Space {
    pub id: String,
    pub label: String,
    /// Whether this is the space the server has in front.
    ///
    /// Paired with `active_tab_id`, this is the tab-level view of what the
    /// user is looking at, which the toast needs: `Agent::focused` is
    /// pane-level and goes false as soon as pane focus moves elsewhere in the
    /// same tab.
    #[cfg(target_env = "ohos")]
    pub focused: bool,
    /// The tab this space has in front, as a public tab id that lines up with
    /// `Agent::tab_id`.
    #[cfg(target_env = "ohos")]
    pub active_tab_id: String,
    /// Agents and empty tabs in one flat list, in tab order.
    pub entries: Vec<SpaceEntry>,
}

impl Space {
    pub(crate) fn agents(&self) -> impl Iterator<Item = &Agent> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            SpaceEntry::Agent(agent) => Some(agent),
            SpaceEntry::Terminal(_) => None,
        })
    }

    pub(crate) fn agents_mut(&mut self) -> impl Iterator<Item = &mut Agent> + '_ {
        self.entries.iter_mut().filter_map(|entry| match entry {
            SpaceEntry::Agent(agent) => Some(agent),
            SpaceEntry::Terminal(_) => None,
        })
    }

    pub(crate) fn terminals(&self) -> impl Iterator<Item = &TerminalEntry> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            SpaceEntry::Terminal(terminal) => Some(terminal),
            SpaceEntry::Agent(_) => None,
        })
    }
}

/// One tab of a space with no agent running in any of its panes.
#[derive(Clone)]
pub(crate) struct TerminalEntry {
    pub tab_id: String,
    pub label: String,
    pub focused: bool,
}

/// One agent, named the way the server names it in its own sidebar.
#[derive(Clone)]
pub(crate) struct Agent {
    /// The server's terminal id. Stable for the life of the terminal, and unique
    /// across the server, so it identifies the row.
    pub id: String,
    /// The pane this agent is drawn in, as the server's public pane id
    /// (`w_3:p2`). This is the only identifier `agent.focus` resolves: it
    /// accepts a public pane id or an agent name, never a terminal id.
    pub pane_id: String,
    pub name: String,
    /// The agent type the detector reported (`claude`, `agy`, `codebuddy`).
    /// Empty when the server could not identify the program in the pane.
    pub kind: String,
    pub status: AgentStatus,
    /// Pane-level focus: true only while the agent's own pane holds pane focus
    /// in the tab in front. `Space::active_tab_id` is the tab-level test.
    pub focused: bool,
    /// The public id of the tab this agent is drawn in, matching the space's
    /// `active_tab_id`.
    pub tab_id: String,
    /// The server's completion marker for the current idle transition.
    ///
    /// It is set when the transition completed work and left unset when the
    /// server suppressed the completion (the user stopped the turn with Esc).
    /// The panel uses it to tell those two cases apart; the state alone looks
    /// identical in both.
    #[cfg(target_env = "ohos")]
    pub completion_seq: Option<u64>,
    /// `(agent, session value)` as the agent itself reported, when it runs
    /// inside a recognized session. Values only line up with history entries
    /// of the same agent.
    pub agent_session: Option<(String, String)>,
    /// The server is still waiting for the agent it started here to become
    /// ready. The wait has the server's own deadline; the panel must not apply
    /// one of its own, or a slow start reads as a failure.
    pub launch_pending: bool,
    /// The server has seen this agent reach its interactive prompt, which is
    /// the only success signal it reports.
    pub interactive_ready: bool,
}

/// Everything the panel draws in one frame.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub spaces: Vec<Space>,
}

impl Snapshot {
    /// Joins the three API payloads. Agents keep the server's order inside
    /// their space; a tab with no agent at all becomes a flat `Terminal-<label>`
    /// entry in the same list.
    pub(crate) fn from_api(
        workspaces: &Value,
        agents: &Value,
        tabs: &Value,
    ) -> Result<Self, ClientError> {
        let workspace_entries = workspaces
            .get("workspaces")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::Failed("workspace.list carried no workspaces".into()))?;
        let agent_entries = agents
            .get("agents")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::Failed("agent.list carried no agents".into()))?;
        let tab_entries = tabs
            .get("tabs")
            .and_then(Value::as_array)
            .ok_or_else(|| ClientError::Failed("tab.list carried no tabs".into()))?;

        let mut spaces: Vec<Space> = workspace_entries
            .iter()
            .map(|entry| Space {
                id: text(entry, "workspace_id"),
                label: text(entry, "label"),
                #[cfg(target_env = "ohos")]
                focused: flag(entry, "focused"),
                #[cfg(target_env = "ohos")]
                active_tab_id: text(entry, "active_tab_id"),
                entries: Vec::new(),
            })
            .collect();

        // Group agents by the tab they run in, keeping the server's order.
        let mut agents_by_tab: std::collections::HashMap<(String, String), Vec<Agent>> =
            std::collections::HashMap::new();
        for entry in agent_entries {
            let workspace_id = text(entry, "workspace_id");
            let tab_id = text(entry, "tab_id");
            agents_by_tab
                .entry((workspace_id, tab_id.clone()))
                .or_default()
                .push(Agent {
                    id: text(entry, "terminal_id"),
                    pane_id: text(entry, "pane_id"),
                    name: agent_name(entry),
                    kind: text(entry, "agent"),
                    status: AgentStatus::parse(entry.get("agent_status")),
                    focused: flag(entry, "focused"),
                    tab_id,
                    #[cfg(target_env = "ohos")]
                    completion_seq: entry.get("completion_seq").and_then(Value::as_u64),
                    // The session the agent itself reports (source + value). The
                    // value is the id the resume path knows; the history view
                    // uses it to hide sessions that are already open.
                    agent_session: entry.get("agent_session").and_then(|session| {
                        Some((
                            session.get("agent")?.as_str()?.to_owned(),
                            session.get("value")?.as_str()?.to_owned(),
                        ))
                    }),
                    launch_pending: flag(entry, "launch_pending"),
                    interactive_ready: flag(entry, "interactive_ready"),
                });
        }

        // Walk the tabs once, in the server's order, so agents and empty tabs
        // interleave the way the tab strip shows them.
        for entry in tab_entries {
            let tab_id = text(entry, "tab_id");
            let workspace_id = text(entry, "workspace_id");
            let Some(space) = spaces.iter_mut().find(|space| space.id == workspace_id) else {
                continue;
            };
            if let Some(agents) = agents_by_tab.remove(&(workspace_id, tab_id.clone())) {
                space
                    .entries
                    .extend(agents.into_iter().map(SpaceEntry::Agent));
            } else {
                space.entries.push(SpaceEntry::Terminal(TerminalEntry {
                    tab_id,
                    label: text(entry, "label"),
                    focused: flag(entry, "focused"),
                }));
            }
        }

        // An agent whose tab is not in `tab.list` yet still belongs somewhere;
        // append it to its space (or the last one) rather than dropping it.
        for ((workspace_id, _), agents) in agents_by_tab {
            let index = spaces
                .iter()
                .position(|space| space.id == workspace_id)
                .or_else(|| spaces.len().checked_sub(1));
            let Some(index) = index else {
                continue;
            };
            spaces[index]
                .entries
                .extend(agents.into_iter().map(SpaceEntry::Agent));
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
