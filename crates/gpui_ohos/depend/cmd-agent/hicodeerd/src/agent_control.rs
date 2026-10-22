//! Loopback bridge from the host application to the embedded agent JSON API.
//!
//! The agent server's API socket lives inside this daemon's tree (mode 0600,
//! owner-only), and the host application runs under a different account, so it
//! cannot open that socket. This module publishes a TCP listener on loopback
//! instead and bridges every accepted connection onto the API socket byte for
//! byte: no protocol translation, so the host speaks the server's own JSON API.
//!
//! The listener is guarded by a token generated for this daemon run. The host
//! learns it from the management bootstrap payload (`SshInfo.agent`), and a
//! connection that does not present it as its first line is closed unread, so
//! nothing on loopback can reach the API without having gone through the
//! authenticated management listener first.
//!
//! The client protocol socket needs the same treatment for a different reason:
//! the TUI client may run under the application account, whose filesystem view
//! of this daemon's state directory is a different tree, so the socket is
//! simply absent there (`ENOENT`). The client therefore publishes a local
//! socket of its own and relays to the loopback bridge. That bridge carries no
//! token -- the socket it forwards to is already reachable by any local
//! process, so guarding the loopback end would add no protection.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rand_core::{OsRng, RngCore};

use crate::protocol::{
    AGENT_CONTROL_PORT, AGENT_CONTROL_TOKEN_PREFIX, AgentControl, LOOPBACK_ADDR,
};

/// Entropy in one run's control token, hex-encoded on the wire.
const TOKEN_BYTES: usize = 32;
/// Upper bound on the token line. The real line is fixed-length; this only keeps
/// a misbehaving peer from making us read forever.
const TOKEN_LINE_MAX: usize = 128;
/// How long a fresh connection may take to present its token line.
const TOKEN_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Loopback port carrying the client protocol socket, for the TUI client.
pub(crate) const AGENT_CLIENT_PORT: u16 = 40250;

/// Binds the loopback end of the client protocol socket. Every accepted
/// connection is bridged onto `client_socket_path` byte for byte, so the TUI
/// client speaks the server's own protocol without sharing this daemon's
/// view.
pub(crate) fn start_client_bridge(client_socket_path: PathBuf) -> io::Result<()> {
    let listener = TcpListener::bind((LOOPBACK_ADDR, AGENT_CLIENT_PORT))?;
    std::thread::Builder::new()
        .name("agent-client-bridge".into())
        .spawn(move || accept_loop_unchecked(listener, client_socket_path))?;
    Ok(())
}

/// Accepts forever, handing each connection to its own thread. Unlike
/// [`accept_loop`] there is no token to check first.
fn accept_loop_unchecked(listener: TcpListener, socket_path: PathBuf) {
    for accepted in listener.incoming() {
        let stream = match accepted {
            Ok(stream) => stream,
            Err(err) => {
                log::warn!("agent client bridge: accept failed: {err}");
                continue;
            }
        };
        let socket_path = socket_path.clone();
        let spawned = std::thread::Builder::new()
            .name("agent-client-bridge-conn".into())
            .spawn(move || serve_unchecked(stream, &socket_path));
        if let Err(err) = spawned {
            log::warn!("agent client bridge: could not spawn connection thread: {err}");
        }
    }
}

/// Bridges one accepted connection onto the client socket.
fn serve_unchecked(stream: TcpStream, socket_path: &Path) {
    let client = match UnixStream::connect(socket_path) {
        Ok(client) => client,
        Err(err) => {
            log::warn!(
                "agent client bridge: cannot connect {}: {err}",
                socket_path.display()
            );
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    bridge(stream, client);
}

/// Binds `local_path` and relays every accepted connection to the daemon's
/// loopback client bridge.
///
/// The caller supplies a path unique to its own client process. That matters in
/// both possible views: in the application's sandbox the daemon's client socket
/// is absent, and beside the daemon a shared path would replace the socket the
/// daemon's own bridge forwards to. The bridge reaches the real socket either
/// way.
pub(crate) fn serve_local_client_proxy(local_path: &Path, upstream_port: u16) -> io::Result<()> {
    if let Some(parent) = local_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Always start from a clean path. Reusing a socket that still answers is
    // the wrong call here: the only process that can be listening is an earlier
    // TUI client, and if that one has wedged -- which is exactly what happens
    // when its own upstream connection stalls -- the new client would attach to
    // a proxy that relays nowhere and fail too. This process is the fixed Agent
    // terminal's client, so there is never a second live client to protect.
    let _ = std::fs::remove_file(local_path);
    let listener = UnixListener::bind(local_path)?;
    let upstream = format!("{LOOPBACK_ADDR}:{upstream_port}");
    std::thread::Builder::new()
        .name("agent-client-proxy".into())
        .spawn(move || {
            for accepted in listener.incoming() {
                let client = match accepted {
                    Ok(client) => client,
                    Err(err) => {
                        log::warn!("agent client proxy: accept failed: {err}");
                        continue;
                    }
                };
                let upstream = upstream.clone();
                let spawned = std::thread::Builder::new()
                    .name("agent-client-proxy-conn".into())
                    .spawn(move || {
                        let stream = match TcpStream::connect(&upstream) {
                            Ok(stream) => stream,
                            Err(err) => {
                                log::warn!("agent client proxy: cannot reach the daemon: {err}");
                                return;
                            }
                        };
                        let _ = stream.set_nodelay(true);
                        bridge(stream, client);
                    });
                if let Err(err) = spawned {
                    log::warn!("agent client proxy: could not spawn connection thread: {err}");
                }
            }
        })?;
    Ok(())
}

/// Binds the bridge for this run and returns the information the host needs to
/// reach it. The listener thread runs for the life of the process.
pub(crate) fn start(socket_path: PathBuf) -> io::Result<AgentControl> {
    let token = new_token();
    let listener = TcpListener::bind((LOOPBACK_ADDR, AGENT_CONTROL_PORT))?;
    let listen_token = token.clone();
    let listen_path = socket_path.clone();
    std::thread::Builder::new()
        .name("agent-control".into())
        .spawn(move || accept_loop(listener, listen_path, listen_token))?;

    Ok(AgentControl {
        socket_path: socket_path.to_string_lossy().into_owned(),
        control_port: AGENT_CONTROL_PORT,
        token,
        // Filled in by the caller, which knows where the package's config lives.
        config_path: None,
    })
}

/// Hex token for this run. `OsRng` is the same source the run's SSH keys use.
fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let mut token = String::with_capacity(TOKEN_BYTES * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

fn accept_loop(listener: TcpListener, socket_path: PathBuf, token: String) {
    for accepted in listener.incoming() {
        let stream = match accepted {
            Ok(stream) => stream,
            Err(err) => {
                log::warn!("agent control: accept failed: {err}");
                continue;
            }
        };
        let socket_path = socket_path.clone();
        let token = token.clone();
        let spawned = std::thread::Builder::new()
            .name("agent-control-conn".into())
            .spawn(move || {
                if let Err(err) = serve(stream, &socket_path, &token) {
                    log::warn!("agent control: connection ended: {err}");
                }
            });
        if let Err(err) = spawned {
            log::warn!("agent control: could not spawn connection thread: {err}");
        }
    }
}

/// Authenticates one connection and bridges it onto the API socket.
fn serve(mut stream: TcpStream, socket_path: &Path, token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(TOKEN_READ_TIMEOUT))?;
    let line = read_token_line(&mut stream)?;
    stream.set_read_timeout(None)?;

    if !token_matches(&line, token) {
        log::warn!("agent control: connection without this run's token");
        return Ok(());
    }

    let api = UnixStream::connect(socket_path)?;
    // The API is request/response over small frames; waiting on Nagle would only
    // add latency to every one of them.
    let _ = stream.set_nodelay(true);
    bridge(stream, api);
    Ok(())
}

/// Reads one newline-terminated line without buffering past it: any byte past
/// the newline belongs to the JSON stream and has to stay in the socket.
fn read_token_line(stream: &mut TcpStream) -> io::Result<String> {
    let mut line = Vec::with_capacity(TOKEN_LINE_MAX);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte)? {
            0 => break,
            _ if byte[0] == b'\n' => break,
            _ => {
                if line.len() >= TOKEN_LINE_MAX {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "control token line too long",
                    ));
                }
                line.push(byte[0]);
            }
        }
    }
    Ok(String::from_utf8_lossy(&line).trim_end_matches('\r').to_string())
}

/// Compares the presented line with this run's token. The line is
/// `<token prefix> <token>`; anything else is a mismatch.
fn token_matches(line: &str, token: &str) -> bool {
    let Some(rest) = line.strip_prefix(AGENT_CONTROL_TOKEN_PREFIX) else {
        return false;
    };
    let Some(presented) = rest.strip_prefix(' ') else {
        return false;
    };
    !token.is_empty() && presented == token
}

/// Copies both directions until each side closes. Neither direction is joined:
/// a peer that holds its end open simply keeps its thread parked in `read`,
/// which costs nothing and ends when the peer does.
fn bridge(tcp: TcpStream, api: UnixStream) {
    let upload_tcp = match tcp.try_clone() {
        Ok(stream) => stream,
        Err(err) => {
            log::warn!("agent control: could not split the connection: {err}");
            return;
        }
    };
    let upload_api = match api.try_clone() {
        Ok(stream) => stream,
        Err(err) => {
            log::warn!("agent control: could not split the api socket: {err}");
            return;
        }
    };

    let spawned = std::thread::Builder::new()
        .name("agent-control-up".into())
        .spawn(move || {
            let mut source = upload_tcp;
            let mut sink = upload_api;
            copy_direction(&mut source, &mut sink, "host->server");
            let _ = sink.shutdown(Shutdown::Write);
        });
    if let Err(err) = spawned {
        log::warn!("agent control: could not spawn the upload thread: {err}");
        return;
    }

    let mut source = api;
    let mut sink = tcp;
    copy_direction(&mut source, &mut sink, "server->host");
    let _ = sink.shutdown(Shutdown::Write);
}

/// Copies one direction, logging anything other than a peer hanging up.
fn copy_direction(source: &mut impl Read, sink: &mut impl Write, direction: &str) {
    match io::copy(source, sink) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
        Err(err) if err.kind() == io::ErrorKind::ConnectionReset => {}
        Err(err) => log::warn!("agent control: {direction} copy failed: {err}"),
    }
    let _ = sink.flush();
}
