//! Resolver hook for OHOS.
//!
//! On HarmonyOS the tools commands actually run (language servers, node, git)
//! live in the hicodeerd environment, which the application sandbox cannot see:
//! a scan of the application's own PATH therefore misses all of them. A
//! resolver registered at startup answers such a lookup. The local scan stays
//! authoritative - the resolver is only consulted when it finds nothing - so
//! nothing that already resolves locally changes behavior.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Answers a lookup the local scan could not satisfy. `None` means the
/// environment behind the resolver holds no entry for the name, which is read
/// as "not present there".
pub type Resolver = dyn Fn(&OsStr) -> Option<Vec<PathBuf>> + Send + Sync + 'static;

static RESOLVER: OnceLock<Box<Resolver>> = OnceLock::new();

thread_local! {
    /// Number of lookups this thread is currently inside. The resolver's own
    /// implementation may resolve programs itself - it hands the query to the
    /// environment the commands run in - and such a nested lookup must not
    /// reach the resolver again, or it would recurse without bound. A nested
    /// lookup therefore keeps its local result and stops there.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Restores the depth counter when the lookup it guards ends, including on
/// unwind.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> Self {
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Installs the resolver. Only the first call takes effect, so the resolver
/// registered at startup stays authoritative for the process lifetime.
pub fn set_resolver(resolver: Box<Resolver>) -> Result<(), ()> {
    RESOLVER.set(resolver).map_err(|_| ())
}

/// Returns the resolver's answer for `name`, or `None` when no resolver is
/// installed or this thread is already inside one.
pub(crate) fn intercept(name: &OsStr) -> Option<Vec<PathBuf>> {
    let resolver = RESOLVER.get()?;
    if DEPTH.with(Cell::get) > 0 {
        return None;
    }
    let _guard = DepthGuard::enter();
    resolver(name)
}
