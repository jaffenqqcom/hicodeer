//! User-visible branding for the agent server embedded in HiCodeer.
//!
//! ===== [OHOS PORT BEGIN] =====
//! Upstream Herdr writes its own name into every state directory, socket, log
//! file, pane environment variable and status string. This build ships as part
//! of HiCodeer, so every one of those names derives from the constants below
//! instead of the upstream product name.
//! ===== [OHOS PORT END] =====

/// Product name shown to the user.
pub(crate) const NAME: &str = "HiCodeer";

/// Directory name under `$XDG_CONFIG_HOME` and `$XDG_STATE_HOME`. The server's
/// own state lives in a subdirectory so it never mixes with HiCodeer's node,
/// language and prettier trees.
pub(crate) const DIR: &str = "hicodeer/agenttui";

/// Prefix of files this server creates: sockets, logs, temp files and the
/// per-user state files.
pub(crate) const FILE_PREFIX: &str = "hicodeer";

/// Command name used in help text and diagnostics.
pub(crate) const COMMAND: &str = "hicodeer-agent";

/// Prefix of agent session keys reported over the API (`<prefix>:<agent>`).
pub(crate) const SESSION_KEY_PREFIX: &str = "hicodeer";
