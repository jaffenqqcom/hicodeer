//! The agent client's own settings file, edited from the editor.
//!
//! The TUI client loads its settings from a TOML file beside the agent state
//! directory. This module reads and writes that same file, so a value changed
//! here is the value the client loads, and a value changed in the client shows
//! up here. Only the settings this build exposes are touched; every other key
//! in the file is left byte for byte as the user wrote it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cmd_client::protocol::AgentControl;
use gpui::{App, AppContext as _, BorrowAppContext as _, Global};
use settings::Settings as _;

use crate::client;

/// File name of the client's settings inside the agent state directory.
const CONFIG_FILE: &str = "agent-config.toml";
/// Section and key of the sound switch.
const SOUND_SECTION: &str = "ui.sound";
const SOUND_KEY: &str = "enabled";
/// Section and key of the notification delivery mode.
const TOAST_SECTION: &str = "ui.toast";
const TOAST_KEY: &str = "delivery";
/// Mode for a state directory this process has to create. The daemon addresses
/// the same path from its own account, so the directory stays writable for it.
const STATE_DIR_MODE: u32 = 0o777;
/// Name of the environment variable that carries the application's sandbox
/// files directory. The settings file lives under it, and this process owns
/// that directory, so it is the writable source of truth.
const SANDBOX_FILES_DIR_ENV: &str = "SANDBOX_FILES_DIR";
/// Directory, under the sandbox files root, that holds the agent state.
const AGENT_STATE_DIR: &str = ".hicodeer";
/// How long a completed integration check counts as current.
const INTEGRATION_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Notification delivery modes, in the order the client's own settings page
/// lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastDelivery {
    Off,
    Herdr,
    Terminal,
    System,
}

impl ToastDelivery {
    pub const ALL: [ToastDelivery; 4] = [
        ToastDelivery::Off,
        ToastDelivery::Herdr,
        ToastDelivery::Terminal,
        ToastDelivery::System,
    ];

    /// Value written to the settings file.
    fn as_str(self) -> &'static str {
        match self {
            ToastDelivery::Off => "off",
            ToastDelivery::Herdr => "herdr",
            ToastDelivery::Terminal => "terminal",
            ToastDelivery::System => "system",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "herdr" => ToastDelivery::Herdr,
            "terminal" => ToastDelivery::Terminal,
            "system" => ToastDelivery::System,
            _ => ToastDelivery::Off,
        }
    }
}

/// The two client settings this build exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSettings {
    pub sound_enabled: bool,
    pub toast: ToastDelivery,
}

/// How long a value read from the file stays valid. The page reads on every
/// repaint, and the file only changes when the user edits it here or in the
/// client, so a short cache keeps the file off the repaint path.
const SETTINGS_CACHE_TTL: Duration = Duration::from_secs(1);

/// The last value read from the settings file.
#[derive(Default)]
pub struct TerminalSettingsCache {
    value: std::sync::Mutex<Option<(Instant, TerminalSettings)>>,
}

impl Global for TerminalSettingsCache {}

impl TerminalSettingsCache {
    fn get(&self) -> Option<TerminalSettings> {
        let value = self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        value
            .as_ref()
            .filter(|(read_at, _)| read_at.elapsed() < SETTINGS_CACHE_TTL)
            .map(|(_, settings)| *settings)
    }

    fn store(&self, settings: TerminalSettings) {
        let mut value = self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *value = Some((Instant::now(), settings));
    }

    fn clear(&self) {
        let mut value = self
            .value
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *value = None;
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            toast: ToastDelivery::Off,
        }
    }
}

/// Installation state the server reports for one integration target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationState {
    NotInstalled,
    Current,
    Outdated,
}

/// One integration target and what the server says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integration {
    pub target: String,
    pub label: String,
    pub state: IntegrationState,
    pub needs_install: bool,
}

impl Integration {
    /// Whether the integration itself is installed and current.
    pub fn installed(&self) -> bool {
        self.state == IntegrationState::Current
    }
}

/// Everything the settings page shows about the agent terminal.
#[derive(Clone, Debug)]
pub struct AgentSettings {
    pub sound_enabled: bool,
    pub toast: ToastDelivery,
    pub auto_install_integrations: bool,
    pub integrations: Vec<Integration>,
    pub integrations_checked: bool,
}

impl Default for AgentSettings {
    fn default() -> Self {
        let settings = TerminalSettings::default();
        Self {
            sound_enabled: settings.sound_enabled,
            toast: settings.toast,
            auto_install_integrations: true,
            integrations: Vec::new(),
            integrations_checked: false,
        }
    }
}

/// Integration state shared with the panel and the settings page.
#[derive(Default)]
pub struct AgentIntegrations {
    pub entries: Vec<Integration>,
    pub checked_at: Option<Instant>,
    pub checking: bool,
}

impl Global for AgentIntegrations {}

/// Resolves the settings file from the endpoint the daemon announced.
pub fn config_path(control: &AgentControl) -> Option<PathBuf> {
    let socket = Path::new(&control.socket_path);
    let state_dir = socket.parent()?.parent()?;
    Some(state_dir.join(CONFIG_FILE))
}

/// Resolves the settings file this process owns.
///
/// The sandbox files directory is the process's own tree, so the file is always
/// writable there and the page works before the daemon has announced anything.
/// The endpoint's socket path is the fallback for builds that do not export it;
/// it names the same directory, because that is how the terminal client derives
/// its own copy.
fn current_config_path() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os(SANDBOX_FILES_DIR_ENV) {
        return Some(
            PathBuf::from(root)
                .join(AGENT_STATE_DIR)
                .join(CONFIG_FILE),
        );
    }
    cmd_client::agent_control()
        .as_ref()
        .and_then(config_path)
}

/// Reads the terminal settings, falling back to the client's defaults for
/// anything the file does not set.
pub fn load(path: &Path) -> TerminalSettings {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_settings(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TerminalSettings::default(),
        Err(error) => {
            log::warn!("agent settings: cannot read {}: {error}", path.display());
            TerminalSettings::default()
        }
    }
}

/// Reads the two values the page owns.
///
/// The file is read line by line rather than through a TOML parser: the writer
/// in this module and the terminal's own writer both emit plain `key = value`
/// lines under a section header, so the same shape that is written is the one
/// read back, and an unrelated syntax problem elsewhere in the file never
/// changes what the page shows.
fn parse_settings(content: &str) -> TerminalSettings {
    let mut settings = TerminalSettings::default();
    let mut section = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(header) = table_header(trimmed) {
            section = header.trim_matches(&['[', ']'][..]).trim().to_string();
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value
            .split('#')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"');
        match (section.as_str(), key.trim()) {
            (SOUND_SECTION, SOUND_KEY) => settings.sound_enabled = value != "false",
            (TOAST_SECTION, TOAST_KEY) => settings.toast = ToastDelivery::from_str(value),
            _ => {}
        }
    }
    settings
}

/// Writes `key = value` into `section`, keeping every other line as it was.
///
/// A second copy of the key inside the same section is dropped rather than
/// replaced, so an edited file cannot end up with a duplicate key.
fn upsert_section_value(content: &str, section: &str, key: &str, value: &str) -> String {
    let header = format!("[{section}]");
    let assignment = format!("{key} = {value}");
    let lines: Vec<&str> = content.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut found_section = false;
    let mut written = false;
    let mut in_section = false;

    for line in &lines {
        let trimmed = line.trim();

        if let Some(current) = table_header(trimmed) {
            if in_section && !written {
                result.push(assignment.clone());
                written = true;
            }
            in_section = current == header;
            found_section |= in_section;
            result.push((*line).to_string());
            continue;
        }

        if in_section && assignment_key(trimmed) == Some(key) {
            if !written {
                result.push(assignment.clone());
                written = true;
            }
            continue;
        }

        result.push((*line).to_string());
    }

    if in_section && !written {
        result.push(assignment.clone());
    }

    if !found_section {
        if !result.is_empty() && !result.last().is_some_and(|line| line.trim().is_empty()) {
            result.push(String::new());
        }
        result.push(header);
        result.push(assignment);
    }

    let mut content = result.join("\n");
    content.push('\n');
    content
}

/// The key of a `key = value` line, with any surrounding whitespace removed.
fn assignment_key(line: &str) -> Option<&str> {
    line.split_once('=').map(|(key, _)| key.trim())
}

/// A table header with any trailing comment removed, so `[ui.toast]  # note`
/// still names the table the writer has to edit. A commented-out header has no
/// content before the `#`, so it is not a header.
fn table_header(line: &str) -> Option<&str> {
    let line = line.split('#').next()?.trim();
    (line.starts_with('[') && line.ends_with(']')).then_some(line)
}

/// Replaces the settings file atomically, so the client never reads a partial
/// write while the editor is saving.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other("settings path has no directory"));
    };
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // `create_dir_all` is idempotent, so this also covers a directory that
        // another process created first; the mode is re-applied each save so the
        // daemon's account keeps read and write access.
        let permissions = std::fs::Permissions::from_mode(STATE_DIR_MODE);
        if let Err(error) = std::fs::set_permissions(parent, permissions) {
            log::warn!(
                "agent settings: cannot widen {} ({error})",
                parent.display()
            );
        }
    }
    let temporary = parent.join(format!("{CONFIG_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, path)
}

fn update(section: &str, key: &str, value: &str) -> Result<(), String> {
    let path = current_config_path()
        .ok_or_else(|| "cannot locate the agent settings file".to_string())?;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        // A missing file is the normal first-save case; anything else would be
        // overwritten with only the new key, so it must not be treated as empty.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            log::error!("agent settings: cannot read {}: {error}", path.display());
            return Err(format!("cannot read {}: {error}", path.display()));
        }
    };
    let updated = upsert_section_value(&content, section, key, value);
    write_atomically(&path, &updated)
        .map_err(|error| format!("cannot save {}: {error}", path.display()))
}

/// Writes the sound switch and asks the agent server to broadcast the change.
pub fn set_sound_enabled(enabled: bool, cx: &mut App) -> Result<(), String> {
    update(SOUND_SECTION, SOUND_KEY, &enabled.to_string())?;
    forget_settings(cx);
    request_client_reload(cx);
    Ok(())
}

/// Writes the notification delivery mode and asks the agent server to
/// broadcast the change.
pub fn set_toast_delivery(delivery: ToastDelivery, cx: &mut App) -> Result<(), String> {
    update(
        TOAST_SECTION,
        TOAST_KEY,
        &format!("\"{}\"", delivery.as_str()),
    )?;
    forget_settings(cx);
    request_client_reload(cx);
    Ok(())
}

/// Drops the cached value after a write, so the next repaint reads what was
/// just saved instead of the value from before the click.
fn forget_settings(cx: &App) {
    if let Some(cache) = cx.try_global::<TerminalSettingsCache>() {
        cache.clear();
    }
}

/// Asks the server to re-read its configuration. The server answers by asking
/// every connected client to re-read its own settings, which is the same path
/// its own settings page takes.
fn request_client_reload(cx: &mut App) {
    if cmd_client::agent_control().is_none() {
        return;
    }
    cx.spawn(async move |cx| {
        let outcome = cx.background_spawn(async { client::reload_config() }).await;
        if let Err(error) = outcome {
            log::warn!("agent settings: reload request failed: {error}");
        }
    })
    .detach();
}

/// Reads what the settings page shows.
///
/// The value is cached for `SETTINGS_CACHE_TTL` because the page reads it on
/// every repaint, and a failed read is cached the same way so an unreadable
/// file cannot flood the log.
pub fn snapshot(cx: &App) -> AgentSettings {
    let settings = cached_settings(cx);
    let integrations = cx.global::<AgentIntegrations>();
    AgentSettings {
        sound_enabled: settings.sound_enabled,
        toast: settings.toast,
        auto_install_integrations: crate::AgentPanelSettings::get_global(cx)
            .auto_install_integrations,
        integrations: integrations.entries.clone(),
        integrations_checked: integrations.checked_at.is_some(),
    }
}

fn cached_settings(cx: &App) -> TerminalSettings {
    let Some(cache) = cx.try_global::<TerminalSettingsCache>() else {
        return current_config_path()
            .map(|path| load(&path))
            .unwrap_or_default();
    };
    if let Some(settings) = cache.get() {
        return settings;
    }
    let settings = current_config_path()
        .map(|path| load(&path))
        .unwrap_or_default();
    cache.store(settings);
    settings
}

/// Installs one integration target and refreshes the list afterwards.
pub fn install_integration(target: String, cx: &mut App) {
    {
        let state = cx.global::<AgentIntegrations>();
        if state.checking {
            return;
        }
    }
    cx.update_global::<AgentIntegrations, _>(|state, _| state.checking = true);
    cx.spawn(async move |cx| {
        let entries = cx
            .background_spawn(async move { client::install_integration(&target) })
            .await;
        cx.update(|cx| {
            cx.update_global::<AgentIntegrations, _>(|state, _| {
                state.checking = false;
                state.checked_at = Some(Instant::now());
                match entries {
                    Ok(entries) => state.entries = entries,
                    Err(error) => log::warn!("agent settings: install failed: {error}"),
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Refreshes the integration list, installing whatever is missing when
/// `auto_install` is set.
pub fn refresh_integrations(cx: &mut App, auto_install: bool) {
    {
        let state = cx.global::<AgentIntegrations>();
        if state.checking {
            return;
        }
    }
    cx.update_global::<AgentIntegrations, _>(|state, _| state.checking = true);
    cx.spawn(async move |cx| {
        let entries = cx
            .background_spawn(async move { client::refresh_integrations(auto_install) })
            .await;
        cx.update(|cx| {
            cx.update_global::<AgentIntegrations, _>(|state, _| {
                state.checking = false;
                state.checked_at = Some(Instant::now());
                match entries {
                    Ok(entries) => state.entries = entries,
                    Err(error) => log::warn!("agent settings: integration check failed: {error}"),
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Whether an automatic check is due, so the panel's refresh loop runs it
/// instead of doing it on every tick.
pub fn automatic_check_due(cx: &App) -> bool {
    let state = cx.global::<AgentIntegrations>();
    if state.checking {
        return false;
    }
    match state.checked_at {
        Some(checked_at) => checked_at.elapsed() >= INTEGRATION_CHECK_INTERVAL,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_keeps_other_keys_and_adds_a_missing_section() {
        let original = "[ui]\nsidebar_width = 26\n";
        let updated = upsert_section_value(original, TOAST_SECTION, TOAST_KEY, "\"herdr\"");

        assert!(updated.contains("sidebar_width = 26"));
        assert!(updated.contains("[ui.toast]\ndelivery = \"herdr\""));
        assert_eq!(parse_settings(&updated).toast, ToastDelivery::Herdr);
    }

    #[test]
    fn upsert_replaces_in_place_without_duplicating_the_key() {
        let original = "[ui]\nsidebar_width = 26\n\n[ui.toast]\ndelivery = \"off\"\n";
        let updated = upsert_section_value(original, TOAST_SECTION, TOAST_KEY, "\"system\"");

        assert_eq!(updated.matches("delivery").count(), 1);
        assert!(updated.contains("sidebar_width = 26"));
        assert_eq!(parse_settings(&updated).toast, ToastDelivery::System);
    }

    #[test]
    fn parse_reads_both_settings_and_defaults_the_rest() {
        let settings = parse_settings("[ui.sound]\nenabled = false\n\n[ui.toast]\ndelivery = \"terminal\"\n");
        assert!(!settings.sound_enabled);
        assert_eq!(settings.toast, ToastDelivery::Terminal);

        let defaults = parse_settings("[ui]\nsidebar_width = 26\n");
        assert!(defaults.sound_enabled);
        assert_eq!(defaults.toast, ToastDelivery::Off);
    }
}
