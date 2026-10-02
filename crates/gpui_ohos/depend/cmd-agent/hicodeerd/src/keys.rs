//! Management keys for the daemon.
//!
//! The daemon serves the management listener with a fixed host key and accepts
//! a fixed client public key. The host private key is read at runtime from the
//! package's `conf` directory (mode 0600) and never embedded: the daemon is a
//! public HNP whose binary is world-readable, so a compiled-in secret would be
//! readable by anyone. The authorized client public key is not a secret and
//! stays embedded, staged by `build.rs` out of the shared keys directory. The
//! matching client halves are embedded in cmd-client and hishell from that same
//! directory (see `build.rs`).

use std::path::Path;

use russh::keys::ssh_key::{PrivateKey, PublicKey};

/// Name of the fixed management host private key file inside `conf`.
const MGMT_HOST_KEY_FILE: &str = "mgmt_host_key";
/// Authorized management client public key (ed25519, one OpenSSH line),
/// embedded at build time.
pub const AUTHORIZED_KEYS: &str = include_str!(concat!(env!("OUT_DIR"), "/authorized_keys"));

/// Loads the management host private key from `conf` and the embedded
/// authorized client public key. The authorized key is the first non-comment
/// line, with a trailing comment (as `ssh-keygen` appends) stripped so it never
/// leaks into the parsed value.
pub fn mgmt_keys(conf: &Path) -> Result<(PrivateKey, PublicKey), String> {
    let host_path = conf.join(MGMT_HOST_KEY_FILE);
    let host_pem = std::fs::read_to_string(&host_path)
        .map_err(|err| format!("read {}: {err}", host_path.display()))?;
    let host_key = PrivateKey::from_openssh(&host_pem)
        .map_err(|err| format!("parse {}: {err}", host_path.display()))?;
    let pub_line = AUTHORIZED_KEYS
        .lines()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .ok_or_else(|| "embedded authorized_keys is empty".to_string())?;
    let canonical: String = pub_line
        .trim()
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let authorized = PublicKey::from_openssh(&canonical)
        .map_err(|err| format!("parse embedded authorized key: {err}"))?;
    Ok((host_key, authorized))
}
