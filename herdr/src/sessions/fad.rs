//! History collection through FAD's own connector registry.
//!
//! FAD ships a connector per agent and owns each store's layout, so the list
//! here is written out by hand ([`fad_stores`]): naming the supported ones is
//! what keeps every other connector out of the binary, and it also stops a FAD
//! upgrade from silently scanning a store this device cannot have a CLI for.
//! [`connector_registry`] then adds the stores FAD does not cover (CodeBuddy,
//! DeepSeek Harness, DevEco Code, ZCode, Codewhale, AtomCode, MiniMax Code,
//! OpenDesk), each with its own removal function. [`agent_label`] maps FAD's
//! slug to the label `agent.list` reports, because the panel matches a history
//! row against a running pane by that pair — a mismatched label leaves the row
//! visible while the session is already open.
//!
//! Scanning is not free: a connector re-reads its whole store on every call.
//! Each one therefore keeps its last answer next to the signature of the files
//! it would read (`discover_source_files`): one stat per file, and the
//! connector only runs again when that signature moved.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use franken_agent_detection::connectors::{
    claude_code::ClaudeCodeConnector, codex::CodexConnector, gemini::GeminiConnector,
    hermes::HermesConnector, kimi::KimiConnector, openclaw::OpenClawConnector,
    opencode::OpenCodeConnector, pi_agent::PiAgentConnector, pi_durable::PiDurableConnector,
    qwen::QwenConnector,
};
use franken_agent_detection::{
    Connector, DetectionResult, DiscoveredSourceFile, NormalizedConversation, ScanContext,
};

use super::CollectedSession;
use super::atomcode;
use super::claude_store;
use super::codebuddy;
use super::codewhale;
use super::codex_store;
use super::deepseek;
use super::deveco;
use super::gemini_store;
use super::kimi_store;
use super::minimax;
use super::opendesk;

/// FAD connector slug → the agent label the daemon reports for a pane of that
/// agent. Slugs without an entry are their own label.
const AGENT_LABELS: &[(&str, &str)] = &[("pi_agent", "pi"), ("pi_durable", "pi")];

/// One connector's last answer, valid while its store signature holds.
struct CachedScan {
    signature: u64,
    sessions: Vec<CollectedSession>,
}

static CACHE: Mutex<Option<HashMap<&'static str, CachedScan>>> = Mutex::new(None);

pub(crate) fn collect() -> Vec<CollectedSession> {
    // Take the previous answers so a connector that failed this time can keep
    // serving its last good list instead of emptying the section.
    let previous = CACHE
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .unwrap_or_default();
    let mut next: HashMap<&'static str, CachedScan> = HashMap::new();
    let mut collected = Vec::new();

    for (slug, store) in connector_registry() {
        // FAD's data dir backs its own bookkeeping, so each connector gets its
        // own scratch directory rather than sharing one across all of them.
        let ctx = ScanContext::local_default(
            std::env::temp_dir().join("herdr-fad-scan").join(slug),
            None,
        );
        let deletable = store.delete.is_some();
        let signature = source_signature(store.connector.as_ref(), &ctx);
        if let Some(cached) = previous.get(slug).filter(|cached| cached.signature == signature) {
            collected.extend(cached.sessions.iter().cloned());
            next.insert(slug, CachedScan {
                signature,
                sessions: cached.sessions.clone(),
            });
            continue;
        }

        match store.connector.scan(&ctx) {
            Ok(conversations) => {
                let sessions = normalize(agent_label(slug), deletable, conversations);
                collected.extend(sessions.iter().cloned());
                next.insert(slug, CachedScan { signature, sessions });
            }
            Err(error) => {
                tracing::warn!("agent sessions: {slug} store scan failed: {error}");
                if let Some(cached) = previous.get(slug) {
                    collected.extend(cached.sessions.iter().cloned());
                    next.insert(slug, CachedScan {
                        signature: cached.signature,
                        sessions: cached.sessions.clone(),
                    });
                }
            }
        }
    }

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(next);
    }
    collected
}

/// One agent's session store: the reader for it and, when the store can remove
/// entries, the function that does so.
///
/// Both halves are the same knowledge — where this agent keeps its sessions and
/// what one of them is made of — so they are registered together. The shared
/// collection path never learns a store's layout, and a store that cannot
/// remove entries simply registers none.
struct RegisteredStore {
    connector: Box<dyn Connector + Send>,
    delete: Option<fn(&Path) -> Result<(), String>>,
    /// Who checks that the path a request names really belongs to this store.
    /// A FAD connector knows its roots but not which file a row came from, so
    /// the shared side has to check the roots; a store written here does the
    /// whole check in its own `delete` and must not be second-guessed.
    guard: Guard,
}

enum Guard {
    /// The path has to lie under the roots the connector reports detecting.
    DetectedRoots,
    /// The store checks its own layout inside `delete`.
    StoreOwned,
}

/// FAD connectors whose `source_path` is the transcript of exactly one session,
/// so unlinking it removes that session and nothing else.
///
/// This is a list rather than a property of the connector because FAD exposes
/// no such capability, and guessing from the path would make an unknown store
/// deletable by accident. A connector is only listed when one file really is
/// one session:
///
/// - `pi_durable` is deliberately absent even though it writes JSONL: every
///   conversation in a durable store reports the *same* `source_path` (the
///   store's `main.jsonl`), so a generic unlink would take the whole store with
///   it. Deleting one of its rows needs the store's own rewrite.
/// - `opencode` and `openclaw` keep their sessions in databases, but each ships
///   a delete command of its own (`opencode session delete`,
///   `openclaw sessions delete`). Wiring those up means running the agent's CLI
///   instead of unlinking a path, which this table cannot express, so they stay
///   out until that path exists.
/// - `hermes` has no delete command at all, and its database refuses to drop a
///   session that has gateway history (a `BEFORE DELETE` trigger), so there is
///   nothing to wire up.
const DELETABLE_FAD_SLUGS: &[&str] = &["claude", "codex", "gemini", "kimi", "pi_agent", "qwen"];

/// File extensions a transcript can have. Most stores write `.jsonl`; older
/// Gemini builds write one `.json` per session.
const TRANSCRIPT_EXTENSIONS: [&str; 2] = ["jsonl", "json"];

/// The FAD connectors this daemon runs, constructed by name.
///
/// `get_connector_factories()` would be the obvious source, but it holds a
/// function pointer for *every* connector FAD ships, which keeps all of their
/// code alive through the linker. Naming the supported ones here is what lets
/// the rest be dropped by `--gc-sections`: no reference in the final link, no
/// code in the binary. It also means a FAD upgrade cannot silently start
/// scanning a store this device cannot have a CLI for — a new connector stays
/// out until it is added below.
///
/// Aider is the case that made this matter: FAD's connector falls back to the
/// process working directory when it finds no Aider root, and the daemon's
/// working directory is `/`, so one scan walked the whole root filesystem (14s
/// on this device) for a tool that is not installed.
fn fad_stores() -> Vec<(&'static str, Box<dyn Connector + Send>)> {
    vec![
        ("claude", Box::new(ClaudeCodeConnector::new())),
        ("codex", Box::new(CodexConnector::new())),
        ("gemini", Box::new(GeminiConnector::new())),
        ("hermes", Box::new(HermesConnector::new())),
        ("kimi", Box::new(KimiConnector::new())),
        ("openclaw", Box::new(OpenClawConnector::new())),
        ("opencode", Box::new(OpenCodeConnector::new())),
        ("pi_agent", Box::new(PiAgentConnector::new())),
        ("pi_durable", Box::new(PiDurableConnector::new())),
        ("qwen", Box::new(QwenConnector::new())),
    ]
}

/// Every connector this build has, keyed by the slug FAD uses for it.
fn connector_registry() -> Vec<(&'static str, RegisteredStore)> {
    let mut stores: Vec<(&'static str, RegisteredStore)> = fad_stores()
        .into_iter()
        .map(|(slug, connector)| {
            (
                slug,
                RegisteredStore {
                    connector: with_store_adjustments(slug, connector),
                    delete: fad_delete_for(slug),
                    guard: Guard::DetectedRoots,
                },
            )
        })
        .collect();
    // FAD has no CodeBuddy or DeepSeek Harness connector; ours implement the
    // same trait so they take part in the same scan, cache and normalization
    // path.
    stores.push((
        "codebuddy",
        RegisteredStore {
            connector: Box::new(codebuddy::CodeBuddyConnector::new()),
            delete: Some(codebuddy::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    stores.push((
        "deepseek",
        RegisteredStore {
            connector: Box::new(deepseek::DeepSeekConnector::new()),
            delete: Some(deepseek::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    // Codewhale and AtomCode are not in FAD's registry; each keeps its own
    // session store, so each needs its own reader.
    stores.push((
        "codewhale",
        RegisteredStore {
            connector: Box::new(codewhale::CodewhaleConnector::new()),
            delete: Some(codewhale::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    stores.push((
        "atomcode",
        RegisteredStore {
            connector: Box::new(atomcode::AtomcodeConnector::new()),
            delete: Some(atomcode::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    // DevEco Code and ZCode keep OpenCode-shaped SQLite stores at paths FAD's
    // own OpenCode connector does not look at; one connector serves both.
    stores.push((
        "deveco",
        RegisteredStore {
            connector: Box::new(deveco::OpencodeFamilyConnector::new(
                deveco::OpencodeFamily::Deveco,
            )),
            delete: Some(deveco::delete_deveco_session),
            guard: Guard::StoreOwned,
        },
    ));
    stores.push((
        "zcode",
        RegisteredStore {
            connector: Box::new(deveco::OpencodeFamilyConnector::new(
                deveco::OpencodeFamily::Zcode,
            )),
            delete: Some(deveco::delete_zcode_session),
            guard: Guard::StoreOwned,
        },
    ));
    // MiniMax Code keeps its conversations in its own runtime-state database,
    // which no FAD connector reads.
    stores.push((
        "minimax",
        RegisteredStore {
            connector: Box::new(minimax::MinimaxConnector::new()),
            delete: Some(minimax::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    // OpenDesk CLI keeps one JSON task per conversation plus an index; nothing
    // in FAD reads that store.
    stores.push((
        "opendesk",
        RegisteredStore {
            connector: Box::new(opendesk::OpendeskConnector::new()),
            delete: Some(opendesk::delete_session),
            guard: Guard::StoreOwned,
        },
    ));
    stores
}

fn fad_delete_for(slug: &'static str) -> Option<fn(&Path) -> Result<(), String>> {
    DELETABLE_FAD_SLUGS
        .contains(&slug)
        .then_some(delete_transcript as fn(&Path) -> Result<(), String>)
}

/// Checks that a path the API asked to delete lies in the store the connector
/// reports.
///
/// FAD knows where its store lives (`detect().root_paths`) but not which file
/// the row pointed at, so the root is the strongest check available here. A
/// connector that detected nothing has no roots and therefore nothing to
/// delete from.
fn ensure_inside_store(connector: &dyn Connector, path: &Path) -> Result<(), String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", path.display()))?;
    let roots: Vec<PathBuf> = connector
        .detect()
        .root_paths
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    if roots.is_empty() || !roots.iter().any(|root| resolved.starts_with(root)) {
        return Err(format!(
            "refusing to delete {}: outside the session store",
            path.display()
        ));
    }
    Ok(())
}

/// Removes one FAD transcript.
///
/// The check is narrow on purpose: this runs as the user and the path arrives
/// over the API, so it only unlinks a file that exists and is called like a
/// transcript.
fn delete_transcript(path: &Path) -> Result<(), String> {
    let is_transcript = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| TRANSCRIPT_EXTENSIONS.contains(&extension));
    if !is_transcript {
        return Err(format!(
            "refusing to delete {}: not a transcript",
            path.display()
        ));
    }
    if !path.exists() {
        return Err(format!("refusing to delete {}: no such file", path.display()));
    }
    std::fs::remove_file(path).map_err(|error| format!("delete {}: {error}", path.display()))
}

/// Routes a deletion to the store that produced the session.
///
/// A label can name more than one store: FAD registers Pi twice (`pi_agent` and
/// `pi_durable`) and Copilot has a CLI and an editor store, and a label's stores
/// do not always report the same roots. Each store that claims the label is
/// therefore given the path in turn, and the first one that recognises it does
/// the removal.
pub(super) fn delete_session(agent: &str, path: &Path) -> Result<(), String> {
    let registry = connector_registry();
    let mut refusal = format!("unknown agent {agent}");
    for (_, store) in registry
        .iter()
        .filter(|(slug, _)| agent_label(slug) == agent)
    {
        let Some(delete) = store.delete else {
            refusal = format!("{agent} sessions cannot be removed here");
            continue;
        };
        if matches!(store.guard, Guard::DetectedRoots) {
            if let Err(error) = ensure_inside_store(store.connector.as_ref(), path) {
                refusal = error;
                continue;
            }
        }
        return delete(path);
    }
    Err(refusal)
}

/// Wraps the FAD connectors whose store needs an extra step before its rows are
/// complete. Every other connector is handed over untouched.
///
/// The step itself belongs to the store that needs it (`kimi_store`,
/// `claude_store`); this table only says which slug has one, the same way the
/// registry says which store has a removal function.
fn with_store_adjustments(
    slug: &'static str,
    connector: Box<dyn Connector + Send>,
) -> Box<dyn Connector + Send> {
    let adjust: Option<fn(&mut Vec<NormalizedConversation>)> = match slug {
        "kimi" => Some(kimi_store::adjust),
        "claude" => Some(claude_store::adjust),
        "codex" => Some(codex_store::adjust),
        "gemini" => Some(gemini_store::adjust),
        _ => None,
    };
    match adjust {
        Some(adjust) => Box::new(AdjustedConnector {
            inner: connector,
            adjust,
        }),
        None => connector,
    }
}

fn agent_label(slug: &'static str) -> &'static str {
    AGENT_LABELS
        .iter()
        .find_map(|(known, label)| (*known == slug).then_some(*label))
        .unwrap_or(slug)
}

/// Hashes every file the connector would read. A connector with no store on
/// this device discovers nothing, signs as zero and is cheap to re-check.
fn source_signature(connector: &dyn Connector, ctx: &ScanContext) -> u64 {
    let Ok(files) = connector.discover_source_files(ctx) else {
        return 0;
    };
    if files.is_empty() {
        return 0;
    }
    let mut entries: Vec<(PathBuf, i64, u64)> = files
        .into_iter()
        .map(|file| {
            let metadata = std::fs::metadata(&file.source_path).ok();
            let modified = file
                .modified_at_ms
                .or_else(|| metadata.as_ref().and_then(modified_ms))
                .unwrap_or(0);
            let size = file
                .size_bytes
                .or_else(|| metadata.as_ref().map(std::fs::Metadata::len))
                .unwrap_or(0);
            (file.source_path, modified, size)
        })
        .collect();
    entries.sort();
    let mut hasher = DefaultHasher::new();
    entries.hash(&mut hasher);
    hasher.finish()
}

fn normalize(
    agent: &'static str,
    deletable: bool,
    conversations: Vec<NormalizedConversation>,
) -> Vec<CollectedSession> {
    conversations
        .into_iter()
        .map(|conversation| CollectedSession {
            agent,
            session_id: session_id(&conversation),
            title: conversation.title,
            workspace_root: conversation.workspace,
            updated_at: conversation
                .ended_at
                .or(conversation.started_at)
                .or_else(|| {
                    std::fs::metadata(&conversation.source_path)
                        .ok()
                        .as_ref()
                        .and_then(modified_ms)
                }),
            source_path: conversation.source_path,
            deletable,
        })
        .collect()
}

/// The id the agent's own resume flag takes.
///
/// FAD's `external_id` is a store identifier for some connectors (Claude's is
/// the path relative to the projects root), so the transcript's own session id
/// wins first; `external_id` is only used when it is not a path. The file name
/// is the last resort.
fn session_id(conversation: &NormalizedConversation) -> String {
    const KEYS: [&str; 8] = [
        "sessionId",
        "session_id",
        "cliSessionId",
        "conversation_id",
        "conversationId",
        "thread_id",
        "threadId",
        "uuid",
    ];
    KEYS.iter()
        .find_map(|key| {
            conversation
                .metadata
                .get(*key)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            conversation
                .external_id
                .as_deref()
                .filter(|id| !looks_like_path(id))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| file_stem(&conversation.source_path))
}

/// Applies one agent's store knowledge to a FAD connector's output.
///
/// Two stores hold information outside the transcript: Kimi records the working
/// directory in a sibling `state.json`, and Claude writes sub-agent transcripts
/// below the session that owns them. FAD's connectors apply neither, and they
/// cannot be edited here, so the knowledge lives in an adapter bound to that one
/// agent instead of in the shared normalization path.
struct AdjustedConnector {
    inner: Box<dyn Connector + Send>,
    adjust: fn(&mut Vec<NormalizedConversation>),
}

impl Connector for AdjustedConnector {
    fn detect(&self) -> DetectionResult {
        self.inner.detect()
    }

    fn scan(&self, ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
        let mut conversations = self.inner.scan(ctx)?;
        (self.adjust)(&mut conversations);
        Ok(conversations)
    }

    fn discover_source_files(
        &self,
        ctx: &ScanContext,
    ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
        self.inner.discover_source_files(ctx)
    }
}

fn looks_like_path(id: &str) -> bool {
    id.contains('/') || id.ends_with(".jsonl") || id.ends_with(".json")
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connector with one known store root and nothing to scan.
    struct FixedRoots(PathBuf);

    impl Connector for FixedRoots {
        fn detect(&self) -> DetectionResult {
            DetectionResult {
                detected: true,
                evidence: vec![self.0.display().to_string()],
                root_paths: vec![self.0.clone()],
            }
        }

        fn scan(&self, _ctx: &ScanContext) -> anyhow::Result<Vec<NormalizedConversation>> {
            Ok(Vec::new())
        }

        fn discover_source_files(
            &self,
            _ctx: &ScanContext,
        ) -> anyhow::Result<Vec<DiscoveredSourceFile>> {
            Ok(Vec::new())
        }
    }

    /// A directory this test owns, cleared before use so a previous run cannot
    /// make the assertions pass by accident.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    /// The path the API asked to delete has to be in the store the connector
    /// reports, because FAD does not expose which file a row came from.
    #[test]
    fn ensure_inside_store_accepts_a_file_under_a_detected_root() {
        let store = scratch("fad-inside");
        let transcript = store.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");
        let connector = FixedRoots(store.canonicalize().expect("resolve the store"));

        ensure_inside_store(&connector, &transcript).expect("a path in the store is allowed");

        std::fs::remove_dir_all(&store).ok();
    }

    /// A transcript elsewhere on disk is refused: the row only ever names a
    /// file in its own store.
    #[test]
    fn ensure_inside_store_refuses_a_file_outside_every_detected_root() {
        let store = scratch("fad-outside");
        let outside = store.join("outside");
        std::fs::create_dir_all(&outside).expect("create the outside directory");
        let transcript = outside.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");
        let root = store.join("projects");
        std::fs::create_dir_all(&root).expect("create the store");
        let connector = FixedRoots(root.canonicalize().expect("resolve the store"));

        let error =
            ensure_inside_store(&connector, &transcript).expect_err("outside must be refused");

        assert!(
            error.contains("outside the session store"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(
            transcript.exists(),
            "a refused delete must leave the file alone"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    /// A connector that detected nothing has no roots, so nothing may be
    /// deleted through it.
    #[test]
    fn ensure_inside_store_refuses_when_no_root_was_detected() {
        let store = scratch("fad-undetected");
        let transcript = store.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");
        let missing = store.join("missing");
        let connector = FixedRoots(missing);

        let error = ensure_inside_store(&connector, &transcript)
            .expect_err("a connector with no detected root must refuse");

        assert!(
            error.contains("outside the session store"),
            "the refusal should name the reason, got: {error}"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn delete_transcript_removes_a_transcript() {
        let store = scratch("fad-delete");
        let transcript = store.join("session.jsonl");
        std::fs::write(&transcript, "{}\n").expect("write the transcript");

        delete_transcript(&transcript).expect("a transcript is removable");

        assert!(!transcript.exists(), "the transcript should be gone");
        std::fs::remove_dir_all(&store).ok();
    }

    /// Ampersand, older Gemini builds and Copilot's editor store write one
    /// `.json` per session, so that spelling is a transcript too.
    #[test]
    fn delete_transcript_accepts_a_json_session() {
        let store = scratch("fad-delete-json");
        let transcript = store.join("thread.json");
        std::fs::write(&transcript, "{}\n").expect("write the session");

        delete_transcript(&transcript).expect("a .json session is removable");

        assert!(!transcript.exists(), "the session should be gone");
        std::fs::remove_dir_all(&store).ok();
    }

    /// Only transcript files are unlinked; the check is cheap and keeps a
    /// path that is not a session from being removed.
    #[test]
    fn delete_transcript_refuses_a_file_that_is_not_a_transcript() {
        let store = scratch("fad-not-transcript");
        let document = store.join("notes.md");
        std::fs::write(&document, "{}\n").expect("write the document");

        let error = delete_transcript(&document).expect_err("a non-transcript must be refused");

        assert!(
            error.contains("not a transcript"),
            "the refusal should name the reason, got: {error}"
        );
        assert!(document.exists(), "a refused delete must leave the file alone");
        std::fs::remove_dir_all(&store).ok();
    }

    /// FAD's slug is the label unless the daemon reports a different one; the
    /// panel matches a history row against a pane by that label.
    #[test]
    fn agent_label_maps_the_slugs_that_differ() {
        assert_eq!(agent_label("pi_agent"), "pi");
        assert_eq!(agent_label("pi_durable"), "pi");
        assert_eq!(agent_label("codebuddy"), "codebuddy");
    }

    /// A row the panel lists must also be a row the panel can open. The scanned
    /// set and the resumable set are therefore one set: a connector registered
    /// here without a resume plan is a history entry that answers
    /// `resume_unsupported` when clicked.
    #[test]
    fn every_scanned_agent_has_a_resume_plan() {
        for (slug, _) in connector_registry() {
            let label = agent_label(slug);
            let session_ref = crate::agent_resume::AgentSessionRef::id("session-1")
                .expect("a plain id is a valid session reference");
            let planned = crate::agent_resume::plan(
                &format!("hicodeer:{label}"),
                label,
                &session_ref,
            );
            let planned = planned
                .unwrap_or_else(|| panic!("{slug} is scanned but has no resume plan"));
            let expected = crate::detect::parse_agent_label(label)
                .map(crate::detect::interactive_agent_executable);
            assert_eq!(
                planned.argv.first().map(String::as_str),
                expected,
                "{slug} must launch the executable the panel starts"
            );
        }
    }
}
