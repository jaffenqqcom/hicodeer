use std::collections::HashSet;
use std::io;
use std::path::Path;

use jsonc_parser::ast::{Array as AstArray, Object as AstObject, Value as AstValue};
use jsonc_parser::common::Ranged;
use jsonc_parser::cst::{CstNode, CstObject, CstRootNode};
use jsonc_parser::{parse_to_ast, CollectOptions, ParseOptions};
use serde_json::{json as serde_json_value, Map, Value};

use super::command::hook_command;
use super::config_edit::{
    ensure_command_hook, ensure_hooks_object, hook_command_variants, hooks_object_if_present,
    is_matching_command_hook,
};

// Claude's documented SessionStart sources. Grok imports Claude hooks but uses
// `new`/`load`; filter before it starts an unnecessary hook process.
const SESSION_START_MATCHER: &str = "^(startup|resume|clear|compact|fork)$";
// Asking the user a question parks the turn on that answer, so the matching
// tool call reports blocked while every other tool reports working. Without
// the split a working report stays as the hook authority for the whole parked
// turn and the panel keeps showing working.
const ASK_USER_MATCHER_EVENT: &str = "PreToolUse";
const ASK_USER_MATCHER: &str = "^AskUserQuestion$";
const OTHER_TOOLS_MATCHER: &str = "^(?!AskUserQuestion$).*$";

struct HookRemoval {
    event: &'static str,
    actions: &'static [&'static str],
}

const HOOK_REMOVALS: &[HookRemoval] = &[
    HookRemoval {
        event: "PostToolUse",
        actions: &["working"],
    },
    HookRemoval {
        event: "PostToolUseFailure",
        actions: &["working"],
    },
    HookRemoval {
        event: "SubagentStop",
        actions: &["working"],
    },
    HookRemoval {
        event: "PermissionRequest",
        actions: &["blocked"],
    },
    HookRemoval {
        event: "SessionStart",
        actions: &["idle", "session"],
    },
    HookRemoval {
        event: "UserPromptSubmit",
        actions: &["working"],
    },
    HookRemoval {
        event: "PreToolUse",
        // `blocked` carries the AskUserQuestion matcher; `working` covers every
        // other tool. Both are written by `install`, so both must be removed.
        actions: &["working", "blocked"],
    },
    HookRemoval {
        event: "Notification",
        // `blocked` is what earlier builds wrote for this event.
        actions: &["notification", "blocked"],
    },
    HookRemoval {
        event: "Stop",
        actions: &["idle"],
    },
    HookRemoval {
        event: "SessionEnd",
        actions: &["release"],
    },
];

pub(crate) fn install(content: &str, settings_path: &Path, hook_path: &Path) -> io::Result<String> {
    let original = parse_value(content, settings_path)?;
    let mut desired = original.clone();
    let hooks = ensure_hooks_object(
        &mut desired,
        settings_path,
        "claude settings",
        "claude settings hooks",
    )?;
    let canonical = canonical_hook_value(hook_path);
    apply_value_removals(hooks, hook_path, Some(&canonical))?;
    ensure_command_hook(
        hooks,
        "SessionStart",
        hook_command(hook_path, Some("session")),
        10,
        Some(SESSION_START_MATCHER),
    )?;
    ensure_command_hook(
        hooks,
        ASK_USER_MATCHER_EVENT,
        hook_command(hook_path, Some("blocked")),
        10,
        Some(ASK_USER_MATCHER),
    )?;
    ensure_command_hook(
        hooks,
        ASK_USER_MATCHER_EVENT,
        hook_command(hook_path, Some("working")),
        10,
        Some(OTHER_TOOLS_MATCHER),
    )?;
    ensure_command_hook(
        hooks,
        "Notification",
        hook_command(hook_path, Some("notification")),
        10,
        None,
    )?;

    if desired == original {
        return Ok(content.to_string());
    }

    rewrite(
        content,
        settings_path,
        hook_path,
        EditKind::Install,
        &desired,
    )
}

pub(crate) fn uninstall(
    content: &str,
    settings_path: &Path,
    hook_path: &Path,
) -> io::Result<String> {
    let original = parse_value(content, settings_path)?;
    let mut desired = original.clone();
    let mut removed = false;

    if let Some(hooks) = hooks_object_if_present(
        &mut desired,
        settings_path,
        "claude settings",
        "claude settings hooks",
    )? {
        removed = apply_value_removals(hooks, hook_path, None)?;
    }

    if !removed {
        return Ok(content.to_string());
    }

    rewrite(
        content,
        settings_path,
        hook_path,
        EditKind::Uninstall,
        &desired,
    )
}

fn apply_value_removals(
    hooks: &mut Map<String, Value>,
    hook_path: &Path,
    canonical: Option<&Value>,
) -> io::Result<bool> {
    let mut removed = false;
    for policy in HOOK_REMOVALS {
        let commands = removal_commands(policy, hook_path);
        removed |= remove_value_event_commands(
            hooks,
            policy.event,
            &commands,
            (policy.event == "SessionStart")
                .then_some(canonical)
                .flatten(),
        )?;
    }
    Ok(removed)
}

fn remove_value_event_commands(
    hooks: &mut Map<String, Value>,
    event: &str,
    commands: &[String],
    canonical: Option<&Value>,
) -> io::Result<bool> {
    let Some(entries_value) = hooks.get_mut(event) else {
        return Ok(false);
    };
    let entries = entries_value
        .as_array_mut()
        .ok_or_else(|| io::Error::other(format!("hook entries for {event} must be an array")))?;
    let mut removed = false;
    let mut canonical_preserved = false;

    entries.retain_mut(|entry| {
        if !canonical_preserved && canonical.is_some_and(|canonical| entry == canonical) {
            canonical_preserved = true;
            return true;
        }
        let Some(command_entries) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = command_entries.len();
        command_entries.retain(|entry| {
            !commands
                .iter()
                .any(|command| is_matching_command_hook(entry, command))
        });
        // Only an entry this pass emptied is ours to drop. A group the user
        // left with no commands at all is untouched configuration.
        let pruned = command_entries.len() != before;
        removed |= pruned;
        !pruned || !command_entries.is_empty()
    });

    if removed && entries.is_empty() && canonical.is_none() {
        hooks.remove(event);
    }
    Ok(removed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Install,
    Uninstall,
}

fn rewrite(
    content: &str,
    settings_path: &Path,
    hook_path: &Path,
    kind: EditKind,
    desired: &Value,
) -> io::Result<String> {
    let root = CstRootNode::parse(content, &strict_parse_options()).map_err(|err| {
        io::Error::other(format!(
            "failed to parse {}: {err}",
            settings_path.display()
        ))
    })?;
    let root_value = root.value().ok_or_else(|| {
        io::Error::other(format!(
            "claude settings at {} must be a JSON object",
            settings_path.display()
        ))
    })?;
    reject_duplicate_keys(&root_value, settings_path)?;
    let root_object = root_value.as_object().ok_or_else(|| {
        io::Error::other(format!(
            "claude settings at {} must be a JSON object",
            settings_path.display()
        ))
    })?;

    let has_hooks = root_object.get("hooks").is_some();
    if has_hooks {
        let hooks = root_object
            .get("hooks")
            .and_then(|property| property.object_value())
            .ok_or_else(|| {
                io::Error::other(format!(
                    "claude settings hooks at {} must be a JSON object",
                    settings_path.display()
                ))
            })?;
        let canonical = canonical_hook_value(hook_path);
        for policy in HOOK_REMOVALS {
            let commands = removal_commands(policy, hook_path);
            remove_event_commands(
                &hooks,
                policy.event,
                &commands,
                kind == EditKind::Install,
                &canonical,
            )?;
        }
    }

    let text = root.to_string();
    match kind {
        EditKind::Install => {
            let appended = append_missing_hooks(&text, settings_path, desired)?;
            verify_rendered(appended, settings_path, desired)
        }
        EditKind::Uninstall if has_hooks => verify_updated(text, settings_path, desired),
        EditKind::Uninstall => Ok(content.to_string()),
    }
}

/// Inserts every hook entry the install promises that the document lacks yet.
///
/// The removal pass above deletes our own commands so a stale matcher or
/// timeout cannot survive an upgrade, which leaves the document without the
/// events install has to guarantee. Inserting the missing pieces textually -
/// rather than re-rendering the object - keeps every byte the user wrote,
/// including their key order, quoting, spacing and line endings.
fn append_missing_hooks(
    content: &str,
    settings_path: &Path,
    desired: &Value,
) -> io::Result<String> {
    let Some(wanted) = desired.get("hooks").and_then(Value::as_object) else {
        return Ok(content.to_string());
    };

    let mut text = content.to_string();
    while let Some(updated) = next_hook_append(&text, settings_path, wanted)? {
        text = updated;
    }
    Ok(text)
}

/// Performs a single insertion and returns the rewritten document, or `None`
/// once the document already carries every event and entry install wants.
fn next_hook_append(
    content: &str,
    settings_path: &Path,
    wanted: &Map<String, Value>,
) -> io::Result<Option<String>> {
    let root = parse_ast_root_object(content, settings_path)?;
    let Some(hooks) = root.get_object("hooks") else {
        let value = render_json(&Value::Object(wanted.clone()), settings_path)?;
        return Ok(Some(append_object_property(content, &root, "hooks", &value)));
    };

    for (event, entries) in wanted {
        let entries = entries.as_array().ok_or_else(|| {
            io::Error::other(format!("hook entries for {event} must be an array"))
        })?;
        let Some(array) = hooks.get_array(event) else {
            let value = render_json(&Value::Array(entries.clone()), settings_path)?;
            return Ok(Some(append_object_property(content, hooks, event, &value)));
        };

        let existing: Vec<Value> = array
            .elements
            .iter()
            .filter_map(|element| {
                let range = element.range();
                serde_json::from_str(&content[range.start..range.end]).ok()
            })
            .collect();
        if let Some(entry) = entries
            .iter()
            .find(|entry| !existing.iter().any(|have| have == *entry))
        {
            let value = render_json(entry, settings_path)?;
            return Ok(Some(append_array_element(content, &array, &value)));
        }
    }

    Ok(None)
}

fn render_json(value: &Value, settings_path: &Path) -> io::Result<String> {
    serde_json::to_string(value).map_err(|err| {
        io::Error::other(format!(
            "failed to render claude settings at {}: {err}",
            settings_path.display()
        ))
    })
}

fn remove_event_commands(
    hooks: &CstObject,
    event: &str,
    commands: &[String],
    installing: bool,
    canonical: &Value,
) -> io::Result<()> {
    let Some(event_property) = hooks.get(event) else {
        return Ok(());
    };
    let entries = event_property
        .array_value()
        .ok_or_else(|| io::Error::other(format!("hook entries for {event} must be an array")))?;
    let mut canonical_preserved = false;
    let mut pruned_any = false;

    for entry in entries.elements() {
        if installing
            && event == "SessionStart"
            && !canonical_preserved
            && entry.to_serde_value().as_ref() == Some(canonical)
        {
            canonical_preserved = true;
            continue;
        }

        let Some(entry_object) = entry.as_object() else {
            continue;
        };
        let Some(command_entries) = entry_object
            .get("hooks")
            .and_then(|property| property.array_value())
        else {
            continue;
        };

        let mut pruned = false;
        for command_entry in command_entries.elements() {
            let matches = command_entry.to_serde_value().is_some_and(|value| {
                commands
                    .iter()
                    .any(|command| is_matching_command_hook(&value, command))
            });
            if matches {
                command_entry.remove();
                pruned = true;
            }
        }

        // Only an entry this pass emptied is ours to drop; a group the user
        // left with no commands at all is untouched configuration.
        if pruned {
            pruned_any = true;
            if command_entries.elements().is_empty() {
                entry.remove();
            }
        }
    }

    if pruned_any && entries.elements().is_empty() && !(installing && event == "SessionStart") {
        event_property.remove();
    }

    Ok(())
}

fn removal_commands(policy: &HookRemoval, hook_path: &Path) -> Vec<String> {
    policy
        .actions
        .iter()
        .flat_map(|action| hook_command_variants(hook_path, Some(action)))
        .collect()
}

fn canonical_hook_value(hook_path: &Path) -> Value {
    serde_json_value!({
        "matcher": SESSION_START_MATCHER,
        "hooks": [{
            "type": "command",
            "command": hook_command(hook_path, Some("session")),
            "timeout": 10,
        }],
    })
}

fn parse_ast_root_object<'a>(content: &'a str, settings_path: &Path) -> io::Result<AstObject<'a>> {
    let parsed = parse_to_ast(content, &CollectOptions::default(), &strict_parse_options())
        .map_err(|err| {
            io::Error::other(format!(
                "failed to parse {}: {err}",
                settings_path.display()
            ))
        })?;
    match parsed.value {
        Some(AstValue::Object(object)) => Ok(object),
        _ => Err(io::Error::other(format!(
            "claude settings at {} must be a JSON object",
            settings_path.display()
        ))),
    }
}

fn append_object_property(
    content: &str,
    object: &AstObject<'_>,
    name: &str,
    value: &str,
) -> String {
    let key = serde_json::to_string(name).expect("JSON object keys are serializable");
    let key_value_separator = object
        .properties
        .first()
        .map(|property| &content[property.name.range().end..property.value.range().start])
        .unwrap_or(":");
    let insertion = format!("{key}{key_value_separator}{value}");
    let delimiter = object_delimiter(content, object);
    // The comma belongs right after the property that was last written; the
    // whitespace and newline before the closing brace are part of the layout
    // the user already had, so they stay at the end of the container.
    let insertion_index = object
        .properties
        .last()
        .map_or(object.range.start + 1, |property| property.range.end);
    append_to_container(
        content,
        insertion_index,
        !object.properties.is_empty(),
        delimiter,
        &insertion,
    )
}

fn append_array_element(content: &str, array: &AstArray<'_>, value: &str) -> String {
    let delimiter = array_delimiter(content, array);
    let insertion_index = array
        .elements
        .last()
        .map_or(array.range.start + 1, |element| element.range().end);
    append_to_container(
        content,
        insertion_index,
        !array.elements.is_empty(),
        delimiter,
        value,
    )
}

fn object_delimiter<'a>(content: &'a str, object: &AstObject<'_>) -> &'a str {
    match object.properties.as_slice() {
        [first, second, ..] => delimiter_suffix(&content[first.range.end..second.range.start]),
        [first] => &content[object.range.start + 1..first.range.start],
        [] => "",
    }
}

fn array_delimiter<'a>(content: &'a str, array: &AstArray<'_>) -> &'a str {
    match array.elements.as_slice() {
        [first, second, ..] => delimiter_suffix(&content[first.range().end..second.range().start]),
        [first] => &content[array.range.start + 1..first.range().start],
        [] => "",
    }
}

fn delimiter_suffix(delimiter: &str) -> &str {
    delimiter
        .split_once(',')
        .map(|(_, suffix)| suffix)
        .unwrap_or(delimiter)
}

fn append_to_container(
    content: &str,
    insertion_index: usize,
    has_elements: bool,
    delimiter: &str,
    value: &str,
) -> String {
    let mut updated = String::with_capacity(content.len() + delimiter.len() + value.len() + 1);
    updated.push_str(&content[..insertion_index]);
    if has_elements {
        updated.push(',');
        updated.push_str(delimiter);
    }
    updated.push_str(value);
    updated.push_str(&content[insertion_index..]);
    updated
}

fn verify_updated(updated: String, settings_path: &Path, desired: &Value) -> io::Result<String> {
    let actual = parse_value(&updated, settings_path)?;
    if &actual != desired {
        return Err(io::Error::other(format!(
            "failed to safely update claude settings at {}",
            settings_path.display()
        )));
    }
    Ok(updated)
}

/// Accepts the surgical edit when it reproduces the settings we intended to
/// write, and otherwise writes the intended object outright.
///
/// Refusing to install because a formatting-preserving edit came out short is
/// what kept the Claude hooks from ever landing on a settings file that needed
/// more than one event created; a settings file that carries every hook is
/// functional even when its layout had to be normalized.
fn verify_rendered(updated: String, settings_path: &Path, desired: &Value) -> io::Result<String> {
    match parse_value(&updated, settings_path) {
        Ok(actual) if &actual == desired => Ok(updated),
        _ => {
            let mut text = serde_json::to_string_pretty(desired).map_err(|err| {
                io::Error::other(format!(
                    "failed to render claude settings at {}: {err}",
                    settings_path.display()
                ))
            })?;
            text.push('\n');
            Ok(text)
        }
    }
}

fn parse_value(content: &str, settings_path: &Path) -> io::Result<Value> {
    serde_json::from_str(content).map_err(|err| {
        io::Error::other(format!(
            "failed to parse {}: {err}",
            settings_path.display()
        ))
    })
}

fn reject_duplicate_keys(node: &CstNode, settings_path: &Path) -> io::Result<()> {
    if let Some(object) = node.as_object() {
        let mut names = HashSet::new();
        for property in object.properties() {
            let name = property
                .name()
                .ok_or_else(|| io::Error::other("JSON object property is missing a name"))?
                .decoded_value()
                .map_err(|err| io::Error::other(format!("failed to decode JSON key: {err}")))?;
            if !names.insert(name.clone()) {
                return Err(io::Error::other(format!(
                    "claude settings at {} contains duplicate key {name:?}",
                    settings_path.display()
                )));
            }
            if let Some(value) = property.value() {
                reject_duplicate_keys(&value, settings_path)?;
            }
        }
    } else if let Some(array) = node.as_array() {
        for element in array.elements() {
            reject_duplicate_keys(&element, settings_path)?;
        }
    }
    Ok(())
}

fn strict_parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: false,
        allow_loose_object_property_names: false,
        allow_trailing_commas: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (&'static Path, &'static Path) {
        (
            Path::new("/home/test/.claude/settings.json"),
            Path::new("/home/test/.claude/hooks/hicodeer-agent-state.sh"),
        )
    }

    /// The hook entries install is expected to write for one event. Spelling
    /// them out here keeps the byte-exact expectations below independent of the
    /// way the settings file is laid out.
    fn expected_entries(hook_path: &Path, event: &str) -> Vec<Value> {
        match event {
            "SessionStart" => vec![canonical_hook_value(hook_path)],
            ASK_USER_MATCHER_EVENT => vec![
                serde_json_value!({
                    "matcher": ASK_USER_MATCHER,
                    "hooks": [{
                        "type": "command",
                        "command": hook_command(hook_path, Some("blocked")),
                        "timeout": 10,
                    }],
                }),
                serde_json_value!({
                    "matcher": OTHER_TOOLS_MATCHER,
                    "hooks": [{
                        "type": "command",
                        "command": hook_command(hook_path, Some("working")),
                        "timeout": 10,
                    }],
                }),
            ],
            "Notification" => vec![serde_json_value!({
                "hooks": [{
                    "type": "command",
                    "command": hook_command(hook_path, Some("notification")),
                    "timeout": 10,
                }],
            })],
            other => panic!("unexpected hook event {other}"),
        }
    }

    fn expected_event(hook_path: &Path, event: &str) -> String {
        serde_json::to_string(&Value::Array(expected_entries(hook_path, event)))
            .expect("hook events are serializable")
    }

    fn expected_notification_entry(hook_path: &Path) -> String {
        serde_json::to_string(&expected_entries(hook_path, "Notification")[0])
            .expect("hook entries are serializable")
    }

    fn expected_session_entry(hook_path: &Path) -> String {
        serde_json::to_string(&expected_entries(hook_path, "SessionStart")[0])
            .expect("hook entries are serializable")
    }

    #[test]
    fn install_preserves_untouched_formatting_and_complete_trailing_suffix() {
        let (settings_path, hook_path) = paths();
        let input = concat!(
            "{\r\n",
            "    \"zeta\" : {\"escaped\":\"\\u0061\", \"number\":1e+02},\r\n",
            "    \"hooks\" : {\r\n",
            "        \"Notification\" : [{\"matcher\":\"keep\",\"hooks\":[]}]\r\n",
            "    },\r\n",
            "    \"alpha\" : 1\r\n",
            "}\r\n\r\n",
        );

        let updated = install(input, settings_path, hook_path).unwrap();

        assert!(
            updated.starts_with(&format!(
                concat!(
                    "{{\r\n",
                    "    \"zeta\" : {{\"escaped\":\"\\u0061\", \"number\":1e+02}},\r\n",
                    "    \"hooks\" : {{\r\n",
                    "        \"Notification\" : [{{\"matcher\":\"keep\",\"hooks\":[]}},{notification_entry}],\r\n",
                ),
                notification_entry = expected_notification_entry(hook_path),
            )),
            "{updated:?}"
        );
        assert!(updated.ends_with(concat!(
            "\r\n    },\r\n",
            "    \"alpha\" : 1\r\n",
            "}\r\n\r\n",
        )));
        assert!(!updated.replace("\r\n", "").contains('\n'));
        assert!(updated.contains("\"SessionStart\""));
        assert_eq!(
            serde_json::from_str::<Value>(&updated).unwrap()["zeta"]["number"],
            100.0
        );
    }

    #[test]
    fn install_keeps_compact_containers_compact() {
        let (settings_path, hook_path) = paths();
        let session = expected_event(hook_path, "SessionStart");
        let pre_tool = expected_event(hook_path, ASK_USER_MATCHER_EVENT);
        let notification = expected_event(hook_path, "Notification");
        let notification_entry = expected_notification_entry(hook_path);
        let session_entry = expected_session_entry(hook_path);
        let cases = [
            (
                "{\"zeta\":{\"escaped\":\"\\u0061\",\"n\":1e+02},\"alpha\":1}\r\n".to_string(),
                format!(
                    "{{\"zeta\":{{\"escaped\":\"\\u0061\",\"n\":1e+02}},\"alpha\":1,\"hooks\":{{\"SessionStart\":{session},\"PreToolUse\":{pre_tool},\"Notification\":{notification}}}}}\r\n"
                ),
            ),
            (
                "{\"hooks\":{\"Notification\":[{\"matcher\":\"keep\",\"hooks\":[]}]}, \"alpha\":1}"
                    .to_string(),
                format!(
                    "{{\"hooks\":{{\"Notification\":[{{\"matcher\":\"keep\",\"hooks\":[]}},{notification_entry}],\"SessionStart\":{session},\"PreToolUse\":{pre_tool}}}, \"alpha\":1}}"
                ),
            ),
            (
                "{\"hooks\":{\"SessionStart\":[{\"matcher\":\"keep\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo keep\"}]}]}}"
                    .to_string(),
                format!(
                    "{{\"hooks\":{{\"SessionStart\":[{{\"matcher\":\"keep\",\"hooks\":[{{\"type\":\"command\",\"command\":\"echo keep\"}}]}},{session_entry}],\"PreToolUse\":{pre_tool},\"Notification\":{notification}}}}}"
                ),
            ),
            (
                "{\"zeta\":{\n  \"x\":1\n},\"alpha\":1}".to_string(),
                format!(
                    "{{\"zeta\":{{\n  \"x\":1\n}},\"alpha\":1,\"hooks\":{{\"SessionStart\":{session},\"PreToolUse\":{pre_tool},\"Notification\":{notification}}}}}"
                ),
            ),
            (
                "{\"hooks\":{\"Notification\":[\n  {\"matcher\":\"keep\",\"hooks\":[]}\n]},\"alpha\":1}"
                    .to_string(),
                format!(
                    "{{\"hooks\":{{\"Notification\":[\n  {{\"matcher\":\"keep\",\"hooks\":[]}},\n  {notification_entry}\n],\"SessionStart\":{session},\"PreToolUse\":{pre_tool}}},\"alpha\":1}}"
                ),
            ),
            (
                "{\"hooks\":{\"SessionStart\":[{\n  \"matcher\":\"keep\",\n  \"hooks\":[{\"type\":\"command\",\"command\":\"echo keep\"}]\n}]}}"
                    .to_string(),
                format!(
                    "{{\"hooks\":{{\"SessionStart\":[{{\n  \"matcher\":\"keep\",\n  \"hooks\":[{{\"type\":\"command\",\"command\":\"echo keep\"}}]\n}},{session_entry}],\"PreToolUse\":{pre_tool},\"Notification\":{notification}}}}}"
                ),
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(install(&input, settings_path, hook_path).unwrap(), expected);
        }
    }

    #[test]
    fn install_scopes_claude_session_start_sources() {
        let (settings_path, hook_path) = paths();
        let installed = install("{}", settings_path, hook_path).unwrap();
        let settings: Value = serde_json::from_str(&installed).unwrap();
        let matcher = settings["hooks"]["SessionStart"][0]["matcher"]
            .as_str()
            .unwrap();
        assert_eq!(matcher, "^(startup|resume|clear|compact|fork)$");
        let pattern = regex::Regex::new(matcher).unwrap();
        for source in ["startup", "resume", "clear", "compact", "fork"] {
            assert!(pattern.is_match(source), "Claude source: {source}");
        }
        for source in ["new", "load", "", "future-source", "startup-extra"] {
            assert!(!pattern.is_match(source), "non-Claude source: {source}");
        }
    }

    #[test]
    fn install_is_a_byte_exact_noop_once_the_hooks_are_installed() {
        let (settings_path, hook_path) = paths();
        let command = serde_json::to_string(&hook_command(hook_path, Some("session"))).unwrap();
        let canonical = format!(
            "{{\"hooks\":[{{\"timeout\":10,\"command\":{command},\"type\":\"command\"}}],\"matcher\":\"{SESSION_START_MATCHER}\"}}"
        );
        let input = format!(
            "{{\"hooks\":{{\"SessionStart\":[{canonical}]}},\"escaped\":\"\\u0061\"}}  \r\n\r\n"
        );

        let installed = install(&input, settings_path, hook_path).unwrap();

        assert!(installed.contains(&canonical));
        assert!(installed.ends_with("\"escaped\":\"\\u0061\"}  \r\n\r\n"));
        assert_eq!(
            install(&installed, settings_path, hook_path).unwrap(),
            installed
        );
    }

    #[test]
    fn install_migrates_wildcard_session_start_and_preserves_user_hook() {
        let (settings_path, hook_path) = paths();
        let command = serde_json::to_string(&hook_command(hook_path, Some("session"))).unwrap();
        let user_hook = r#"{ "type" : "command", "command" : "echo keep", "timeout" : 3 }"#;
        let input = format!(
            "{{\n  \"hooks\": {{\n    \"SessionStart\": [{{\"matcher\":\"*\",\"hooks\":[{{\"type\":\"command\",\"command\":{command},\"timeout\":10}},{user_hook}]}}]\n  }}\n}}\n\n"
        );
        let installed = install(&input, settings_path, hook_path).unwrap();
        assert!(installed.contains(user_hook));
        assert!(installed.ends_with("}\n\n"));
        let settings: Value = serde_json::from_str(&installed).unwrap();
        let groups = settings["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["matcher"], "*");
        assert_eq!(groups[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "echo keep");
        assert_eq!(groups[1], canonical_hook_value(hook_path));
        assert_eq!(
            install(&installed, settings_path, hook_path).unwrap(),
            installed
        );

        let removed = uninstall(&installed, settings_path, hook_path).unwrap();
        assert!(removed.contains(user_hook));
        assert!(!removed.contains(&command));
        let settings: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(
            settings["hooks"]["SessionStart"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn install_preserves_canonical_session_start_position_during_migration() {
        let (settings_path, hook_path) = paths();
        let pre_tool = expected_event(hook_path, ASK_USER_MATCHER_EVENT);
        let notification = expected_event(hook_path, "Notification");
        let canonical =
            serde_json::to_string(&canonical_hook_value(hook_path)).expect("canonical hook");
        let old_command = serde_json::to_string(&hook_command(hook_path, Some("working"))).unwrap();
        let session_start = format!(
            "\"SessionStart\":[{canonical},{{\"matcher\":\"foreign\",\"hooks\":[{{\"type\":\"command\",\"command\":\"echo keep\"}}]}}]"
        );
        let old_event = [
            "\"PostToolUse\":[{\"matcher\":\"*\",\"hooks\":[{\"type\":\"command\",\"command\":",
            &old_command,
            "}]}]",
        ]
        .concat();
        let input = ["{\"hooks\":{", &session_start, ",", &old_event, "}}"].concat();
        let expected = [
            "{\"hooks\":{",
            &session_start,
            ",\"PreToolUse\":",
            &pre_tool,
            ",\"Notification\":",
            &notification,
            "}}",
        ]
        .concat();

        let updated = install(&input, settings_path, hook_path).unwrap();

        assert_eq!(updated, expected);
    }

    #[test]
    fn install_removes_only_owned_commands_from_shared_hook_groups() {
        let (settings_path, hook_path) = paths();
        let old_command = serde_json::to_string(&hook_command(hook_path, Some("working"))).unwrap();
        let input = format!(
            concat!(
                "{{\n",
                "  \"hooks\": {{\n",
                "    \"PostToolUse\": [{{\n",
                "      \"matcher\": \"*\",\n",
                "      \"hooks\": [\n",
                "        {{\"type\":\"command\",\"command\":{old_command},\"timeout\":10}},\n",
                "        {{  \"type\" : \"command\", \"command\" : \"echo keep\", \"timeout\" : 3  }}\n",
                "      ]\n",
                "    }}],\n",
                "    \"Notification\": [{{\"matcher\":\"keep\",\"hooks\":[]}}]\n",
                "  }}\n",
                "}}\n",
            ),
            old_command = old_command,
        );

        let updated = install(&input, settings_path, hook_path).unwrap();

        assert!(updated.contains(
            "        {  \"type\" : \"command\", \"command\" : \"echo keep\", \"timeout\" : 3  }"
        ));
        assert!(updated.contains("    \"Notification\": [{\"matcher\":\"keep\",\"hooks\":[]}"));
        let parsed: Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(
            parsed["hooks"]["PostToolUse"][0]["hooks"],
            serde_json_value!([{
                "type": "command",
                "command": "echo keep",
                "timeout": 3,
            }]),
            "the owned command is dropped and the user's command survives"
        );
        assert_eq!(parsed["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn uninstall_preserves_unrelated_hook_text() {
        let (settings_path, hook_path) = paths();
        let command = serde_json::to_string(&hook_command(hook_path, Some("session"))).unwrap();
        let input = format!(
            concat!(
                "{{\n",
                "    \"before\" : \"\\u0061\",\n",
                "    \"hooks\" : {{\n",
                "        \"SessionStart\" : [{{\n",
                "            \"matcher\" : \"*\",\n",
                "            \"hooks\" : [\n",
                "                {{\"type\":\"command\",\"command\":{command},\"timeout\":10}},\n",
                "                {{  \"type\" : \"command\", \"command\" : \"echo keep\"  }}\n",
                "            ]\n",
                "        }}]\n",
                "    }},\n",
                "    \"after\" : 1e+02\n",
                "}}\n\n",
            ),
            command = command,
        );

        let updated = uninstall(&input, settings_path, hook_path).unwrap();

        assert_ne!(updated, input);
        assert!(!updated.contains(&command));
        assert!(updated
            .contains("                {  \"type\" : \"command\", \"command\" : \"echo keep\"  }"));
        assert!(updated.starts_with("{\n    \"before\" : \"\\u0061\","));
        assert!(updated.ends_with("    \"after\" : 1e+02\n}\n\n"));
    }



    #[test]
    fn uninstall_removes_every_entry_install_writes() {
        let (settings_path, hook_path) = paths();
        let installed = install("{\n  \"hooks\": {}\n}", settings_path, hook_path).unwrap();
        // Build the expectations through `hook_command`: the quoting differs per
        // platform, and this test is about which entries exist, not about the
        // shell syntax around them.
        for action in ["blocked", "notification"] {
            let command = hook_command(hook_path, Some(action));
            assert!(installed.contains(&command), "{action}: {installed}");
        }

        let removed = uninstall(&installed, settings_path, hook_path).unwrap();

        assert!(
            !removed.contains("agent-state"),
            "uninstall must not leave a hook pointing at the removed script: {removed}"
        );
    }

    #[test]
    fn install_rejects_duplicate_keys() {
        let (settings_path, hook_path) = paths();
        let error = install(
            r#"{"alpha": 1, "alpha": 2, "hooks": {}}"#,
            settings_path,
            hook_path,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("duplicate key \"alpha\""), "{error}");
    }

    #[test]
    fn install_keeps_structurally_invalid_content_unchanged() {
        let (settings_path, hook_path) = paths();
        for input in ["[]", r#"{"hooks": []}"#, r#"{"hooks":{"SessionStart":{}}}"#] {
            assert!(install(input, settings_path, hook_path).is_err());
        }
    }
}
