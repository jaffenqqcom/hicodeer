//! Which client instances are alive, and the process trees they started.
//!
//! A client leaves no notice when it goes away: its process is killed outright,
//! or it starts again under a new identity while its earlier children keep
//! running. What it starts is long-lived by design -- language servers, agents,
//! interactive shells -- so without a record those outlive the client that
//! needed them and another set is started the next time it runs.
//!
//! What ends an instance's ownership of what it started is the loss of its
//! management connection, and nothing else. A client holds that connection open
//! for as long as it runs and polls the management listener over it, so the
//! connection is a statement of presence: while it is up the instance is there
//! and its trees are left alone, and when it goes the trees go with it. No
//! timer is involved -- an instance that is merely idle, or one frozen by the
//! system while the device sleeps, keeps its connection and keeps its trees.
//!
//! That equivalence -- a dropped connection means a process that is gone --
//! rests on this being a loopback connection that neither side closes while the
//! instance is alive: the only way left for it to end is the client's file
//! descriptors being reclaimed by the kernel when its process goes away. Should
//! the daemon and its clients ever be split across machines, a connection could
//! then drop for reasons that say nothing about the process, and a grace period
//! would have to be given before retiring anything.
//!
//! The management connection must therefore stay silent and long-lived: no
//! keepalive, and a rekey interval long enough that a frozen client is never
//! asked to answer (see `REKEY_BYTE_LIMIT` / `REKEY_TIME_LIMIT` in `main`).
//! The pooled command connections may carry a keepalive and rekey sooner,
//! because losing one says nothing about the instance and retires nothing.
//!
//! Identity rides on the SSH user name (see `protocol::CLIENT_ID_PREFIX`):
//! every connection a client opens -- the pooled command connections and the
//! management connection alike -- already carries one, so no payload format had
//! to change and no extra request had to be invented.
//!
//! When the daemon itself exits, nothing may outlive it: [`retire_all`] takes
//! down every recorded tree on the way out.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

/// Grace between the polite signal and the one that cannot be ignored.
const TERM_GRACE: Duration = Duration::from_secs(1);
/// Lowest process group id worth signalling: 0 addresses the caller's own
/// group and 1 belongs to init, so both reach far beyond an instance's tree.
const MIN_GROUP: i32 = 2;
/// The procfs mount the session scan reads process state from.
const PROC: &str = "/proc";

/// One client instance: the process groups it started that have not exited
/// yet, the sessions it opened, and the management connections currently open
/// on its behalf. The instance is alive for exactly as long as the latter is
/// non-empty.
struct Peer {
    groups: BTreeSet<i32>,
    sessions: BTreeSet<i32>,
    management_conns: BTreeSet<u64>,
}

/// Live instances, keyed by the identity their connections authenticated as.
static PEERS: LazyLock<Mutex<HashMap<String, Peer>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Hands out the token that identifies one management connection. Several may
/// be open for the same instance while a client reconnects, and telling them
/// apart is what keeps one connection closing from retiring an instance that
/// another is still vouching for.
static NEXT_MANAGEMENT_CONN: AtomicU64 = AtomicU64::new(1);

/// Borrows the instance table, ignoring a poisoned lock (the table is plain
/// data that stays consistent, so a panic elsewhere must not disable it).
fn peers() -> std::sync::MutexGuard<'static, HashMap<String, Peer>> {
    PEERS.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Whether a user name identifies an instance worth tracking. An older client
/// authenticates as a bare account name, which names no instance: such a
/// connection is left behaving exactly as it did before this record existed.
fn is_tracked(client_id: &str) -> bool {
    client_id.starts_with(crate::protocol::CLIENT_ID_PREFIX)
}

/// Allocates the token a management connection is known by.
pub(crate) fn new_management_token() -> u64 {
    NEXT_MANAGEMENT_CONN.fetch_add(1, Ordering::Relaxed)
}

/// Records that the instance behind `client_id` exists.
///
/// Called for every connection an instance opens, the pooled command
/// connections included, so a tree started before the management connection is
/// established is still recorded under the instance. Nothing beyond that join
/// happens: an instance already on record is left exactly as it is, and the
/// others are not touched at all.
pub(crate) fn touch(client_id: &str) {
    if !is_tracked(client_id) {
        return;
    }
    let fresh = {
        let mut table = peers();
        if table.contains_key(client_id) {
            false
        } else {
            table.insert(
                client_id.to_string(),
                Peer {
                    groups: BTreeSet::new(),
                    sessions: BTreeSet::new(),
                    management_conns: BTreeSet::new(),
                },
            );
            true
        }
    };
    if fresh {
        log::info!("conn: client {client_id} connected");
    }
}

/// Records a management connection as open on behalf of `client_id`. The
/// instance stays alive until every such connection has been closed.
pub(crate) fn management_opened(client_id: &str, token: u64) {
    if !is_tracked(client_id) {
        return;
    }
    let mut table = peers();
    if let Some(peer) = table.get_mut(client_id) {
        peer.management_conns.insert(token);
        return;
    }
    let mut peer = Peer {
        groups: BTreeSet::new(),
        sessions: BTreeSet::new(),
        management_conns: BTreeSet::new(),
    };
    peer.management_conns.insert(token);
    table.insert(client_id.to_string(), peer);
}

/// Records a management connection as closed. The instance is over -- and
/// everything it started goes with it -- once none are left.
pub(crate) fn management_closed(client_id: &str, token: u64) {
    let over = {
        let mut table = peers();
        let Some(peer) = table.get_mut(client_id) else {
            return;
        };
        if !peer.management_conns.remove(&token) {
            return;
        }
        peer.management_conns.is_empty()
    };
    if over {
        retire(client_id, "management connection closed");
    }
}

/// Records a process group as belonging to an instance.
pub(crate) fn add_group(client_id: &str, pgid: i32) {
    if !is_tracked(client_id) || pgid < MIN_GROUP {
        return;
    }
    let mut table = peers();
    let Some(peer) = table.get_mut(client_id) else {
        return;
    };
    peer.groups.insert(pgid);
}

/// Forgets a process group that exited on its own.
pub(crate) fn drop_group(client_id: &str, pgid: i32) {
    let mut table = peers();
    let Some(peer) = table.get_mut(client_id) else {
        return;
    };
    peer.groups.remove(&pgid);
}

/// Records a session as belonging to an instance.
///
/// An interactive shell is started as its own session leader (`setsid`), and
/// the shell's jobs -- each in its own process group under job control -- stay
/// in that session. Signalling only the shell's group would leave those jobs
/// behind, so the session is recorded too: [`session_pgids`] finds every group
/// its tree spread across.
pub(crate) fn add_session(client_id: &str, session_id: i32) {
    if !is_tracked(client_id) || session_id < MIN_GROUP {
        return;
    }
    let mut table = peers();
    let Some(peer) = table.get_mut(client_id) else {
        return;
    };
    peer.sessions.insert(session_id);
}

/// Forgets a session that ended on its own.
pub(crate) fn drop_session(client_id: &str, session_id: i32) {
    let mut table = peers();
    let Some(peer) = table.get_mut(client_id) else {
        return;
    };
    peer.sessions.remove(&session_id);
}

/// Takes down everything an instance started and forgets it.
fn retire(client_id: &str, reason: &str) {
    let (groups, sessions) = {
        let mut table = peers();
        match table.remove(client_id) {
            Some(peer) => (peer.groups, peer.sessions),
            None => return,
        }
    };
    let group_count = groups.len();
    let session_count = sessions.len();
    signal_tree(&groups, &sessions);
    log::warn!(
        "conn: client {client_id} {reason}; took down {group_count} group(s) across {session_count} \
         session(s)"
    );
}

/// Takes down everything every instance started and clears the record.
///
/// Runs on the daemon's own way out, where there is no runtime to hand an
/// escalation to and no client left to serve: the trees are signalled in one
/// pass, given the same grace [`retire`] would give them, and then insisted
/// upon, so no child can outlive the daemon that owns it.
pub(crate) fn retire_all(reason: &str) {
    let drained: Vec<(String, BTreeSet<i32>, BTreeSet<i32>)> = {
        let mut table = peers();
        table
            .drain()
            .map(|(client_id, peer)| (client_id, peer.groups, peer.sessions))
            .collect()
    };
    if drained.is_empty() {
        return;
    }
    let instances = drained.len();
    let mut groups: BTreeSet<i32> = BTreeSet::new();
    let mut sessions: BTreeSet<i32> = BTreeSet::new();
    for (client_id, peer_groups, peer_sessions) in drained {
        log::warn!("conn: client {client_id} {reason}");
        groups.extend(peer_groups);
        sessions.extend(peer_sessions);
    }
    // One scan covers every session at once. This runs on the daemon's own way
    // out, where there is no runtime to hand an escalation to and no client
    // left to serve.
    groups.extend(session_pgids(&sessions));
    log::warn!(
        "conn: {reason}; taking down {} group(s) from {instances} client(s)",
        groups.len()
    );
    signal_now(&groups, libc::SIGTERM);
    std::thread::sleep(TERM_GRACE);
    signal_now(&groups, libc::SIGKILL);
}

/// Every process group an instance's tree is spread across: the groups recorded
/// directly, plus the groups of every process found in its sessions.
fn tree_groups(groups: &BTreeSet<i32>, sessions: &BTreeSet<i32>) -> BTreeSet<i32> {
    let mut targets = groups.clone();
    targets.extend(session_pgids(sessions));
    targets
}

/// Signals every group in an instance's tree politely, then again without
/// appeal once the grace has passed. The escalation runs off the caller, so the
/// connection teardown that noticed the instance is gone is never held up by
/// it. The tree is resolved once, up front: re-deriving it after the grace
/// would risk matching a session that reused a freed id in the meantime, and
/// the jobs that matter are the ones running now.
fn signal_tree(groups: &BTreeSet<i32>, sessions: &BTreeSet<i32>) {
    let tree = tree_groups(groups, sessions);
    signal_now(&tree, libc::SIGTERM);
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // Nothing to schedule the follow-up on, so a tree that ignores the
        // polite signal would keep running: insist immediately instead.
        signal_now(&tree, libc::SIGKILL);
        return;
    };
    handle.spawn(async move {
        tokio::time::sleep(TERM_GRACE).await;
        signal_now(&tree, libc::SIGKILL);
    });
}

/// Takes down one session's whole tree: its leader and every process that
/// carries its session id, across the process groups job control created.
pub(crate) fn kill_session(session_id: i32) {
    if session_id < MIN_GROUP {
        return;
    }
    let mut sessions = BTreeSet::new();
    sessions.insert(session_id);
    signal_tree(&BTreeSet::new(), &sessions);
}

/// The process groups of every live process whose session is in `targets`.
///
/// A session leader (an interactive shell, started with `setsid`) is reached by
/// its recorded group, but its jobs -- each in its own group under job control
/// -- are not. This finds them by their session, the leader's pid, which stays
/// with them even after the leader exits. Only `targets` are matched, so no
/// other client's trees are touched.
fn session_pgids(targets: &BTreeSet<i32>) -> BTreeSet<i32> {
    let mut groups = BTreeSet::new();
    if targets.is_empty() {
        return groups;
    }
    let own_pid = std::process::id() as i32;
    let entries = match std::fs::read_dir(PROC) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!("conn: cannot read {PROC}, session trees will not be found: {err}");
            return groups;
        }
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if pid <= 1 || pid == own_pid {
            continue;
        }
        let Some((pgrp, session)) = process_groups(pid) else {
            continue;
        };
        if pgrp >= MIN_GROUP && targets.contains(&session) {
            groups.insert(pgrp);
        }
    }
    groups
}

/// The process group and session a live process reports, from its `/proc`
/// entry. A process that exits between the directory listing and this read, or
/// one this daemon may not inspect, reports nothing and is skipped.
fn process_groups(pid: i32) -> Option<(i32, i32)> {
    let stat = std::fs::read_to_string(format!("{PROC}/{pid}/stat")).ok()?;
    parse_stat_groups(&stat)
}

/// The process group and session carried by a `/proc/<pid>/stat` line.
///
/// Field 2 (`comm`) is the program name in parentheses and may itself contain
/// spaces and parentheses, so the fields that follow are only reliably located
/// after the last `)`: state, ppid, pgrp, session, ...
fn parse_stat_groups(stat: &str) -> Option<(i32, i32)> {
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    let pgrp = fields.next()?.parse().ok()?;
    let session = fields.next()?.parse().ok()?;
    Some((pgrp, session))
}

/// Sends one signal to each group. A negative pid addresses the whole group,
/// which is what reaches the descendants that never appear in any table.
fn signal_now(groups: &BTreeSet<i32>, signal: i32) {
    // Never signal the daemon's own group: it is part of no client's tree, and
    // a scan that somehow reported it would take the daemon down with it.
    let own_group = unsafe { libc::getpgrp() };
    for &pgid in groups {
        if pgid < MIN_GROUP || pgid == own_group {
            continue;
        }
        // SAFETY: a negative pid addresses the process group; the group was
        // created by this daemon (see `exec` and `pty`) and is still on record.
        unsafe { libc::kill(-pgid, signal) };
    }
}

#[cfg(test)]
mod tests {
    use super::parse_stat_groups;

    /// The fields after a plain `comm`.
    #[test]
    fn parses_plain_stat() {
        assert_eq!(parse_stat_groups("1 (init) S 0 1 1 0 0 0"), Some((1, 1)));
    }

    /// `comm` may contain spaces: they must not shift the fields.
    #[test]
    fn parses_comm_with_space() {
        assert_eq!(parse_stat_groups("42 (a b) S 1 42 42 0 0"), Some((42, 42)));
    }

    /// `comm` may contain parentheses, including a trailing one: only the last
    /// `)` ends the name.
    #[test]
    fn parses_comm_with_parens() {
        assert_eq!(parse_stat_groups("7 (weird)name) R 1 7 7 0 0"), Some((7, 7)));
    }

    /// An empty `comm` still parses.
    #[test]
    fn parses_empty_comm() {
        assert_eq!(parse_stat_groups("8 () S 1 8 8 0 0"), Some((8, 8)));
    }

    /// A line missing the session field reports nothing.
    #[test]
    fn rejects_truncated_stat() {
        assert_eq!(parse_stat_groups("9 (x) S 1 9"), None);
    }
}
