//! Fixed management keys compiled into the bridge.
//!
//! The bootstrap authenticates against the daemon's management listener, so
//! this side needs the host public key it must verify and the private key it
//! authenticates with. Both are embedded with `include_str!` out of `build.rs`'s
//! `OUT_DIR`: the bridge carries its credentials inside the binary and reads no
//! key directory at runtime.
//!
//! The matching halves live in the hicodeerd crate, which embeds the host key's
//! private part and this client key's public part. All three consumers copy
//! from the same shared key directory (see `build.rs`), so they always match.

/// Management host public key (ed25519, one OpenSSH line).
pub const MGMT_HOST_PUB: &str = include_str!(concat!(env!("OUT_DIR"), "/mgmt_host_key.pub"));
/// Management client private key (ed25519, OpenSSH format).
pub const MGMT_CLIENT_KEY: &str = include_str!(concat!(env!("OUT_DIR"), "/mgmt_client_key"));
