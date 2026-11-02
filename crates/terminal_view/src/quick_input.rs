// HiCodeer addition (not part of upstream Zed): storage for the terminal's
// saved phrases. Plain, portable Rust so it builds on every target.
use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};

/// Directory under the application data directory. Shared by every terminal,
/// so the list reads the same in each pane.
const QUICK_INPUT_DIR: &str = "terminal";
const QUICK_INPUT_FILE: &str = "quick-inputs.json";

/// Longest single phrase, in characters. Longer input is truncated rather than
/// rejected so a paste never silently disappears.
pub const MAX_TEXT_CHARS: usize = 2000;
/// Upper bound on the list. Extra entries in a hand-edited file are dropped.
pub const MAX_ITEMS: usize = 200;
/// A hand-edited file larger than this is treated as broken and left untouched.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct QuickInputFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    items: Vec<String>,
    /// Whether sending a phrase also presses Enter. Stored with the list so the
    /// choice survives restarts; missing in older files means "on".
    #[serde(default = "default_direct_execute")]
    direct_execute: bool,
}

fn default_direct_execute() -> bool {
    true
}

/// The phrases shown by the quick input list.
///
/// The list lives in one file shared by all terminals. A missing file is an
/// empty list; a file that fails to parse is also an empty list, but it is
/// flagged so that saving cannot destroy whatever the user actually had.
pub struct QuickInputs {
    path: PathBuf,
    items: Vec<String>,
    direct_execute: bool,
    loaded: bool,
    parse_failed: bool,
}

impl QuickInputs {
    fn load() -> Self {
        let path = default_path();
        let mut this = Self {
            path,
            items: Vec::new(),
            direct_execute: true,
            loaded: false,
            parse_failed: false,
        };
        this.read();
        this
    }

    /// Re-reads the file from disk.
    ///
    /// Opening the modal calls this so hand edits show up without a restart,
    /// and so a file that failed to parse earlier gets another chance instead
    /// of blocking every future write.
    pub fn reload(&mut self) {
        self.read();
    }

    fn read(&mut self) {
        self.loaded = true;
        self.items.clear();
        self.parse_failed = false;

        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
                log::error!(
                    "quick inputs at {} exceed {MAX_FILE_BYTES} bytes; leaving it alone",
                    self.path.display()
                );
                self.parse_failed = true;
                return;
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                log::error!(
                    "failed to stat quick inputs at {}: {error}",
                    self.path.display()
                );
                self.parse_failed = true;
                return;
            }
        }

        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) => {
                log::error!(
                    "failed to read quick inputs at {}: {error}",
                    self.path.display()
                );
                self.parse_failed = true;
                return;
            }
        };

        // Parse leniently: the file is documented as user-editable, so a single
        // stray value must not throw away the rest of the list.
        let Some(document) = serde_json::from_str::<serde_json::Value>(&contents).ok() else {
            log::error!(
                "quick inputs at {} are not valid JSON",
                self.path.display()
            );
            self.parse_failed = true;
            return;
        };

        self.direct_execute = document
            .get("direct_execute")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);

        let Some(entries) = document
            .get("items")
            .and_then(|items| items.as_array().cloned())
        else {
            log::error!(
                "quick inputs at {} are not an object with an items array",
                self.path.display()
            );
            self.parse_failed = true;
            return;
        };

        for entry in entries {
            if self.items.len() >= MAX_ITEMS {
                log::warn!("quick inputs exceed {MAX_ITEMS} entries; dropping the rest");
                break;
            }
            if let Some(text) = entry.as_str().and_then(sanitize_text) {
                self.items.push(text);
            }
        }
    }

    pub fn direct_execute(&self) -> bool {
        self.direct_execute
    }

    pub fn set_direct_execute(&mut self, direct_execute: bool) {
        if self.direct_execute == direct_execute {
            return;
        }
        self.direct_execute = direct_execute;
        self.save();
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn item(&self, index: usize) -> Option<&str> {
        self.items.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Appends a phrase and returns its index. Blank phrases are dropped and
    /// over-long text is truncated.
    pub fn add(&mut self, text: impl Into<String>) -> Option<usize> {
        let text = sanitize_text(&text.into())?;
        if self.items.len() >= MAX_ITEMS {
            return None;
        }
        self.items.push(text);
        self.save();
        Some(self.items.len() - 1)
    }

    /// Replaces the phrase at `index`. Blank phrases are rejected so an
    /// accidental empty save does not leave an invisible row behind.
    pub fn update(&mut self, index: usize, text: impl Into<String>) -> bool {
        let Some(text) = sanitize_text(&text.into()) else {
            return false;
        };
        let Some(slot) = self.items.get_mut(index) else {
            return false;
        };
        if *slot == text {
            return true;
        }
        *slot = text;
        self.save();
        true
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.items.remove(index);
        self.save();
        true
    }

    fn save(&self) {
        if !self.loaded {
            return;
        }
        // Never overwrite a file this process could not understand: that would
        // turn a transient read problem into permanent data loss.
        if self.parse_failed {
            log::error!(
                "refusing to overwrite unparsable quick inputs at {}",
                self.path.display()
            );
            return;
        }

        if let Some(parent) = self.path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            log::error!(
                "failed to create quick input directory {}: {error}",
                parent.display()
            );
            return;
        }

        let file = QuickInputFile {
            version: FORMAT_VERSION,
            items: self.items.clone(),
            direct_execute: self.direct_execute,
        };
        let json = match serde_json::to_vec_pretty(&file) {
            Ok(json) => json,
            Err(error) => {
                log::error!("failed to serialize quick inputs: {error}");
                return;
            }
        };

        // Write beside the target and rename, so a crash mid-write leaves the
        // previous list intact.
        let temp = self.path.with_extension("json.tmp");
        if let Err(error) = fs::write(&temp, json) {
            log::error!(
                "failed to write quick inputs to {}: {error}",
                temp.display()
            );
            return;
        }
        if let Err(error) = fs::rename(&temp, &self.path) {
            log::error!(
                "failed to replace quick inputs at {}: {error}",
                self.path.display()
            );
        }
    }
}

fn default_path() -> PathBuf {
    paths::data_dir().join(QUICK_INPUT_DIR).join(QUICK_INPUT_FILE)
}

/// Removes control characters and caps the length.
///
/// The phrase is pasted into a terminal, so an embedded ESC or BEL would be
/// interpreted as a control sequence rather than text. Tabs and newlines are
/// kept: they are meaningful in prompts.
fn sanitize_text(text: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(text.len().min(MAX_TEXT_CHARS * 4));
    let mut count = 0;
    for character in text.chars() {
        if count >= MAX_TEXT_CHARS {
            break;
        }
        let keep = character == '\n' || character == '\t' || !character.is_control();
        if keep {
            cleaned.push(character);
            count += 1;
        }
    }
    if cleaned.chars().any(|character| !character.is_whitespace()) {
        Some(cleaned)
    } else {
        None
    }
}

static STORE: OnceLock<Mutex<QuickInputs>> = OnceLock::new();

/// The single list shared by all terminals.
pub fn store() -> &'static Mutex<QuickInputs> {
    STORE.get_or_init(|| Mutex::new(QuickInputs::load()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_at(name: &str) -> QuickInputs {
        QuickInputs {
            path: std::env::temp_dir().join(format!("quick-inputs-{name}.json")),
            items: Vec::new(),
            direct_execute: true,
            loaded: true,
            parse_failed: false,
        }
    }

    fn fresh_store(name: &str) -> QuickInputs {
        let store = store_at(name);
        let _ = fs::remove_file(&store.path);
        store
    }

    #[test]
    fn adds_updates_removes_and_round_trips() {
        let mut inputs = fresh_store("round-trip");
        assert_eq!(inputs.add("first"), Some(0));
        assert_eq!(inputs.add("  second  "), Some(1));
        assert_eq!(inputs.add("   "), None);
        assert_eq!(inputs.items(), ["first", "  second  "]);

        assert!(inputs.update(0, "renamed"));
        assert!(!inputs.update(0, "  "));
        assert_eq!(inputs.item(0), Some("renamed"));

        assert!(inputs.remove(0));
        assert!(!inputs.remove(9));

        let mut reloaded = store_at("round-trip");
        reloaded.read();
        assert_eq!(reloaded.items(), ["  second  "]);
    }

    #[test]
    fn keeps_a_newline() {
        let mut inputs = fresh_store("newline");
        inputs.add("line one\nline two");
        let mut reloaded = store_at("newline");
        reloaded.read();
        assert_eq!(reloaded.item(0), Some("line one\nline two"));
    }

    #[test]
    fn unparsable_file_is_not_overwritten() {
        let mut inputs = fresh_store("broken");
        fs::write(&inputs.path, b"{ not json").unwrap();
        inputs.read();
        assert!(inputs.parse_failed);
        assert!(inputs.is_empty());

        inputs.add("new phrase");
        let on_disk = fs::read_to_string(&inputs.path).unwrap();
        assert_eq!(on_disk, "{ not json");
    }

    #[test]
    fn strips_control_characters_but_keeps_tabs_and_newlines() {
        let mut inputs = fresh_store("control");
        inputs.add("safe\x1b[2Jtext\x07\tline\nnext");
        let mut reloaded = store_at("control");
        reloaded.read();
        assert_eq!(reloaded.item(0), Some("safe[2Jtext\tline\nnext"));
    }

    #[test]
    fn truncates_to_the_character_limit() {
        let mut inputs = fresh_store("long");
        inputs.add("a".repeat(MAX_TEXT_CHARS + 50));
        assert_eq!(
            inputs.item(0).unwrap().chars().count(),
            MAX_TEXT_CHARS,
            "stored value must be capped"
        );
    }

    #[test]
    fn oversized_file_is_ignored_and_left_alone() {
        let mut inputs = fresh_store("oversized");
        let padding = "x".repeat(MAX_FILE_BYTES as usize + 1);
        fs::write(&inputs.path, padding.as_bytes()).unwrap();

        inputs.read();
        assert!(inputs.parse_failed);
        assert!(inputs.is_empty());

        inputs.add("new");
        assert_eq!(
            fs::metadata(&inputs.path).unwrap().len(),
            MAX_FILE_BYTES + 1,
            "an oversized file must not be overwritten"
        );
    }

    #[test]
    fn skips_non_string_entries() {
        let mut inputs = fresh_store("mixed");
        fs::write(
            &inputs.path,
            br#"{"version":1,"items":["keep",42,null,{"text":"no"},"also keep"]}"#,
        )
        .unwrap();

        inputs.read();
        assert_eq!(inputs.items(), ["keep", "also keep"]);
        assert!(!inputs.parse_failed);
    }

    #[test]
    fn caps_the_item_count() {
        let mut inputs = fresh_store("many");
        let items: Vec<String> = (0..MAX_ITEMS + 10).map(|index| format!("item {index}")).collect();
        let json = serde_json::json!({ "version": 1, "items": items }).to_string();
        fs::write(&inputs.path, json).unwrap();

        inputs.read();
        assert_eq!(inputs.len(), MAX_ITEMS);
    }

    #[test]
    fn non_object_file_is_treated_as_broken() {
        let mut inputs = fresh_store("array");
        fs::write(&inputs.path, b"[\"a\"]").unwrap();

        inputs.read();
        assert!(inputs.parse_failed);
        assert!(inputs.is_empty());
    }

    #[test]
    fn strips_carriage_returns_so_a_phrase_cannot_submit_itself() {
        // A stored `\r` would submit the prompt as soon as it is pasted.
        let mut inputs = fresh_store("carriage-return");
        inputs.add("rm -rf build\r\nls");
        // The newline survives on purpose; only the carriage return is dropped.
        assert_eq!(inputs.item(0), Some("rm -rf build\nls"));
    }

    #[test]
    fn rejects_text_that_is_only_control_characters() {
        let mut inputs = fresh_store("control-only");
        assert_eq!(inputs.add("\u{1b}\u{7}\r\u{0}"), None);
        assert!(inputs.is_empty());
    }

    #[test]
    fn remembers_the_direct_execute_choice() {
        let mut inputs = fresh_store("direct-execute");
        assert!(inputs.direct_execute(), "new files default to on");
        inputs.set_direct_execute(false);

        let mut reloaded = store_at("direct-execute");
        reloaded.read();
        assert!(!reloaded.direct_execute());
    }

    #[test]
    fn older_files_without_the_flag_stay_on() {
        let mut inputs = fresh_store("legacy-direct-execute");
        fs::write(&inputs.path, br#"{"version":1,"items":["a"]}"#).unwrap();

        inputs.read();
        assert!(inputs.direct_execute());
    }

    #[test]
    fn truncates_over_long_entries_when_reading() {
        let mut inputs = fresh_store("read-long");
        let json = serde_json::json!({
            "version": 1,
            "items": ["b".repeat(MAX_TEXT_CHARS + 20)],
        })
        .to_string();
        fs::write(&inputs.path, json).unwrap();

        inputs.read();
        assert_eq!(inputs.item(0).unwrap().chars().count(), MAX_TEXT_CHARS);
    }
}
