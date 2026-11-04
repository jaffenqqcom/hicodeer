//! The pane right-click menu: one source of truth for which items exist, their
//! labels, and the action ids the host sends back to execute them.
//!
//! Both the TUI client (`client/shell/context_menu.rs`) and the API server
//! (`app/api/panes.rs`) build their menu from `pane_context_menu_specs`, so the
//! client's own menu and the menu an outer application renders from the API can
//! never drift apart.

use serde::{Deserialize, Serialize};

pub const RENAME_PANE: &str = "rename_pane";
pub const CLEAR_PANE_NAME: &str = "clear_pane_name";
pub const SWAP_WITH_FOCUSED_PANE: &str = "swap_with_focused_pane";
pub const SPLIT_RIGHT: &str = "split_right";
pub const SPLIT_DOWN: &str = "split_down";
pub const ZOOM: &str = "zoom";
pub const TOGGLE_RIGHT_CLICK: &str = "toggle_right_click";
pub const CLOSE_PANE: &str = "close_pane";

#[derive(Debug, Clone, Copy, PartialEq, Eq, schemars::JsonSchema)]
pub struct PaneContextMenuSpec {
    pub id: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneContextMenuParams {
    /// Public tab id the right-click happened in. When omitted the server uses
    /// the tab currently in front, which is what an outer application that
    /// does not track herdr's tab ids needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    /// Row/column in terminal cells, relative to the pane surface. `None`
    /// resolves to the tab's focused pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneContextMenuInvokeParams {
    pub pane_id: String,
    pub action_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneContextMenuItem {
    pub id: String,
    pub label: String,
}

/// Items for a pane right-click, in the order the menu shows them.
///
/// `has_swap_source` is true when a different pane currently holds focus (the
/// "Swap with focused pane" target). `right_click_passthrough` flips the label
/// of the toggle item, matching the client's own menu.
pub fn pane_context_menu_specs(
    has_manual_label: bool,
    has_swap_source: bool,
    right_click_passthrough: bool,
) -> Vec<PaneContextMenuSpec> {
    let spec = |id, label| PaneContextMenuSpec { id, label };
    let mut items = vec![spec(RENAME_PANE, "Rename pane")];
    if has_manual_label {
        items.push(spec(CLEAR_PANE_NAME, "Clear pane name"));
    }
    if has_swap_source {
        items.push(spec(SWAP_WITH_FOCUSED_PANE, "Swap with focused pane"));
    }
    let toggle_label = if right_click_passthrough {
        "Use HiCodeer right-click menu"
    } else {
        "Send right-clicks to pane"
    };
    items.extend([
        spec(SPLIT_RIGHT, "Split right"),
        spec(SPLIT_DOWN, "Split down"),
        spec(ZOOM, "Zoom"),
        spec(TOGGLE_RIGHT_CLICK, toggle_label),
        spec(CLOSE_PANE, "Close pane"),
    ]);
    items
}
