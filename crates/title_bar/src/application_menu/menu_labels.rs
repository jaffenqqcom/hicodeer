use gpui::{App, KeyBinding, KeybindingKeystroke, SharedString, Window};

use super::OpenApplicationMenu;

pub(super) fn matches(name: &str, requested: &str) -> bool {
    matches_label(name, localization::lookup(name).unwrap_or(name), requested)
}

fn matches_label(name: &str, translated: &str, requested: &str) -> bool {
    // Accept existing locale-specific keymaps without making them the stable identity.
    name == requested || translated == requested
}

pub(super) fn label(name: &str, window: &Window, cx: &App) -> SharedString {
    let translated = localization::lookup(name).unwrap_or(name);
    let bindings = |name: &str| {
        let action = OpenApplicationMenu(name.to_owned());
        match window.focused(cx) {
            Some(focus) => window.bindings_for_action_in(&action, &focus),
            None => window.bindings_for_action(&action),
        }
    };
    let hint = access_key(&bindings(name)).or_else(|| {
        (translated != name)
            .then(|| access_key(&bindings(translated)))
            .flatten()
    });
    format_label(translated, hint).into()
}

fn access_key(bindings: &[KeyBinding]) -> Option<char> {
    bindings
        .iter()
        .rev()
        .find_map(|binding| alt_letter(binding.keystrokes()))
}

fn alt_letter(keystrokes: &[KeybindingKeystroke]) -> Option<char> {
    let [keystroke] = keystrokes else {
        return None;
    };
    let modifiers = keystroke.modifiers();
    if !modifiers.alt || modifiers.number_of_modifiers() != 1 {
        return None;
    }
    let mut characters = keystroke.key().chars();
    let letter = characters.next()?;
    (letter.is_ascii_alphabetic() && characters.next().is_none())
        .then(|| letter.to_ascii_uppercase())
}

fn format_label(translated: &str, hint: Option<char>) -> String {
    match hint {
        Some(letter) => format!("{translated} ({letter})"),
        None => translated.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, FocusHandle, InteractiveElement, IntoElement, Render, TestAppContext, div,
    };

    struct MenuHost(FocusHandle);

    impl Render for MenuHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().key_context("Workspace").track_focus(&self.0)
        }
    }

    #[gpui::test]
    fn hints_follow_active_keymap_and_disappear_when_shadowed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new(
                "alt-f",
                OpenApplicationMenu("File".into()),
                Some("Workspace"),
            )]);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            MenuHost(focus)
        });
        cx.run_until_parked();
        let translated = localization::lookup("File").unwrap_or("File");
        cx.update(|window, cx| {
            assert_eq!(
                label("File", window, cx).as_ref(),
                format!("{translated} (F)")
            );
            cx.bind_keys([KeyBinding::new(
                "alt-f",
                OpenApplicationMenu("Edit".into()),
                Some("Workspace"),
            )]);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(label("File", window, cx).as_ref(), translated);
            cx.bind_keys([KeyBinding::new(
                "alt-x",
                OpenApplicationMenu("File".into()),
                Some("Workspace"),
            )]);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                label("File", window, cx).as_ref(),
                format!("{translated} (X)")
            );
        });
    }

    #[test]
    fn stable_names_work_across_locales_and_legacy_names_still_work() {
        for translated in ["File", "ファイル", "Datei", "파일"] {
            assert!(matches_label("File", translated, "File"));
            assert!(matches_label("File", translated, translated));
            assert!(!matches_label("File", translated, "Edit"));
            assert!(!matches_label(
                "File",
                translated,
                &format!("{translated} (F)")
            ));
        }
    }

    #[test]
    fn hints_follow_alt_letter_bindings_only() {
        let binding = |keys: &str| KeyBinding::new(keys, OpenApplicationMenu("File".into()), None);
        assert_eq!(access_key(&[]), None);
        assert_eq!(access_key(&[binding("alt-f")]), Some('F'));
        assert_eq!(access_key(&[binding("alt-f"), binding("alt-x")]), Some('X'));
        for keys in [
            "f10",
            "ctrl-f",
            "ctrl-alt-f",
            "alt-shift-f",
            "alt-f s",
            "alt-enter",
        ] {
            assert_eq!(access_key(&[binding(keys)]), None, "{keys}");
        }
    }

    #[test]
    fn labels_do_not_claim_an_unbound_access_key() {
        assert_eq!(format_label("ファイル", None), "ファイル");
        assert_eq!(format_label("ファイル", Some('F')), "ファイル (F)");
        assert_eq!(format_label("File", Some('X')), "File (X)");
    }
}
