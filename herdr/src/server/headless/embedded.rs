//! In-process embedding of the headless server.
//!
//! `build` performs the synchronous half of `run_server` (config load, API
//! listener, app construction, client listener) without creating a runtime or
//! touching process-wide signal handlers, so an embedding host can drive the
//! returned [`EmbeddedServer`] from its own runtime.

use super::*;

/// A headless server built on the caller's runtime.
pub struct EmbeddedServer {
    server: HeadlessServer,
}

/// Synchronous half of `run_server`: everything up to (but excluding) the
/// event loop. Returns an error instead of exiting the process when a socket is
/// already in use, because the host process owns the exit policy.
pub fn build() -> io::Result<EmbeddedServer> {
    crate::platform::raise_server_nofile_limit();

    let loaded_config = config::Config::load();
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();
    let server_stop = crate::server::shutdown::ServerStop::default();

    let api_server = api::start_server_with_stop_control(
        api_tx.clone(),
        event_hub.clone(),
        server_stop.clone(),
    )?;

    let mut app = app::App::try_new(
        &loaded_config.config,
        app::AppPolicy::PRODUCTION,
        config::config_diagnostic_summary(&loaded_config.diagnostics),
        api_rx,
        event_hub,
    )?;
    super::bootstrap::seed_startup_workspace_if_empty(&mut app);

    let server = HeadlessServer::new(
        app,
        &loaded_config.diagnostics,
        Some(api_tx),
        Some(api_server),
        server_stop,
    )?;

    info!(
        api_socket = %api::socket_path().display(),
        client_socket = %client_socket_path().display(),
        "herdr server built for embedding"
    );

    Ok(EmbeddedServer { server })
}

impl EmbeddedServer {
    /// Run the event loop until shutdown.
    pub async fn run(mut self) -> io::Result<()> {
        self.server.app.run_plugin_startup_hooks();
        self.server.run().await
    }
}
