use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use bytes::Bytes;

use super::{terminal_targets::TerminalTargetError, App};
use crate::api::schema::AgentStartParams;

const DEFAULT_AGENT_START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_AGENT_START_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const AGENT_START_SETTLE_DELAY: Duration = Duration::from_secs(3);
const INVALID_AGENT_TIMEOUT_MESSAGE: &str =
    "agent start timeout must be greater than 3000ms and at most 300000ms";
const INVALID_AGENT_NAME_MESSAGE: &str = "agent name must start with a letter and contain only letters, digits, spaces, '-' or '_' (1-32 characters)";
/// Longest agent name, counted in characters rather than bytes so a name written
/// in a multi-byte script gets the same budget as an ASCII one.
const MAX_AGENT_NAME_CHARS: usize = 32;

/// The one place an agent name is built. `taken` decides what uniqueness means
/// for the caller, so the app and its state machine share the naming rule
/// instead of each holding a copy.
pub(super) fn auto_agent_name_with(
    workspaces: &[crate::workspace::Workspace],
    ws_idx: usize,
    tab_idx: usize,
    kind: crate::detect::Agent,
    taken: impl Fn(&str) -> bool,
) -> String {
    let Some(workspace) = workspaces.get(ws_idx) else {
        return crate::detect::agent_label(kind).to_owned();
    };
    let space = workspace.id.to_lowercase();
    let tab = workspace
        .tab_display_name(tab_idx)
        .unwrap_or_else(|| (tab_idx + 1).to_string());
    let base = format!("{space}-{}-{tab}", crate::detect::agent_label(kind));
    let base: String = base
        .chars()
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    let mut name = base.clone();
    let mut suffix = 2;
    while taken(&name) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    name
}

/// Whether a name can address an agent from the CLI and over the API.
///
/// A name starts with a letter and may then hold letters in any script (so a
/// Chinese label works), digits, inner spaces, `-` and `_`. Everything else is
/// rejected, which keeps whitespace from trailing a name, and keeps `:`, quotes
/// and control characters out of a value that may be used as a target. Spaces
/// mean a name typed into a shell needs quoting; the pane id never does.
fn valid_agent_name(name: &str) -> bool {
    if name.chars().count() > MAX_AGENT_NAME_CHARS {
        return false;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_alphabetic() => {}
        _ => return false,
    }
    // A trailing space would be invisible in a target; the panel trims, but the
    // server has to hold the same rule on its own.
    if name.ends_with(' ') {
        return false;
    }
    chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | ' '))
}

/// Folds the wide spaces a keyboard or a paste produces into the ASCII space.
///
/// Chinese input methods insert U+3000 by default, and text copied from a web
/// page can carry U+00A0 or a thin space. They all look like a space in a row,
/// so they all mean one here. ASCII whitespace (tabs, newlines) is left alone
/// for the grammar to reject.
fn normalize_agent_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_whitespace() && !character.is_ascii_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

impl App {
    pub(super) fn handle_agent_sessions(&self, id: String) -> String {
        let workspace_roots: Vec<(String, std::path::PathBuf)> = self
            .state
            .workspaces
            .iter()
            .map(|ws| (ws.id.clone(), ws.identity_cwd.clone()))
            .collect();
        // Scanned on every request rather than on a timer. Each connector keeps
        // its own index, so unless a session file changed this is just a walk
        // of `stat` calls over the stores; the first request after start is the
        // one that parses them, and it is answered on this thread.
        let sessions = crate::sessions::collect_sessions(&workspace_roots);
        crate::app::api::responses::encode_success(
            id,
            crate::api::schema::response::ResponseResult::AgentSessions { sessions },
        )
    }

    /// Removes one session file. The panel asks for this instead of deleting it
    /// itself: an app process gets `EPERM` unlinking from the user's store, while
    /// the daemon runs as the user and can.
    pub(super) fn handle_agent_session_delete(
        &self,
        id: String,
        params: crate::api::schema::AgentSessionDeleteParams,
    ) -> String {
        match crate::sessions::delete_session(&params.agent, &params.source_path) {
            Ok(()) => crate::app::api::responses::encode_success(
                id,
                crate::api::schema::response::ResponseResult::AgentSessionDeleted {},
            ),
            Err(message) => {
                crate::app::api::responses::encode_error(id, "session_delete_failed", message)
            }
        }
    }

    pub(super) fn collect_agent_infos(
        &self,
        include_reply: bool,
    ) -> Vec<crate::api::schema::AgentInfo> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| self.agent_info(ws_idx, pane_id, include_reply))
                })
            })
            .collect()
    }

    pub(super) fn reconcile_managed_agent_target(&mut self, target: &str) {
        let Ok(resolved) = self.resolve_agent_target(target) else {
            return;
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return;
        };
        let changed = self
            .state
            .terminals
            .get_mut(&terminal_id)
            .is_some_and(|terminal| terminal.reconcile_managed_agent_at(Instant::now(), false));
        if changed {
            self.state.mark_session_dirty();
            self.schedule_session_save();
            self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        }
    }

    pub(super) fn agent_info_for_target(
        &self,
        target: &str,
    ) -> Result<crate::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.agent_info(resolved.ws_idx, resolved.pane_id, true)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(super) fn focus_agent_target(
        &mut self,
        target: &str,
    ) -> Result<crate::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.state
            .focus_pane_in_workspace(resolved.ws_idx, resolved.pane_id);
        self.state.mark_active_tab_seen();
        self.state.mode = crate::app::Mode::Terminal;
        self.agent_info(resolved.ws_idx, resolved.pane_id, true)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(super) fn rename_agent_target(
        &mut self,
        target: &str,
        name: Option<String>,
    ) -> Result<crate::api::schema::AgentInfo, AgentRenameError> {
        let resolved = self
            .resolve_agent_target(target)
            .map_err(AgentRenameError::Target)?;
        let normalized_name = match name {
            Some(name) => {
                let name = normalize_agent_name(&name);
                if !valid_agent_name(&name) {
                    return Err(AgentRenameError::InvalidName);
                }
                Some(name)
            }
            None => None,
        };

        // Collect the running keys before taking a mutable borrow of one
        // terminal, so lazy touch sees every agent including this one.
        let in_use_keys: BTreeSet<String> = self
            .state
            .terminals
            .values()
            .flat_map(|terminal| terminal.agent_name_in_use_keys())
            .collect();

        let Some(terminal) = self
            .state
            .terminals
            .values_mut()
            .find(|terminal| terminal.id.to_string() == resolved.terminal_id)
        else {
            return Err(AgentRenameError::Target(TerminalTargetError::NotFound {
                target: target.to_string(),
            }));
        };
        if terminal.managed_agent_launch_pending() {
            return Err(AgentRenameError::PendingLaunch);
        }
        if terminal.effective_agent_label().is_none() {
            return Err(AgentRenameError::NotAgent);
        }
        let session_identity = terminal.agent_name_identity();
        match normalized_name {
            Some(name) => {
                if let Some((source, value)) = session_identity.as_ref() {
                    if let Err(err) = crate::persist::agent_names::store(
                        source,
                        value,
                        &name,
                        &in_use_keys,
                    ) {
                        tracing::error!(err = %err, "failed to persist manual agent name");
                        return Err(AgentRenameError::StoreFailed(err.to_string()));
                    }
                    // The identity is known and the name is safely on disk; a
                    // stale pending value must not overwrite it on a later
                    // identity change.
                    terminal.clear_pending_manual_agent_name();
                } else {
                    // No identity yet; keep the choice in memory so the first
                    // session report can persist it.
                    terminal.set_pending_manual_agent_name(name.clone());
                }
                terminal.set_agent_name(name);
                terminal.mark_agent_name_manual();
            }
            None => {
                if let Err(err) = crate::persist::agent_names::clear_all() {
                    tracing::error!(err = %err, "failed to clear the agent name table");
                    return Err(AgentRenameError::StoreFailed(err.to_string()));
                }
                terminal.clear_agent_name();
            }
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();
        self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        self.agent_info(resolved.ws_idx, resolved.pane_id, true)
            .ok_or_else(|| {
                AgentRenameError::Target(TerminalTargetError::NotFound {
                    target: target.to_string(),
                })
            })
    }

    pub(super) fn start_agent(
        &mut self,
        params: AgentStartParams,
    ) -> Result<(crate::api::schema::AgentInfo, Vec<String>), AgentStartError> {
        let Some(kind) = crate::detect::parse_agent_label(&params.kind) else {
            return Err(AgentStartError::UnsupportedKind(params.kind));
        };
        if params
            .args
            .iter()
            .any(|arg| arg.chars().any(char::is_control))
        {
            return Err(AgentStartError::InvalidArgument);
        }
        // A resumed pane reports the session it is showing, so the panel can
        // tell that session is already open instead of listing it again. The
        // argv path only recognises Codex's launch form, so `resume_session`
        // states the identity directly.
        let persisted_agent_session = match params.resume_session.as_deref() {
            Some(session) => persisted_session_for_resume(kind, session),
            None => crate::agent_resume::persisted_session_from_launch_args(kind, &params.args),
        };
        let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(&params.pane_id) else {
            return Err(AgentStartError::TargetNotFound(params.pane_id));
        };
        // Naming has one home: the server. An explicit name is validated; an
        // absent one prefers the remembered name for the resumed session and
        // otherwise falls back to space, kind and tab — the same call the
        // detection path uses for agents started from the command line.
        let name = self.agent_start_name(
            &params,
            kind,
            ws_idx,
            pane_id,
            persisted_agent_session.as_ref(),
        )?;
        let terminal_id = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|workspace| workspace.terminal_id(pane_id))
            .cloned()
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        let terminal = self
            .state
            .terminals
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        if terminal.is_agent_terminal() || terminal.managed_agent_kind().is_some() {
            return Err(AgentStartError::TargetBusy(params.pane_id));
        }
        let runtime = self
            .terminal_runtimes
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetUnavailable(params.pane_id.clone()))?;
        let shell_name = available_shell_name(runtime)
            .ok_or_else(|| AgentStartError::TargetBusy(params.pane_id.clone()))?;

        // A resume plan replaces the argv tail, not the whole argv: the
        // executable still comes from `interactive_agent_executable`, so the
        // command name has one home. Combining the two forms is a caller bug,
        // and silently preferring one would hide it.
        let mut argv = vec![crate::detect::interactive_agent_executable(kind).to_string()];
        if let Some(session) = params.resume_session.as_deref() {
            if !params.args.is_empty() {
                return Err(AgentStartError::InvalidArgument);
            }
            argv.extend(resume_args_for(kind, session)?);
        } else {
            argv.extend(params.args);
        }
        let command = crate::platform::interactive_shell_command(&argv, &shell_name)
            .ok_or(AgentStartError::InvalidArgument)?;
        let bytes = crate::app::api_helpers::encode_api_submission(runtime, &command);
        let timeout = Duration::from_millis(
            params
                .timeout_ms
                .unwrap_or(DEFAULT_AGENT_START_TIMEOUT.as_millis() as u64),
        );
        if timeout <= AGENT_START_SETTLE_DELAY || timeout > MAX_AGENT_START_TIMEOUT {
            return Err(AgentStartError::InvalidTimeout);
        }

        runtime
            .validate_input_submission(&bytes)
            .map_err(|err| AgentStartError::InputFailed(err.to_string()))?;

        let now = Instant::now();
        let terminal = self
            .state
            .terminals
            .get_mut(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetUnavailable(params.pane_id.clone()))?;
        terminal.begin_managed_agent(name.clone(), kind, now, AGENT_START_SETTLE_DELAY, timeout);
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            terminal.clear_agent_name();
            return Err(AgentStartError::InputFailed(err.to_string()));
        }
        if let Some(session) = persisted_agent_session {
            terminal.set_managed_agent_launch_session(session);
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();

        let agent = self
            .agent_info(ws_idx, pane_id, true)
            .ok_or(AgentStartError::TargetUnavailable(params.pane_id))?;
        Ok((agent, argv))
    }

    pub(super) fn agent_start_error_body(
        &self,
        err: AgentStartError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            AgentStartError::InvalidName => crate::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentStartError::UnsupportedKind(kind) => crate::api::schema::ErrorBody {
                code: "unsupported_agent_kind".into(),
                message: format!("unsupported interactive agent kind {kind}"),
            },
            AgentStartError::InvalidArgument => crate::api::schema::ErrorBody {
                code: "invalid_agent_argument".into(),
                message: "agent arguments cannot be encoded for the target shell, resume_session is not a usable session id, or both were sent".into(),
            },
            AgentStartError::ResumeUnsupported(agent) => crate::api::schema::ErrorBody {
                code: "resume_unsupported".into(),
                message: format!("{agent} has no resume command known to this server"),
            },
            AgentStartError::InvalidTimeout => crate::api::schema::ErrorBody {
                code: "invalid_agent_timeout".into(),
                message: INVALID_AGENT_TIMEOUT_MESSAGE.into(),
            },
            AgentStartError::TargetNotFound(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_not_found".into(),
                message: format!("agent target pane {target} not found"),
            },
            AgentStartError::TargetBusy(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_busy".into(),
                message: format!("agent target pane {target} is not an available shell"),
            },
            AgentStartError::TargetUnavailable(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_unavailable".into(),
                message: format!("agent target pane {target} has no live terminal"),
            },
            AgentStartError::InputFailed(message) => crate::api::schema::ErrorBody {
                code: "agent_start_input_failed".into(),
                message,
            },
            AgentStartError::DuplicateName { name, candidates } => crate::api::schema::ErrorBody {
                code: "agent_name_taken".into(),
                message: format!(
                    "agent name {name} is already used; candidates: {}",
                    candidates
                        .into_iter()
                        .map(|candidate| format!(
                            "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                            candidate.terminal_id,
                            candidate.pane_id,
                            candidate.workspace_id,
                            candidate.tab_id,
                            candidate.cwd.unwrap_or_else(|| "unknown".into()),
                            candidate.agent_status,
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }

    pub(super) fn agent_target_error_body(
        &self,
        err: TerminalTargetError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            TerminalTargetError::NotFound { target } => crate::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: format!("agent target {target} not found"),
            },
            TerminalTargetError::Ambiguous { target, candidates } => {
                crate::api::schema::ErrorBody {
                    code: "agent_target_ambiguous".into(),
                    message: format!(
                        "agent target {target} is ambiguous; candidates: {}",
                        candidates
                            .into_iter()
                            .map(|candidate| format!(
                                "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                                candidate.terminal_id,
                                candidate.pane_id,
                                candidate.workspace_id,
                                candidate.tab_id,
                                candidate.cwd.unwrap_or_else(|| "unknown".into()),
                                candidate.agent_status,
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                }
            }
        }
    }

    pub(super) fn agent_rename_error_body(
        &self,
        err: AgentRenameError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            AgentRenameError::Target(err) => self.agent_target_error_body(err),
            AgentRenameError::InvalidName => crate::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentRenameError::NotAgent => crate::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: "agent target does not currently host an agent".into(),
            },
            AgentRenameError::PendingLaunch => crate::api::schema::ErrorBody {
                code: "agent_launch_pending".into(),
                message: "agent name cannot change while startup is pending".into(),
            },
            AgentRenameError::StoreFailed(message) => crate::api::schema::ErrorBody {
                code: "agent_rename_store_failed".into(),
                message: format!("failed to persist agent name: {message}"),
            },
        }
    }

    pub(super) fn agent_info(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        include_reply: bool,
    ) -> Option<crate::api::schema::AgentInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_state = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane_state.attached_terminal_id)?;
        if !terminal.is_agent_terminal() {
            return None;
        }
        let pane = self.pane_metadata(ws_idx, pane_id)?;
        // Reading the reply means parsing the agent's own transcript. A status
        // poll runs every few seconds, so it asks for the state without it and
        // the reply is fetched on demand instead.
        //
        // No completion marker is required: an agent waiting for the user has
        // not finished the turn, and the text before its question is exactly
        // what the reader needs to answer.
        // A waiting pane is answered from the screen; its transcript only holds
        // the previous turn, so parsing it here would be discarded work.
        #[cfg(target_env = "ohos")]
        let last_reply = if include_reply
            && pane.agent_status != crate::api::schema::AgentStatus::Blocked
        {
            let session = pane.agent_session.as_ref();
            let session_path = session
                .and_then(|session| session.path.as_deref())
                .map(std::path::Path::new);
            let session_id = session.map(|session| session.value.as_str());
            // The stop hook carried the text already; it is authoritative and
            // does not need the transcript to be flushed.
            let hook_reply = terminal
                .hook_authority
                .as_ref()
                .and_then(|authority| authority.message.as_deref());
            crate::fetch_reply::fetch_reply(
                pane.agent.as_deref().unwrap_or(""),
                session_path,
                session_id,
                pane.cwd.as_deref(),
                hook_reply,
            )
        } else {
            None
        };
        #[cfg(not(target_env = "ohos"))]
        let last_reply: Option<String> = None;
        Some(crate::api::schema::AgentInfo {
            terminal_id: pane.terminal_id,
            name: terminal.agent_name.clone(),
            agent: pane.agent,
            title: pane.title,
            terminal_title: pane.terminal_title,
            terminal_title_stripped: pane.terminal_title_stripped,
            display_agent: pane.display_agent,
            agent_status: pane.agent_status,
            blocked_seq: terminal
                .hook_authority
                .as_ref()
                .map(|authority| authority.blocked_seq)
                .unwrap_or(0),
            screen_detection_skipped: terminal.full_lifecycle_hook_authority_active(),
            state_labels: pane.state_labels,
            tokens: pane.tokens,
            agent_session: pane.agent_session,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
            focused: pane.focused,
            launch_pending: terminal.managed_agent_launch_pending(),
            interactive_ready: terminal.managed_agent_interactive_ready(),
            state_change_seq: terminal.last_agent_state_change_seq.unwrap_or(0),
            completion_seq: terminal.last_agent_completion_seq,
            last_reply,
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            revision: pane.revision,
        })
    }

    /// Resolves the name for a starting agent: an explicit name wins, then the
    /// remembered name for a resumed session, then the automatic
    /// space-kind-tab name. Validation and duplicate detection stay in the
    /// explicit branch, so those semantics are unchanged.
    fn agent_start_name(
        &self,
        params: &AgentStartParams,
        kind: crate::detect::Agent,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        persisted_session: Option<&crate::agent_resume::PersistedAgentSession>,
    ) -> Result<String, AgentStartError> {
        match params.name.as_deref() {
            Some(name) => {
                let name = normalize_agent_name(name);
                if !valid_agent_name(&name) {
                    return Err(AgentStartError::InvalidName);
                }
                let conflicts = self.agent_name_conflicts(&name, "");
                if !conflicts.is_empty() {
                    return Err(AgentStartError::DuplicateName {
                        name,
                        candidates: conflicts,
                    });
                }
                Ok(name)
            }
            None => {
                if let Some(name) = persisted_session.and_then(|session| {
                    crate::persist::agent_names::lookup(
                        &session.source,
                        &session.session_ref.value,
                    )
                }) {
                    return Ok(name);
                }
                let tab_idx = self
                    .state
                    .workspaces
                    .get(ws_idx)
                    .and_then(|workspace| workspace.find_tab_index_for_pane(pane_id))
                    .ok_or_else(|| {
                        AgentStartError::TargetUnavailable(params.pane_id.clone())
                    })?;
                Ok(self.auto_agent_name(ws_idx, tab_idx, kind))
            }
        }
    }

    /// The name every agent gets when nobody supplies one: its space, its kind
    /// and the number the tab strip shows. Both the explicit `agent.start` path
    /// and the detection path call `auto_agent_name_with`, so the rule lives in
    /// one place.
    pub(super) fn auto_agent_name(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        kind: crate::detect::Agent,
    ) -> String {
        auto_agent_name_with(&self.state.workspaces, ws_idx, tab_idx, kind, |name| {
            !self.agent_name_conflicts(name, "").is_empty()
        })
    }

    fn agent_name_conflicts(
        &self,
        name: &str,
        except_terminal_id: &str,
    ) -> Vec<crate::api::schema::AgentInfo> {
        self.collect_agent_infos(true)
            .into_iter()
            .filter(|agent| {
                agent.name.as_deref() == Some(name) && agent.terminal_id != except_terminal_id
            })
            .collect()
    }
}

/// The session identity a resumed pane carries, so `agent.list` reports the
/// session it is showing. The source comes from the same helper the resume argv
/// uses, which keeps a hook report and this identity on one spelling.
fn persisted_session_for_resume(
    kind: crate::detect::Agent,
    session_id: &str,
) -> Option<crate::agent_resume::PersistedAgentSession> {
    let agent = crate::detect::agent_label(kind);
    let session_ref = crate::agent_resume::AgentSessionRef::id(session_id)?;
    Some(crate::agent_resume::PersistedAgentSession {
        source: crate::agent_resume::official_source_for(agent),
        agent: agent.to_string(),
        session_ref,
    })
}

/// The argv tail that resumes one agent session, taken from the single table in
/// `agent_resume`. The plan's first element is the command name and is dropped
/// here: `start_agent` already put that in front, and keeping both would run
/// `codewhale codewhale --resume …`.
fn resume_args_for(
    kind: crate::detect::Agent,
    session_id: &str,
) -> Result<Vec<String>, AgentStartError> {
    let agent = crate::detect::agent_label(kind);
    let session_ref = crate::agent_resume::AgentSessionRef::id(session_id)
        .ok_or(AgentStartError::InvalidArgument)?;
    let source = crate::agent_resume::official_source_for(agent);
    let plan = crate::agent_resume::plan(&source, agent, &session_ref)
        .ok_or_else(|| AgentStartError::ResumeUnsupported(agent.to_string()))?;
    Ok(plan.argv.into_iter().skip(1).collect())
}

fn available_shell_name(runtime: &crate::terminal::TerminalRuntime) -> Option<String> {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return Some("sh".into());
    }
    let child_pid = runtime.child_pid()?;
    if let Some(shell) = crate::platform::available_pane_shell(child_pid) {
        return Some(shell);
    }
    crate::platform::available_pane_shell_from_job(child_pid, foreground_job(runtime)?)
}

fn foreground_job(
    runtime: &crate::terminal::TerminalRuntime,
) -> Option<crate::platform::ForegroundJob> {
    let child_pid = runtime.child_pid()?;
    crate::detect::foreground_job(child_pid).or_else(|| {
        crate::detect::foreground_group_leader_job(runtime.pty_foreground_process_group_id()?)
    })
}

pub(super) fn runtime_hosts_agent(
    runtime: &crate::terminal::TerminalRuntime,
    expected: crate::detect::Agent,
) -> bool {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return true;
    }
    live_runtime_agent(runtime) == Some(expected)
}

fn live_runtime_agent(runtime: &crate::terminal::TerminalRuntime) -> Option<crate::detect::Agent> {
    let job = foreground_job(runtime)?;
    crate::detect::identify_agent_in_job(&job)
        .map(|(agent, _)| agent)
        .or_else(|| {
            job.processes
                .iter()
                .find_map(|process| crate::platform::process_agent_hint(process.pid))
        })
}

pub(super) enum AgentStartError {
    InvalidName,
    UnsupportedKind(String),
    InvalidArgument,
    InvalidTimeout,
    ResumeUnsupported(String),
    TargetNotFound(String),
    TargetBusy(String),
    TargetUnavailable(String),
    InputFailed(String),
    DuplicateName {
        name: String,
        candidates: Vec<crate::api::schema::AgentInfo>,
    },
}

#[derive(Debug)]
pub(super) enum AgentRenameError {
    Target(TerminalTargetError),
    InvalidName,
    NotAgent,
    PendingLaunch,
    StoreFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    #[test]
    fn auto_agent_name_uses_space_kind_and_tab_number() {
        let workspace = crate::workspace::Workspace::test_new("proj");
        let space = workspace.id.to_lowercase();

        let name = auto_agent_name_with(
            &[workspace],
            0,
            0,
            crate::detect::Agent::CodeBuddy,
            |_| false,
        );

        assert_eq!(name, format!("{space}-codebuddy-1"));
    }

    #[test]
    fn auto_agent_name_appends_the_next_free_suffix() {
        let workspace = crate::workspace::Workspace::test_new("proj");
        let space = workspace.id.to_lowercase();
        let base = format!("{space}-codebuddy-1");

        let name = auto_agent_name_with(
            &[workspace],
            0,
            0,
            crate::detect::Agent::CodeBuddy,
            |candidate| candidate == base,
        );

        assert_eq!(name, format!("{base}-2"));
    }

    #[test]
    fn auto_agent_name_keeps_a_renamed_tab() {
        let mut workspace = crate::workspace::Workspace::test_new("proj");
        workspace.tabs[0].set_custom_name("build".into());
        let space = workspace.id.to_lowercase();

        let name =
            auto_agent_name_with(&[workspace], 0, 0, crate::detect::Agent::Claude, |_| false);

        assert_eq!(name, format!("{space}-claude-build"));
    }

    #[test]
    fn a_wide_space_is_read_as_a_space() {
        assert_eq!(normalize_agent_name("总\u{3000}指挥"), "总 指挥");
        assert_eq!(normalize_agent_name("a\u{3000}\u{3000}b"), "a  b");
        // NBSP and a thin space are what a paste from a web page tends to bring.
        assert_eq!(normalize_agent_name("a\u{00a0}b"), "a b");
        assert_eq!(normalize_agent_name("a\u{2009}b"), "a b");
        // ASCII whitespace keeps its own identity so the grammar still rejects it.
        assert_eq!(normalize_agent_name("a\tb"), "a\tb");
        assert!(valid_agent_name(&normalize_agent_name("总\u{3000}指挥")));
    }

    #[test]
    fn agent_names_accept_letters_in_any_script() {
        for name in [
            "a",
            "reviewer-one",
            "reviewer_2",
            "Reviewer",
            "reviewer one",
            "QA lead",
            "总指挥",
            "总 指挥",
            "开发-2",
            &"a".repeat(MAX_AGENT_NAME_CHARS),
            &"总".repeat(MAX_AGENT_NAME_CHARS),
        ] {
            assert!(valid_agent_name(name), "expected {name:?} to be valid");
        }
        for name in [
            "",
            " reviewer",
            "reviewer ",
            "reviewer\tone",
            "1reviewer",
            "-reviewer",
            "reviewer.one",
            "总:指挥",
            &"a".repeat(MAX_AGENT_NAME_CHARS + 1),
        ] {
            assert!(!valid_agent_name(name), "expected {name:?} to be invalid");
        }
    }

    fn test_app() -> crate::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    fn with_temp_config<T>(run: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let base = std::env::temp_dir().join(format!(
            "hicodeer-agents-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &base);
        let result = run();
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        if let Err(err) = std::fs::remove_dir_all(&base) {
            eprintln!("agent test cleanup failed for {}: {err}", base.display());
        }
        result
    }

    fn named_resume_app() -> (crate::app::App, crate::layout::PaneId) {
        let mut app = test_app();
        app.state.workspaces.push(Workspace::test_new("proj"));
        app.state.ensure_test_terminals();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        (app, pane_id)
    }

    fn start_params(name: Option<&str>, resume: Option<&str>) -> AgentStartParams {
        AgentStartParams {
            name: name.map(str::to_string),
            kind: "codebuddy".into(),
            pane_id: String::new(),
            args: vec![],
            resume_session: resume.map(str::to_string),
            timeout_ms: None,
        }
    }

    fn resolved_start_name(
        app: &crate::app::App,
        pane_id: crate::layout::PaneId,
        params: &AgentStartParams,
    ) -> String {
        let kind = crate::detect::parse_agent_label("codebuddy").unwrap();
        let persisted = params
            .resume_session
            .as_deref()
            .and_then(|session| persisted_session_for_resume(kind, session));
        match app.agent_start_name(params, kind, 0, pane_id, persisted.as_ref()) {
            Ok(name) => name,
            Err(_) => panic!("start name resolution should succeed"),
        }
    }

    #[test]
    fn agent_start_name_prefers_remembered_name_for_a_resumed_session() {
        with_temp_config(|| {
            crate::persist::agent_names::store(
                "hicodeer:codebuddy",
                "session-1",
                "reviewer",
                &BTreeSet::new(),
            )
            .unwrap();
            let (app, pane_id) = named_resume_app();
            let name = resolved_start_name(&app, pane_id, &start_params(None, Some("session-1")));
            assert_eq!(name, "reviewer");
        });
    }

    #[test]
    fn agent_start_name_keeps_explicit_name_over_the_table() {
        with_temp_config(|| {
            crate::persist::agent_names::store(
                "hicodeer:codebuddy",
                "session-1",
                "reviewer",
                &BTreeSet::new(),
            )
            .unwrap();
            let (app, pane_id) = named_resume_app();
            let name = resolved_start_name(
                &app,
                pane_id,
                &start_params(Some("chosen"), Some("session-1")),
            );
            assert_eq!(name, "chosen");
        });
    }

    #[test]
    fn agent_start_name_falls_back_to_automatic_when_the_table_misses() {
        with_temp_config(|| {
            let (app, pane_id) = named_resume_app();
            let name =
                resolved_start_name(&app, pane_id, &start_params(None, Some("session-404")));
            let space = app.state.workspaces[0].id.to_lowercase();
            assert_eq!(name, format!("{space}-codebuddy-1"));
        });
    }

    #[test]
    fn rename_reports_store_failure_without_changing_the_in_memory_name() {
        with_temp_config(|| {
            let (mut app, pane_id) = named_resume_app();
            let terminal_id = app.state.workspaces[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            {
                let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
                terminal.detected_agent = Some(crate::detect::Agent::CodeBuddy);
                terminal.set_persisted_agent_session(
                    crate::agent_resume::PersistedAgentSession {
                        source: "hicodeer:codebuddy".into(),
                        agent: "codebuddy".into(),
                        session_ref: crate::agent_resume::AgentSessionRef::id("session-1")
                            .unwrap(),
                    },
                );
                terminal.set_agent_name("original".into());
            }
            // An unknown schema makes the save path fail without letting the
            // store replace the file.
            let path = crate::session::data_dir().join("agent-names.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r#"{"schema_version": 99, "names": {}}"#).unwrap();

            let result = app.rename_agent_target("original", Some("reviewer".into()));
            let body = match result {
                Ok(_) => panic!("a failed store must be reported as an error"),
                Err(err) => app.agent_rename_error_body(err),
            };
            assert_eq!(body.code, "agent_rename_store_failed");
            assert_eq!(
                app.state.terminals[&terminal_id].agent_name.as_deref(),
                Some("original"),
                "the in-memory name must not change before the save succeeds"
            );
        });
    }

    #[test]
    fn a_leftover_name_does_not_make_a_shell_pane_an_agent() {
        let (mut app, pane_id) = named_resume_app();
        let terminal_id = app.state.workspaces[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        {
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            assert!(!terminal.is_agent_terminal());
            terminal.set_agent_name("leftover".into());
            assert!(
                !terminal.is_agent_terminal(),
                "a leftover name must not keep a shell pane in agent.list"
            );
        }
        assert!(
            app.collect_agent_infos(true).is_empty(),
            "agent.list must not contain a shell pane that only carries a name"
        );
    }

    #[test]
    fn pending_manual_name_is_written_when_the_identity_arrives() {
        with_temp_config(|| {
            let (mut app, pane_id) = named_resume_app();
            let terminal_id = app.state.workspaces[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            {
                let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
                terminal.detected_agent = Some(crate::detect::Agent::CodeBuddy);
                // Addressable by name, but still without a session identity.
                terminal.set_agent_name("before".into());
            }

            // No session identity yet, so the rename is held as pending. A
            // failed rename here would make the assertions below meaningless.
            app.rename_agent_target("before", Some("reviewer".into()))
                .unwrap();
            assert_eq!(
                app.state.terminals[&terminal_id].pending_manual_agent_name(),
                Some("reviewer")
            );

            // The identity arrives; session_ref_changed must persist the name.
            app.state
                .handle_app_event(crate::events::AppEvent::AgentSessionReported {
                    pane_id,
                    source: "hicodeer:codebuddy".into(),
                    agent_label: "codebuddy".into(),
                    seq: Some(1),
                    session_ref: crate::agent_resume::AgentSessionRef::id("session-1"),
                    session_start_source: Some("startup".into()),
                });
            assert_eq!(
                crate::persist::agent_names::lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("reviewer")
            );
            assert_eq!(
                app.state.terminals[&terminal_id].pending_manual_agent_name(),
                None
            );
        });
    }

    #[test]
    fn successful_rename_clears_a_stale_pending_name() {
        with_temp_config(|| {
            let (mut app, pane_id) = named_resume_app();
            let terminal_id = app.state.workspaces[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            {
                let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
                terminal.detected_agent = Some(crate::detect::Agent::CodeBuddy);
                terminal.set_agent_name("before".into());
                terminal.set_persisted_agent_session(
                    crate::agent_resume::PersistedAgentSession {
                        source: "hicodeer:codebuddy".into(),
                        agent: "codebuddy".into(),
                        session_ref: crate::agent_resume::AgentSessionRef::id("session-1")
                            .unwrap(),
                    },
                );
                // Simulate a pending value left behind by an earlier identity
                // report that did not trigger session_ref_changed.
                terminal.set_pending_manual_agent_name("stale".into());
            }

            app.rename_agent_target("before", Some("fresh".into()))
                .unwrap();
            assert_eq!(
                app.state.terminals[&terminal_id].pending_manual_agent_name(),
                None,
                "a successful rename must drop a stale pending name"
            );
            assert_eq!(
                crate::persist::agent_names::lookup("hicodeer:codebuddy", "session-1").as_deref(),
                Some("fresh")
            );
        });
    }
}
