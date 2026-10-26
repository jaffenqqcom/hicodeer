use std::time::Instant;

use crate::terminal::TerminalId;

/// Viewport state for a pane.
///
/// Terminal identity, cwd, labels, and agent metadata live in TerminalState.
pub struct PaneState {
    pub attached_terminal_id: TerminalId,
    /// Whether the user has seen this pane since its last state change to Idle.
    /// False = "Done" (agent finished while user was in another workspace).
    pub seen: bool,
    /// Whether unmodified right-click gestures should be forwarded to the pane application.
    pub right_click_passthrough: bool,
    /// When the user last pressed Esc while this pane's agent was Working or
    /// Blocked. The next state change consumes it: if that change lands within
    /// `ESC_INTERRUPT_WINDOW` and is a completion, the user stopped the turn
    /// themselves and its notification is suppressed.
    ///
    /// Deliberately not persisted and not cloned: a rebuilt pane starts clean,
    /// which keeps a stale mark from ever crossing a restart.
    pub interrupted_at: Option<Instant>,
}

impl PaneState {
    pub fn new(attached_terminal_id: TerminalId) -> Self {
        Self {
            attached_terminal_id,
            seen: true,
            right_click_passthrough: false,
            interrupted_at: None,
        }
    }
}
