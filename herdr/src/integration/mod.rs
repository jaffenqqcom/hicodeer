mod actions;
mod atomcode_settings;
mod claude_settings;
mod codewhale_settings;
mod command;
mod config_edit;
mod config_file;
mod env;
mod file_ops;
mod opencode_config;
mod registry;
mod targets;
mod types;
mod version;
mod zcode_settings;

pub(crate) use actions::{
    install_experimental_letta, install_target, uninstall_experimental_letta, uninstall_target,
};
#[cfg(test)]
pub(crate) use env::integration_env_lock;
pub(crate) use env::{
    apply_pane_base_env, HICODEER_AGENT_PANE_ID_ENV_VAR, HICODEER_AGENT_TAB_ID_ENV_VAR, HICODEER_AGENT_WORKSPACE_ID_ENV_VAR,
};
pub(crate) use registry::{
    experimental_letta_integration_status, installed_integration_statuses,
    integration_recommendations, integration_target_label, print_outdated_update_notice,
};

pub(crate) use types::{
    ExperimentalIntegrationStatus, IntegrationRecommendation, IntegrationStatus,
    IntegrationStatusKind,
};

/// CLI labels for experimental integrations that are intentionally not part of
/// the frozen client endpoint `IntegrationTarget` enum. Empty this list once the
/// agent registry provides first-class target registration.
pub(crate) const EXPERIMENTAL_INTEGRATION_TARGET_LABELS: &[&str] = &["letta"];

const PI_EXTENSION_INSTALL_NAME: &str = "hicodeer-agent-state.ts";
const PI_EXTENSION_ASSET: &str = include_str!("assets/pi/hicodeer-agent-state.ts");
const PI_INTEGRATION_VERSION: u32 = 11;
const OMP_EXTENSION_INSTALL_NAME: &str = "hicodeer-omp-agent-state.ts";
const OMP_EXTENSION_ASSET: &str = include_str!("assets/omp/hicodeer-agent-state.ts");
const OMP_INTEGRATION_VERSION: u32 = 12;
const CLAUDE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const CLAUDE_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/claude/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/claude/hicodeer-agent-state.sh")
};
const CLAUDE_INTEGRATION_VERSION: u32 = 16;
const ZCODE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const ZCODE_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/zcode/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/zcode/hicodeer-agent-state.sh")
};
const ZCODE_INTEGRATION_VERSION: u32 = 16;
const CODEWHALE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const CODEWHALE_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/codewhale/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/codewhale/hicodeer-agent-state.sh")
};
const CODEWHALE_INTEGRATION_VERSION: u32 = 16;
const ATOMCODE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const ATOMCODE_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/atomcode/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/atomcode/hicodeer-agent-state.sh")
};
const ATOMCODE_INTEGRATION_VERSION: u32 = 16;
const DEVECO_PLUGIN_INSTALL_NAME: &str = "hicodeer-agent-state.js";
const DEVECO_PLUGIN_ASSET: &str = include_str!("assets/deveco/hicodeer-agent-state.js");
/// DevEco Code runs its TUI as a separate plugin host: `tui.json` plugins only
/// load in the terminal UI, while `deveco.jsonc` plugins only load in the
/// headless server. Both are installed so either entry point reports state.
const DEVECO_TUI_PLUGIN_INSTALL_NAME: &str = "hicodeer-tui-session.js";
const DEVECO_TUI_PLUGIN_SPEC: &str = "./hicodeer-tui-session.js";
const DEVECO_TUI_PLUGIN_ASSET: &str = include_str!("assets/deveco/hicodeer-tui-session.js");
const DEVECO_INTEGRATION_VERSION: u32 = 3;
const CODEBUDDY_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const CODEBUDDY_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/codebuddy/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/codebuddy/hicodeer-agent-state.sh")
};
const CODEBUDDY_INTEGRATION_VERSION: u32 = 7;
const CODEX_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const CODEX_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/codex/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/codex/hicodeer-agent-state.sh")
};
const CODEX_INTEGRATION_VERSION: u32 = 12;
const KIMI_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const KIMI_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/kimi/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/kimi/hicodeer-agent-state.sh")
};
const KIMI_INTEGRATION_VERSION: u32 = 10;
const KIMI_CONFIG_BLOCK_BEGIN: &str = "# >>> hicodeer-agent kimi integration";
const KIMI_CONFIG_BLOCK_END: &str = "# <<< hicodeer-agent kimi integration";
const KIMI_MIN_VERSION: &str = "0.14.0";
const KIMI_ASK_USER_QUESTION_MATCHER: &str = "^AskUserQuestion$";
const KIMI_OTHER_TOOL_MATCHER: &str = "^(?!AskUserQuestion$).*$";
const KIMI_HOOK_EVENTS: [(&str, Option<&str>, &str); 12] = [
    ("SessionStart", None, "session"),
    ("UserPromptSubmit", None, "working"),
    ("PreToolUse", Some(KIMI_OTHER_TOOL_MATCHER), "working"),
    (
        "PreToolUse",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "blocked",
    ),
    (
        "PostToolUse",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "working",
    ),
    (
        "PostToolUseFailure",
        Some(KIMI_ASK_USER_QUESTION_MATCHER),
        "working",
    ),
    ("SubagentStart", None, "working"),
    ("PreCompact", None, "working"),
    ("PermissionRequest", None, "blocked"),
    ("PermissionResult", None, "working"),
    ("Stop", None, "idle"),
    ("Interrupt", None, "idle"),
];
const COPILOT_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const COPILOT_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/copilot/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/copilot/hicodeer-agent-state.sh")
};
const COPILOT_INTEGRATION_VERSION: u32 = 7;
const COPILOT_HOOK_EVENTS: [&str; 7] = [
    "sessionStart",
    "userPromptSubmitted",
    "preToolUse",
    "postToolUse",
    "permissionRequest",
    "notification",
    "agentStop",
];
const COPILOT_REMOVED_LIFECYCLE_HOOK_EVENTS: [&str; 9] = [
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "agentStop",
    "SessionEnd",
    "notification",
    "sessionStart",
];
const DEVIN_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const DEVIN_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/devin/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/devin/hicodeer-agent-state.sh")
};
const DEVIN_INTEGRATION_VERSION: u32 = 5;
const DEVIN_HOOK_EVENTS: [(&str, &str); 6] = [
    ("SessionStart", "session"),
    ("UserPromptSubmit", "session"),
    ("PreToolUse", "session"),
    ("PostToolUse", "session"),
    ("PermissionRequest", "session"),
    ("Stop", "session"),
];
const DEVIN_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 6] = [
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("PermissionRequest", "blocked"),
    ("Stop", "idle"),
    ("SessionEnd", "release"),
];
const DROID_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const DROID_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/droid/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/droid/hicodeer-agent-state.sh")
};
const DROID_INTEGRATION_VERSION: u32 = 9;
const DROID_HOOK_EVENTS: [(&str, Option<&str>, &str); 8] = [
    ("SessionStart", None, "session"),
    ("UserPromptSubmit", None, "working"),
    ("PreToolUse", Some("^AskUser$"), "blocked"),
    ("PreToolUse", Some("^(?!AskUser$).*$"), "working"),
    ("PostToolUse", None, "working"),
    ("Notification", None, "notification"),
    ("Stop", None, "idle"),
    ("SubagentStop", None, "idle"),
];
const DROID_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 9] = [
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("Notification", "blocked"),
    ("Stop", "idle"),
    ("SubagentStop", "working"),
    ("PreCompact", "working"),
    ("SessionEnd", "release"),
];
const OPENCODE_PLUGIN_INSTALL_NAME: &str = "hicodeer-agent-state.js";
const OPENCODE_PLUGIN_ASSET: &str = include_str!("assets/opencode/hicodeer-agent-state.js");
const OPENCODE_TUI_PLUGIN_INSTALL_NAME: &str = "hicodeer-tui-session.js";
const OPENCODE_TUI_PLUGIN_SPEC: &str = "./hicodeer-tui-session.js";
const OPENCODE_TUI_PLUGIN_ASSET: &str = include_str!("assets/opencode/hicodeer-tui-session.js");
const OPENCODE_V2_TUI_PLUGIN_DIR: &str = "hicodeer-opencode";
const OPENCODE_V2_TUI_PLUGIN_SPEC: &str = "./hicodeer-opencode";
const OPENCODE_V2_TUI_PLUGIN_ASSET: &str = include_str!("assets/opencode/tui.js");
const OPENCODE_INTEGRATION_VERSION: u32 = 15;
const KILO_PLUGIN_INSTALL_NAME: &str = "hicodeer-agent-state.js";
const KILO_PLUGIN_ASSET: &str = include_str!("assets/kilo/hicodeer-agent-state.js");
const KILO_INTEGRATION_VERSION: u32 = 6;
const HERMES_PLUGIN_INSTALL_NAME: &str = "hicodeer-agent-state";
const HERMES_PLUGIN_MANIFEST_INSTALL_NAME: &str = "plugin.yaml";
const HERMES_PLUGIN_INIT_INSTALL_NAME: &str = "__init__.py";
const HERMES_PLUGIN_MANIFEST_ASSET: &str = include_str!("assets/hermes/plugin.yaml");
const HERMES_PLUGIN_INIT_ASSET: &str = include_str!("assets/hermes/__init__.py");
const HERMES_INTEGRATION_VERSION: u32 = 6;
const QODERCLI_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const QODERCLI_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/qodercli/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/qodercli/hicodeer-agent-state.sh")
};
const QODERCLI_INTEGRATION_VERSION: u32 = 6;
const QODERCLI_HOOK_EVENTS: [(&str, &str); 1] = [("SessionStart", "session")];
const QWEN_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const QWEN_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/qwen/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/qwen/hicodeer-agent-state.sh")
};
const QWEN_INTEGRATION_VERSION: u32 = 7;
const QWEN_HOOK_EVENTS: [(&str, Option<&str>, &str); 7] = [
    ("SessionStart", None, "session"),
    ("UserPromptSubmit", None, "working"),
    ("PreToolUse", Some("^AskUserQuestion$"), "blocked"),
    ("PreToolUse", Some("^(?!AskUserQuestion$).*$"), "working"),
    ("PermissionRequest", None, "blocked"),
    ("Notification", None, "notification"),
    ("Stop", None, "idle"),
];
const LETTA_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-session.ps1"
} else {
    "hicodeer-agent-session.sh"
};
const LETTA_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/letta/hicodeer-agent-session.ps1")
} else {
    include_str!("assets/letta/hicodeer-agent-session.sh")
};
const LETTA_INTEGRATION_VERSION: u32 = 4;
const LETTA_HOOK_TIMEOUT_MS: u64 = 10_000;
const QODERCLI_REMOVED_LIFECYCLE_HOOK_EVENTS: [(&str, &str); 12] = [
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "working"),
    ("PreToolUse", "working"),
    ("PostToolUse", "working"),
    ("PostToolUseFailure", "working"),
    ("SubagentStart", "working"),
    ("SubagentStop", "working"),
    ("PreCompact", "working"),
    ("Notification", "blocked"),
    ("PermissionRequest", "blocked"),
    ("Stop", "idle"),
    ("SessionEnd", "release"),
];
const CURSOR_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const CURSOR_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/cursor/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/cursor/hicodeer-agent-state.sh")
};
const CURSOR_INTEGRATION_VERSION: u32 = 4;
#[cfg(windows)]
const ANTIGRAVITY_CLI_HOOK_INSTALL_NAME: &str = "hicodeer-agent-state.ps1";
#[cfg(not(windows))]
const ANTIGRAVITY_CLI_HOOK_INSTALL_NAME: &str = "hicodeer-agent-state.sh";
#[cfg(windows)]
const ANTIGRAVITY_CLI_HOOK_ASSET: &str =
    include_str!("assets/antigravity_cli/hicodeer-agent-state.ps1");
#[cfg(not(windows))]
const ANTIGRAVITY_CLI_HOOK_ASSET: &str =
    include_str!("assets/antigravity_cli/hicodeer-agent-state.sh");
const ANTIGRAVITY_CLI_INTEGRATION_VERSION: u32 = 7;
/// Antigravity CLI keys `hooks.json` by hook name, so every Herdr entry lives
/// under one Herdr-owned block that install rewrites and uninstall removes.
const ANTIGRAVITY_CLI_HOOK_BLOCK_NAME: &str = "hicodeer-agent";
const ANTIGRAVITY_CLI_HOOK_TIMEOUT_SEC: u64 = 10;
/// `(event, reported action)`. Session-only: `PreInvocation` is the only event
/// we need because it carries `conversationId`. The others cannot express
/// lifecycle safely — Antigravity CLI has no blocked event, `PostInvocation` is
/// skipped on interruption, and `Stop` is end-of-turn rather than process exit.
/// Screen detection owns agent state instead.
///
/// `PreInvocation` takes a flat handler list; only the `PreToolUse`/`PostToolUse`
/// events accept a `matcher`/`hooks` wrapper, and sending one here would
/// invalidate the whole file.
const ANTIGRAVITY_CLI_HOOK_EVENTS: [(&str, Option<&str>, &str); 4] = [
    ("PreInvocation", None, "session"),
    ("PreToolUse", Some("^ask_question$"), "blocked"),
    ("PostToolUse", None, "working"),
    ("Stop", None, "idle"),
];
const INTEGRATION_VERSION_MARKER: &str = "HICODEER_AGENT_INTEGRATION_VERSION=";
const MASTRACODE_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const MASTRACODE_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/mastracode/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/mastracode/hicodeer-agent-state.sh")
};
const MASTRACODE_INTEGRATION_VERSION: u32 = 6;
const MASTRACODE_HOOK_TIMEOUT_MS: u64 = 10_000;
const MASTRACODE_REMOVED_HOOK_EVENTS: [(&str, &str); 2] =
    [("SessionStart", "idle"), ("SessionEnd", "release")];
const MASTRACODE_HOOK_EVENTS: [(&str, &str); 12] = [
    ("SessionStart", "session"),
    ("UserPromptSubmit", "working"),
    ("AgentStart", "working"),
    ("PreToolUse", "working"),
    ("PermissionRequest", "blocked"),
    ("PermissionResult", "working"),
    ("Notification", "notification"),
    ("SubagentStart", "working"),
    ("SubagentEnd", "working"),
    ("Interrupt", "idle"),
    ("AgentEnd", "idle"),
    ("Stop", "idle"),
];
const GROK_HOOK_INSTALL_NAME: &str = if cfg!(windows) {
    "hicodeer-agent-state.ps1"
} else {
    "hicodeer-agent-state.sh"
};
const GROK_HOOK_CONFIG_INSTALL_NAME: &str = "hicodeer.json";
const GROK_HOOK_ASSET: &str = if cfg!(windows) {
    include_str!("assets/grok/hicodeer-agent-state.ps1")
} else {
    include_str!("assets/grok/hicodeer-agent-state.sh")
};
const GROK_INTEGRATION_VERSION: u32 = 6;

/// The version an embedded asset declares for itself.
///
/// Checks use this when they must follow the asset rather than the target-wide
/// constant, so bumping the asset is the only edit a version change needs.
pub(crate) fn declared_version(asset: &str) -> u32 {
    registry::parse_integration_version(asset).unwrap_or(0)
}

pub(crate) const INSTALL_WARNING_PREFIX: &str = "warning:";

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
