//! Binary entry point. All logic lives in the `herdr` library so that hosts
//! (for example the HiCodeer daemon) can embed the server and the TUI client.

fn main() -> std::io::Result<()> {
    herdr::run_cli()
}
