//! Build script: embed the fixed management keys into the bridge.
//!
//! cmd-client, hicodeerd and hishell share one management key pair kept in a
//! single directory (`HICODEERD_KEYS_DIR`, or
//! `<repository root>/target/hicodeerd-keys` when that is unset). Each crate
//! copies the files it needs into its own `OUT_DIR` and embeds them with
//! `include_str!`, so no key file is read at runtime.
//!
//! When the pair is missing, a build script generates it under an exclusive
//! file lock and the others re-check after acquiring the lock, so concurrent
//! builds never end up with competing pairs.

#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

/// (source file in the shared keys directory, file name in OUT_DIR).
const KEYS: &[(&str, &str)] = &[
    ("mgmt_host_key.pub", "mgmt_host_key.pub"),
    ("mgmt_client_key", "mgmt_client_key"),
];

/// Every file one generated pair consists of. Any that is missing triggers a
/// regeneration of the whole pair (`ssh-keygen` always writes both halves).
const PAIR_FILES: &[&str] = &[
    "mgmt_host_key",
    "mgmt_host_key.pub",
    "mgmt_client_key",
    "mgmt_client_key.pub",
];

fn main() {
    let keys_dir = keys_dir();
    ensure_keys(&keys_dir);
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    for (source, out_name) in KEYS {
        let from = keys_dir.join(source);
        let to = out_dir.join(out_name);
        std::fs::copy(&from, &to)
            .unwrap_or_else(|err| panic!("copy {} to {}: {err}", from.display(), to.display()));
        println!("cargo:rerun-if-changed={}", from.display());
    }
    println!("cargo:rerun-if-env-changed=HICODEERD_KEYS_DIR");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");
}

/// Resolves the shared keys directory.
fn keys_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HICODEERD_KEYS_DIR") {
        return PathBuf::from(dir);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    target.join("hicodeerd-keys")
}

/// Repository root, identified by the `crates/gpui_ohos` directory that every
/// crate under `depend/` sits beneath. Resolving the shared keys directory from
/// it is what keeps all three consumers on one key pair. Ascending to the
/// nearest `[workspace]` manifest instead would not: `hicodeerd` and `hishell`
/// are standalone crates whose own manifests declare `[workspace]`, so each
/// would stop at itself, and `cmd-client` carries the token `[workspace]` only
/// inside a comment.
fn repository_root() -> PathBuf {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let mut dir = manifest_dir.as_path();
    loop {
        if dir.join("crates").join("gpui_ohos").is_dir() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            // A crate copied out of the repository cannot name the shared keys
            // directory. Failing the build is deliberate: silently generating a
            // pair here would split this crate's keys from the other two.
            None => panic!(
                "no repository root above {}: no ancestor holds crates/gpui_ohos",
                manifest_dir.display()
            ),
        }
    }
}

/// Generates the management key pair when any file is missing, holding an
/// exclusive lock so concurrent builds do not each create their own pair.
fn ensure_keys(dir: &Path) {
    if pair_present(dir) {
        return;
    }
    std::fs::create_dir_all(dir)
        .unwrap_or_else(|err| panic!("create keys dir {}: {err}", dir.display()));
    let lock_path = dir.join(".keys.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        // The lock file exists only to hold the flock taken below; its contents
        // are never written or read, so an existing file must not be truncated.
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|err| panic!("open lock {}: {err}", lock_path.display()));
    // SAFETY: the fd is valid for the lifetime of `lock`; flock only reads it.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        panic!(
            "flock {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        );
    }
    // Re-check after acquiring the lock: whichever build got here first has,
    // by now, generated the pair.
    if pair_present(dir) {
        return;
    }
    for name in PAIR_FILES {
        let path = dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .unwrap_or_else(|err| panic!("remove stale {}: {err}", path.display()));
        }
    }
    ssh_keygen(&dir.join("mgmt_host_key"));
    ssh_keygen(&dir.join("mgmt_client_key"));
}

/// Whether every file of a generated pair is present.
fn pair_present(dir: &Path) -> bool {
    PAIR_FILES.iter().all(|name| dir.join(name).is_file())
}

/// Runs `ssh-keygen` to write one ed25519 key pair.
fn ssh_keygen(path: &Path) {
    let status = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-f"])
        .arg(path)
        .status()
        .unwrap_or_else(|err| panic!("run ssh-keygen: {err}"));
    if !status.success() {
        panic!("ssh-keygen failed for {}", path.display());
    }
}
