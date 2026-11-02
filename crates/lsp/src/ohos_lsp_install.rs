//! [OHOS PORT] Installs language servers through the command daemon.
//!
//! On OHOS the application sandbox cannot host language servers: they are
//! executed by the command daemon, which runs under a different account and
//! cannot reach the application's files. Instead of downloading archives into
//! the application and syncing them across, every server is installed globally
//! with npm, which puts its executable on the daemon's `PATH`; `which` (already
//! routed to the daemon) is then the single existence check.
//!
//! A global install takes whatever the registry tags as `latest` at the moment
//! it runs, and is left alone afterwards: upgrading an installed server is the
//! user's call, not something checked on every launch.
//!
//! Only packages that are pure JavaScript can be installed this way. Servers
//! shipped as native binaries (clangd, gopls, ty, ruff) have no OHOS build and
//! are intentionally absent from the table. So are the `typescript` and
//! `eslint` adapters: they never look a binary up with `which`, so an install
//! could not make them start.

use std::sync::OnceLock;
use std::time::Duration;

use async_io::Timer;
use futures::future::Either;
use futures::lock::Mutex;
use util::command;

/// npm executable. Resolved by the daemon, whose environment carries node.
const NPM_PROGRAM: &str = "npm";

/// Deadline for one global install. A package over a slow link still needs room
/// to finish, but a launch must not wait longer than this.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Serializes global installs: concurrent calls would write the same npm prefix.
static NPM_INSTALL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Packages this process installed to completion.
///
/// Several servers share one package (the CSS and JSON servers both come from
/// `vscode-langservers-extracted`), so two launches can queue on the install
/// lock for the same npm command. Whoever gets the lock second finds the
/// package already complete here and skips the rerun. Entries are process-wide
/// only; a restart re-checks through `which` as usual.
static COMPLETED_INSTALLS: OnceLock<parking_lot::Mutex<Vec<&'static str>>> = OnceLock::new();

/// What an install attempt left behind.
///
/// The caller turns this into the message the user sees, because "the daemon is
/// not running" and "this server is not installed" need different fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The package is installed and should now resolve through `which`.
    Installed,
    /// The command daemon could not be reached, so nothing could be located or
    /// run. The user has to start hicodeerd.
    DaemonUnavailable,
    /// No npm package is known for this server; the user has to install it.
    NoPackage,
    /// npm ran but did not report success.
    Failed,
}

/// npm package that provides the given language server.
///
/// Keyed by the adapter's `name()`, which is what the caller passes in. That is
/// not always the binary the adapter looks up with `which` (the JSON server is
/// named `json-language-server` but its executable is
/// `vscode-json-language-server`), so the entry only has to cover the package;
/// the second `which` in the caller decides whether the install worked.
fn npm_package_for(server_name: &str) -> Option<&'static str> {
    match server_name {
        "vtsls" => Some("@vtsls/language-server"),
        "json-language-server" | "vscode-css-language-server" => {
            Some("vscode-langservers-extracted")
        }
        "yaml-language-server" => Some("yaml-language-server"),
        "bash-language-server" => Some("bash-language-server"),
        "pyright" => Some("pyright"),
        "basedpyright" => Some("basedpyright"),
        "tailwindcss-language-server" => Some("@tailwindcss/language-server"),
        _ => None,
    }
}

/// Installs `server_name` globally so the daemon can execute it.
///
/// The daemon is probed first: with it down, neither the npm lookup nor the
/// install can run, and reporting "not installed" would send the user to
/// install something that is already there.
pub async fn ensure_installed(server_name: &str) -> InstallOutcome {
    if !daemon_reachable().await {
        log::warn!("ohos_lsp_install: the command daemon is unreachable");
        return InstallOutcome::DaemonUnavailable;
    }

    let Some(package) = npm_package_for(server_name) else {
        log::warn!("ohos_lsp_install: no npm package known for language server {server_name}");
        return InstallOutcome::NoPackage;
    };

    let install_lock = NPM_INSTALL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = install_lock.lock().await;

    // A second language can map to the same package and was queued behind this
    // lock; the wait already ended with the package installed.
    let completed = COMPLETED_INSTALLS.get_or_init(|| parking_lot::Mutex::new(Vec::new()));
    if completed.lock().contains(&package) {
        log::debug!("ohos_lsp_install: {package} was installed while waiting for the lock");
        return InstallOutcome::Installed;
    }

    log::info!("ohos_lsp_install: installing {package} for {server_name}");
    let arguments = ["install", "--global", package];
    let output = match run_with_deadline(NPM_PROGRAM, &arguments, INSTALL_TIMEOUT, "npm install")
        .await
    {
        Ok(Some(output)) => output,
        Ok(None) => return InstallOutcome::Failed,
        Err(_) => return InstallOutcome::DaemonUnavailable,
    };
    if output.status.success() {
        completed.lock().push(package);
        log::info!("ohos_lsp_install: installed {package} for {server_name}");
        return InstallOutcome::Installed;
    }

    log::warn!(
        "ohos_lsp_install: npm install {package} failed: status={:?} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    InstallOutcome::Failed
}

/// Checks whether the command daemon answers at all.
///
/// Runs a shell builtin, so a failure means the connection is gone rather than
/// a missing program.
async fn daemon_reachable() -> bool {
    // Absolute path on purpose: a bare `sh` would be answered by the on-device
    // HNP copy if one is ever shipped, and the probe would then always succeed
    // regardless of the daemon.
    let mut command = command::new_command("/bin/sh");
    command.args(["-c", "true"]);
    match command.output().await {
        Ok(output) => output.status.success(),
        Err(error) => {
            log::warn!("ohos_lsp_install: cannot reach the command daemon: {error}");
            false
        }
    }
}

/// Runs a command through the daemon and gives up after `timeout`, killing it.
///
/// `Err` means the command could not be started at all (the daemon is gone);
/// `Ok(None)` means it started but did not finish in time.
async fn run_with_deadline(
    program: &str,
    arguments: &[&str],
    timeout: Duration,
    description: &str,
) -> std::io::Result<Option<std::process::Output>> {
    let mut command = command::new_command(program);
    command.args(arguments);
    command.kill_on_drop(true);
    // The deadline is polled first so its timer is registered before
    // `output()` performs the synchronous process hand-off. Otherwise that
    // hand-off (up to the pool's reconnect budget) would run before the clock
    // started and would not count against the timeout.
    let selected = futures::future::select(
        Box::pin(Timer::after(timeout)),
        Box::pin(command.output()),
    )
    .await;
    match selected {
        Either::Left((_, _)) => {
            log::warn!(
                "ohos_lsp_install: {description} timed out after {timeout:?}; \
                 an interrupted install is repaired by the next attempt"
            );
            Ok(None)
        }
        Either::Right((Ok(output), _)) => Ok(Some(output)),
        Either::Right((Err(error), _)) => {
            log::warn!("ohos_lsp_install: {description} could not run: {error}");
            Err(error)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Every adapter that goes through `which` must have a package here; a
    /// missing entry shows up as "not installed" after waiting out a probe.
    #[test]
    fn known_servers_map_to_their_package() {
        let cases = [
            ("vtsls", "@vtsls/language-server"),
            ("json-language-server", "vscode-langservers-extracted"),
            ("vscode-css-language-server", "vscode-langservers-extracted"),
            ("yaml-language-server", "yaml-language-server"),
            ("bash-language-server", "bash-language-server"),
            ("pyright", "pyright"),
            ("basedpyright", "basedpyright"),
            ("tailwindcss-language-server", "@tailwindcss/language-server"),
        ];
        for (server, package) in cases {
            assert_eq!(npm_package_for(server), Some(package), "server {server}");
        }
    }

    /// Servers shipped as native binaries have no npm package; mapping one here
    /// would make the caller run an install that can never satisfy `which`.
    #[test]
    fn native_servers_have_no_package() {
        for server in [
            "clangd",
            "gopls",
            "ty",
            "ruff",
            "eslint",
            "typescript-language-server",
            "package-version-server",
        ] {
            assert_eq!(npm_package_for(server), None, "server {server}");
        }
    }

    /// The JSON and CSS servers come from one package, so a second install must
    /// be skipped once the first succeeded.
    #[test]
    fn css_and_json_share_one_package() {
        assert_eq!(
            npm_package_for("json-language-server"),
            npm_package_for("vscode-css-language-server")
        );
    }

    /// The adapter names are what the caller passes; the executable the adapter
    /// looks up may differ (json-language-server ships as
    /// vscode-json-language-server), and the table must key on the former.
    #[test]
    fn table_is_keyed_by_adapter_name_not_executable_name() {
        assert_eq!(npm_package_for("vscode-json-language-server"), None);
        assert_eq!(npm_package_for("pyright-langserver"), None);
    }
}
