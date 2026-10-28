use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::env::*;

pub(crate) fn integration_target_label(
    target: crate::api::schema::IntegrationTarget,
) -> &'static str {
    match target {
        crate::api::schema::IntegrationTarget::Pi => "pi",
        crate::api::schema::IntegrationTarget::Omp => "omp",
        crate::api::schema::IntegrationTarget::Claude => "claude",
        crate::api::schema::IntegrationTarget::Codex => "codex",
        crate::api::schema::IntegrationTarget::Copilot => "copilot",
        crate::api::schema::IntegrationTarget::Devin => "devin",
        crate::api::schema::IntegrationTarget::Droid => "droid",
        crate::api::schema::IntegrationTarget::Kimi => "kimi",
        crate::api::schema::IntegrationTarget::Opencode => "opencode",
        crate::api::schema::IntegrationTarget::Kilo => "kilo",
        crate::api::schema::IntegrationTarget::Hermes => "hermes",
        crate::api::schema::IntegrationTarget::Qodercli => "qodercli",
        crate::api::schema::IntegrationTarget::Qwen => "qwen",
        crate::api::schema::IntegrationTarget::Cursor => "cursor",
        crate::api::schema::IntegrationTarget::Mastracode => "mastracode",
        crate::api::schema::IntegrationTarget::AntigravityCli => "antigravity-cli",
        crate::api::schema::IntegrationTarget::Grok => "grok",
        crate::api::schema::IntegrationTarget::CodeBuddy => "codebuddy",
        crate::api::schema::IntegrationTarget::Atomcode => "atomcode",
        crate::api::schema::IntegrationTarget::Deveco => "deveco",
        crate::api::schema::IntegrationTarget::Codewhale => "codewhale",
        crate::api::schema::IntegrationTarget::Zcode => "zcode",
    }
}

pub(crate) fn integration_target_command(
    target: crate::api::schema::IntegrationTarget,
) -> &'static str {
    integration_target_command_names(target)[0]
}

pub(crate) fn integration_target_command_names(
    target: crate::api::schema::IntegrationTarget,
) -> &'static [&'static str] {
    match target {
        crate::api::schema::IntegrationTarget::Pi => &["pi"],
        crate::api::schema::IntegrationTarget::Omp => &["omp"],
        crate::api::schema::IntegrationTarget::Claude => &["claude"],
        crate::api::schema::IntegrationTarget::Codex => &["codex"],
        crate::api::schema::IntegrationTarget::Copilot => &["copilot"],
        crate::api::schema::IntegrationTarget::Devin => &["devin"],
        crate::api::schema::IntegrationTarget::Droid => &["droid"],
        crate::api::schema::IntegrationTarget::Kimi => &["kimi"],
        crate::api::schema::IntegrationTarget::Opencode => &["opencode"],
        crate::api::schema::IntegrationTarget::Kilo => &["kilo", "kilo-code"],
        crate::api::schema::IntegrationTarget::Hermes => &["hermes"],
        crate::api::schema::IntegrationTarget::Qodercli => qodercli_command_names(),
        crate::api::schema::IntegrationTarget::Qwen => &["qwen"],
        crate::api::schema::IntegrationTarget::Cursor => cursor_command_names(),
        crate::api::schema::IntegrationTarget::Mastracode => &["mastracode"],
        crate::api::schema::IntegrationTarget::AntigravityCli => &["agy"],
        crate::api::schema::IntegrationTarget::Grok => &["grok"],
        crate::api::schema::IntegrationTarget::CodeBuddy => &["codebuddy"],
        crate::api::schema::IntegrationTarget::Atomcode => &["atomcode"],
        crate::api::schema::IntegrationTarget::Deveco => &["deveco"],
        crate::api::schema::IntegrationTarget::Codewhale => &["codewhale"],
        crate::api::schema::IntegrationTarget::Zcode => &["zcode"],
    }
}

pub(crate) fn cursor_command_names() -> &'static [&'static str] {
    &["cursor-agent"]
}

pub(crate) fn integration_target_supported(target: crate::api::schema::IntegrationTarget) -> bool {
    #[cfg(windows)]
    {
        matches!(
            target,
            crate::api::schema::IntegrationTarget::Pi
                | crate::api::schema::IntegrationTarget::Omp
                | crate::api::schema::IntegrationTarget::Claude
                | crate::api::schema::IntegrationTarget::Codex
                | crate::api::schema::IntegrationTarget::Copilot
                | crate::api::schema::IntegrationTarget::Opencode
                | crate::api::schema::IntegrationTarget::Kilo
                | crate::api::schema::IntegrationTarget::Droid
                | crate::api::schema::IntegrationTarget::Kimi
                | crate::api::schema::IntegrationTarget::Qodercli
                | crate::api::schema::IntegrationTarget::Qwen
                | crate::api::schema::IntegrationTarget::AntigravityCli
                | crate::api::schema::IntegrationTarget::Devin
                | crate::api::schema::IntegrationTarget::Hermes
                | crate::api::schema::IntegrationTarget::Cursor
                | crate::api::schema::IntegrationTarget::Mastracode
                | crate::api::schema::IntegrationTarget::Grok
        )
    }

    #[cfg(not(windows))]
    {
        let _ = target;
        true
    }
}

pub(crate) fn integration_target_available(target: crate::api::schema::IntegrationTarget) -> bool {
    if !integration_target_supported(target) {
        return false;
    }

    integration_target_command_names(target)
        .iter()
        .any(|command| command_available(command))
        || integration_target_install_layout_available(target)
}

#[cfg(windows)]
pub(crate) fn qodercli_command_names() -> &'static [&'static str] {
    &["qodercli", "qoder", "qoderclicn", "qodercn"]
}

#[cfg(not(windows))]
pub(crate) fn qodercli_command_names() -> &'static [&'static str] {
    &["qodercli"]
}

pub(crate) fn integration_target_install_layout_available(
    target: crate::api::schema::IntegrationTarget,
) -> bool {
    match target {
        crate::api::schema::IntegrationTarget::Codex => codex_standalone_binary_available(),
        crate::api::schema::IntegrationTarget::Hermes => hermes_install_layout_available(),
        _ => false,
    }
}

pub(crate) fn command_available(command: &str) -> bool {
    path_directories()
        .into_iter()
        .any(|dir| command_in_directory(&dir, command))
        || user_bin_directories()
            .into_iter()
            .any(|dir| command_in_directory(&dir, command))
}

/// `PATH` as the daemon sees it.
fn path_directories() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default()
}

/// Per-user binary directories the daemon never inherits.
///
/// The daemon is started by the system, not from a login shell, so a directory
/// the user appended in `.zshrc` (`~/npm/bin`, `~/.bun/bin`) is missing from its
/// `PATH`. Without this fallback the panel reports npm-installed agents such as
/// zcode and deveco as unavailable and never installs their integration.
fn user_bin_directories() -> Vec<PathBuf> {
    let Ok(home) = home_dir() else {
        return Vec::new();
    };
    ["npm/bin", ".bun/bin", ".local/bin", ".harmonybrew/bin"]
        .iter()
        .map(|suffix| home.join(suffix))
        .collect()
}

fn command_in_directory(dir: &Path, command: &str) -> bool {
    command_path_candidates(dir, command)
        .into_iter()
        .any(|path| executable_file_exists(&path))
}

pub(crate) fn command_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    let base = dir.join(command);

    #[cfg(not(windows))]
    {
        vec![base]
    }

    #[cfg(windows)]
    {
        if Path::new(command).extension().is_some() {
            return vec![base];
        }

        let mut candidates = vec![base];
        for extension in [".exe", ".cmd", ".bat", ".ps1"] {
            candidates.push(dir.join(format!("{command}{extension}")));
        }
        candidates
    }
}

pub(crate) fn executable_file_exists(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn codex_standalone_binary_available() -> bool {
    let Ok(releases_dir) =
        codex_dir().map(|dir| dir.join("packages").join("standalone").join("releases"))
    else {
        return false;
    };
    let Ok(entries) = fs::read_dir(releases_dir) else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        executable_file_exists(&entry.path().join("bin").join(codex_executable_name()))
    })
}

pub(crate) fn codex_executable_name() -> &'static str {
    if cfg!(windows) {
        "codex.exe"
    } else {
        "codex"
    }
}

pub(crate) fn hermes_install_layout_available() -> bool {
    #[cfg(windows)]
    {
        let Ok(dir) = hermes_dir() else {
            return false;
        };
        [
            dir.join("hermes.exe"),
            dir.join("bin").join("hermes.exe"),
            dir.join("Scripts").join("hermes.exe"),
        ]
        .into_iter()
        .any(|path| executable_file_exists(&path))
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn installed_integration_statuses() -> Vec<super::IntegrationStatus> {
    integration_specs()
        .into_iter()
        .filter_map(|(target, path, expected_version)| {
            if !integration_target_supported(target) {
                return None;
            }
            Some(integration_status_at(target, path.ok()?, expected_version))
        })
        .collect()
}

pub(crate) fn integration_recommendations() -> Vec<super::IntegrationRecommendation> {
    integration_specs()
        .into_iter()
        .filter_map(|(target, path, expected_version)| {
            if !integration_target_supported(target) {
                return None;
            }
            let path = path.ok()?;
            let status = integration_status_at(target, path.clone(), expected_version);
            Some(super::IntegrationRecommendation {
                target,
                label: integration_target_label(target),
                command: integration_target_command(target),
                available: integration_target_available(target)
                    || status.state != super::IntegrationStatusKind::NotInstalled,
                path,
                state: status.state,
            })
        })
        .collect()
}

pub(crate) fn outdated_installed_integrations() -> Vec<super::IntegrationStatus> {
    installed_integration_statuses()
        .into_iter()
        .filter(|status| status.state == super::IntegrationStatusKind::Outdated)
        .collect()
}

fn integration_specs() -> [(
    crate::api::schema::IntegrationTarget,
    io::Result<PathBuf>,
    u32,
); 22] {
    [
        (
            crate::api::schema::IntegrationTarget::Pi,
            pi_extension_dir().map(|dir| dir.join(super::PI_EXTENSION_INSTALL_NAME)),
            super::PI_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Omp,
            omp_extension_dir().map(|dir| dir.join(super::OMP_EXTENSION_INSTALL_NAME)),
            super::OMP_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Claude,
            claude_dir().map(|dir| dir.join("hooks").join(super::CLAUDE_HOOK_INSTALL_NAME)),
            super::CLAUDE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Codex,
            codex_dir().map(|dir| dir.join(super::CODEX_HOOK_INSTALL_NAME)),
            super::CODEX_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Copilot,
            copilot_dir().map(|dir| dir.join("hooks").join(super::COPILOT_HOOK_INSTALL_NAME)),
            super::COPILOT_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Devin,
            devin_dir().map(|dir| dir.join(super::DEVIN_HOOK_INSTALL_NAME)),
            super::DEVIN_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Droid,
            droid_dir().map(|dir| dir.join("hooks").join(super::DROID_HOOK_INSTALL_NAME)),
            super::DROID_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Kimi,
            kimi_dir().map(|dir| dir.join("hooks").join(super::KIMI_HOOK_INSTALL_NAME)),
            super::KIMI_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Opencode,
            opencode_dir().map(|dir| {
                dir.join("plugins")
                    .join(super::OPENCODE_PLUGIN_INSTALL_NAME)
            }),
            super::OPENCODE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Kilo,
            kilo_dir().map(|dir| dir.join("plugin").join(super::KILO_PLUGIN_INSTALL_NAME)),
            super::KILO_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Hermes,
            hermes_plugin_dir().map(|dir| dir.join(super::HERMES_PLUGIN_INIT_INSTALL_NAME)),
            super::HERMES_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Qodercli,
            qodercli_dir().map(|dir| dir.join("hooks").join(super::QODERCLI_HOOK_INSTALL_NAME)),
            super::QODERCLI_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Qwen,
            qwen_dir().map(|dir| dir.join("hooks").join(super::QWEN_HOOK_INSTALL_NAME)),
            super::QWEN_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Cursor,
            cursor_dir().map(|dir| dir.join(super::CURSOR_HOOK_INSTALL_NAME)),
            super::CURSOR_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Mastracode,
            mastracode_dir().map(|dir| dir.join("hooks").join(super::MASTRACODE_HOOK_INSTALL_NAME)),
            super::MASTRACODE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::AntigravityCli,
            antigravity_cli_dir().map(|dir| {
                dir.join("hooks")
                    .join(super::ANTIGRAVITY_CLI_HOOK_INSTALL_NAME)
            }),
            super::ANTIGRAVITY_CLI_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::CodeBuddy,
            codebuddy_dir().map(|dir| dir.join("hooks").join(super::CODEBUDDY_HOOK_INSTALL_NAME)),
            super::CODEBUDDY_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Grok,
            grok_dir().map(|dir| dir.join("hooks").join(super::GROK_HOOK_INSTALL_NAME)),
            super::GROK_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Deveco,
            deveco_dir().map(|dir| dir.join("plugins").join(super::DEVECO_PLUGIN_INSTALL_NAME)),
            super::DEVECO_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Atomcode,
            atomcode_hooks_path()
                .map(|path| path.parent().unwrap_or_else(|| Path::new(".")).join(super::ATOMCODE_HOOK_INSTALL_NAME)),
            super::ATOMCODE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Codewhale,
            codewhale_config_path().map(|path| {
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("hooks")
                    .join(super::CODEWHALE_HOOK_INSTALL_NAME)
            }),
            super::CODEWHALE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Zcode,
            zcode_settings_path().map(|path| {
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("hooks")
                    .join(super::ZCODE_HOOK_INSTALL_NAME)
            }),
            super::ZCODE_INTEGRATION_VERSION,
        ),
    ]
}

pub(crate) fn integration_update_instructions(
    targets: &[crate::api::schema::IntegrationTarget],
) -> String {
    let commands: Vec<String> = targets
        .iter()
        .map(|target| {
            format!(
                "`herdr integration install {}`",
                integration_target_label(*target)
            )
        })
        .collect();

    match commands.as_slice() {
        [] => String::new(),
        [command] => format!("run {command}"),
        [rest @ .., last] => format!("run {} and {last}", rest.join(", ")),
    }
}

pub(crate) fn print_outdated_update_notice() -> bool {
    let outdated = outdated_installed_integrations();
    if outdated.is_empty() {
        return false;
    }

    let targets = outdated
        .iter()
        .map(|integration| integration.target)
        .collect::<Vec<_>>();
    eprintln!(
        "installed herdr integrations need updating; {}.",
        integration_update_instructions(&targets).replace('`', "")
    );
    true
}

/// Whether the Herdr-owned Grok hook config exactly matches the installed
/// integration. JSON formatting and object key order do not affect validity.
fn grok_hook_config_is_valid(hook_path: &Path) -> bool {
    let Some(hooks_dir) = hook_path.parent() else {
        return false;
    };
    let config_path = hooks_dir.join(super::GROK_HOOK_CONFIG_INSTALL_NAME);
    fs::read_to_string(config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .is_some_and(|config| config == super::targets::grok_hook_config(hook_path))
}

fn opencode_tui_integration_is_valid(plugin_path: &Path, expected_version: u32) -> bool {
    let Some(config_dir) = plugin_path.parent().and_then(Path::parent) else {
        return false;
    };
    let tui_plugin_path = config_dir.join(super::OPENCODE_TUI_PLUGIN_INSTALL_NAME);
    let tui_plugin_current = fs::read_to_string(tui_plugin_path)
        .ok()
        .and_then(|content| parse_integration_version(&content))
        .is_some_and(|version| version >= expected_version);
    tui_plugin_current
        && super::opencode_config::tui_plugin_is_configured(
            config_dir,
            super::OPENCODE_TUI_PLUGIN_SPEC,
        )
        && (!config_dir.join("cli.json").exists()
            || (super::opencode_config::cli_plugin_is_configured(
                config_dir,
                super::OPENCODE_V2_TUI_PLUGIN_SPEC,
            ) && fs::read_to_string(
                config_dir
                    .join(super::OPENCODE_V2_TUI_PLUGIN_DIR)
                    .join("tui.js"),
            )
            .ok()
            .and_then(|content| parse_integration_version(&content))
            .is_some_and(|version| version >= expected_version)))
}

/// DevEco Code loads `deveco.jsonc` plugins only in its headless server and
/// `tui.json` plugins only in the terminal UI, so a server-only install reports
/// no state in the pane. Treat that half-install as outdated so a reinstall
/// writes the TUI plugin.
///
/// The check looks at `tui.json` alone because that is the file the install
/// writes and the uninstall edits: accepting `tui.jsonc` here would let a stale
/// entry in the other file report `current` while the pane stays silent.
fn deveco_tui_integration_is_valid(plugin_path: &Path, expected_version: u32) -> bool {
    let Some(config_dir) = plugin_path.parent().and_then(Path::parent) else {
        return false;
    };
    let tui_plugin_path = config_dir.join(super::DEVECO_TUI_PLUGIN_INSTALL_NAME);
    let tui_plugin_current = fs::read_to_string(tui_plugin_path)
        .ok()
        .and_then(|content| parse_integration_version(&content))
        .is_some_and(|version| version >= expected_version);
    tui_plugin_current
        && super::opencode_config::plugin_is_configured(
            &config_dir.join("tui.json"),
            "plugin",
            super::DEVECO_TUI_PLUGIN_SPEC,
        )
}

fn integration_state_for_path(
    path: &Path,
    expected_version: u32,
) -> (super::IntegrationStatusKind, Option<u32>) {
    if !path.is_file() {
        return (super::IntegrationStatusKind::NotInstalled, None);
    }

    let installed_version = fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_integration_version(&content));
    let state = if installed_version.is_some_and(|version| version >= expected_version) {
        super::IntegrationStatusKind::Current
    } else {
        super::IntegrationStatusKind::Outdated
    };

    (state, installed_version)
}

pub(crate) fn integration_status_at(
    target: crate::api::schema::IntegrationTarget,
    path: PathBuf,
    expected_version: u32,
) -> super::IntegrationStatus {
    let (mut state, installed_version) = integration_state_for_path(&path, expected_version);

    // Grok only invokes the hook when the herdr-owned `hooks/herdr.json`
    // registers it, so a current hook script with a missing or broken config
    // is a nonfunctional install: report it as outdated so `herdr integration
    // status` flags it and a reinstall rewrites both files.
    if target == crate::api::schema::IntegrationTarget::Grok
        && state == super::IntegrationStatusKind::Current
        && !grok_hook_config_is_valid(&path)
    {
        state = super::IntegrationStatusKind::Outdated;
    }
    if target == crate::api::schema::IntegrationTarget::Opencode
        && state == super::IntegrationStatusKind::Current
        && !opencode_tui_integration_is_valid(&path, expected_version)
    {
        state = super::IntegrationStatusKind::Outdated;
    }
    if target == crate::api::schema::IntegrationTarget::Deveco
        && state == super::IntegrationStatusKind::Current
        && !deveco_tui_integration_is_valid(&path, expected_version)
    {
        state = super::IntegrationStatusKind::Outdated;
    }

    super::IntegrationStatus {
        target,
        path,
        state,
        installed_version,
        expected_version,
    }
}

/// Letta is intentionally kept out of the frozen client endpoint
/// `IntegrationTarget` enum so published generation-1 clients never receive an
/// unknown variant. It is installable and reportable as an experimental
/// CLI-only target until the agent registry replaces the enum-keyed registry.
pub(crate) fn experimental_letta_integration_status() -> Option<super::ExperimentalIntegrationStatus>
{
    let path = letta_dir()
        .ok()?
        .join("hooks")
        .join(super::LETTA_HOOK_INSTALL_NAME);
    let (state, installed_version) =
        integration_state_for_path(&path, super::LETTA_INTEGRATION_VERSION);
    Some(super::ExperimentalIntegrationStatus {
        label: "letta",
        path,
        state,
        installed_version,
        expected_version: super::LETTA_INTEGRATION_VERSION,
    })
}

pub(crate) fn parse_integration_version(content: &str) -> Option<u32> {
    content.lines().find_map(|line| {
        let marker_line = line
            .trim()
            .trim_start_matches('/')
            .trim_start_matches('#')
            .trim();
        marker_line
            .strip_prefix(super::INTEGRATION_VERSION_MARKER)?
            .trim()
            .parse()
            .ok()
    })
}
