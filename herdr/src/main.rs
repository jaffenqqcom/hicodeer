//! Binary entry point. All logic lives in the `hicodeer_agent` library so that
//! hosts (for example the HiCodeer daemon) can embed the server and the TUI
//! client.

fn main() -> std::io::Result<()> {
    hicodeer_agent::run_cli()
}
