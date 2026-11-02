// HiCodeer addition (not part of upstream Zed): the quick input modal shown
// over a terminal. Portable GPUI code, not HarmonyOS-specific.
use editor::Editor;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyDownEvent,
    KeystrokeEvent, MouseButton, MouseDownEvent, ScrollHandle, Subscription, WeakEntity, Window, div,
    prelude::*, px,
};
use ui::prelude::*;
use ui::{
    ButtonCommon, ButtonLike, ButtonStyle, Checkbox, Color, Icon, IconName, IconSize, ScrollAxes,
    Scrollbars, ToggleState, Tooltip, WithScrollbar,
};

use crate::{TerminalView, quick_input};

/// The modal takes two fifths of the window, but never gets so narrow that a
/// phrase becomes unreadable.
const MODAL_WIDTH_RATIO: f32 = 0.4;
const MODAL_MIN_WIDTH: f32 = 280.0;
/// Long lists scroll instead of growing past this share of the window.
const MODAL_MAX_HEIGHT_RATIO: f32 = 0.6;
/// Fixed heights used to size the modal; the list itself takes the remainder.
const ROW_HEIGHT: f32 = 38.0;
const HEADER_HEIGHT: f32 = 40.0;
const INPUT_HEIGHT: f32 = 46.0;
const OPTIONS_HEIGHT: f32 = 40.0;
const EMPTY_LIST_HEIGHT: f32 = 56.0;
const MAX_VISIBLE_ROWS: usize = 8;
/// Gap between the paste and the Enter that follows it, long enough for the pty
/// to deliver the paste as its own read.
const DELAYED_ENTER_MS: u64 = 50;

pub struct QuickInputState {
    pub open: bool,
    pub selected: usize,
    /// `Some(index)` means the row at `index` is being edited in place.
    pub editing: Option<usize>,
    /// The always-present "add a phrase" box at the top.
    pub input: Option<Entity<Editor>>,
    /// The editor that replaces a row while it is being edited.
    pub row_editor: Option<Entity<Editor>>,
    /// Drives the list's scrollbar and lets a new or selected phrase be scrolled
    /// into view.
    pub scroll: ScrollHandle,
    /// Esc is also bound inside `Editor`, whose binding wins over any keymap
    /// entry we add. A keystroke interceptor runs before keymap dispatch, so it
    /// is the only place the modal can reliably claim Esc. Held only while the
    /// modal is open.
    escape_interceptor: Option<Subscription>,
    focus_handle: Option<FocusHandle>,
}

impl Default for QuickInputState {
    fn default() -> Self {
        Self {
            open: false,
            selected: 0,
            editing: None,
            input: None,
            row_editor: None,
            scroll: ScrollHandle::new(),
            escape_interceptor: None,
            focus_handle: None,
        }
    }
}

impl TerminalView {
    pub fn quick_input_is_open(&self) -> bool {
        self.quick_input.open
    }

    pub fn toggle_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_input.open {
            self.close_quick_input(window, cx);
        } else {
            self.open_quick_input(window, cx);
        }
    }

    fn open_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.quick_input.open {
            return;
        }

        let focus_handle = self
            .quick_input
            .focus_handle
            .clone()
            .unwrap_or_else(|| cx.focus_handle());
        self.quick_input.focus_handle = Some(focus_handle);

        self.quick_input.open = true;
        self.quick_input.selected = 0;
        self.quick_input.editing = None;
        self.quick_input.row_editor = None;

        // Re-read on every open so hand edits to the JSON show up without a
        // restart, and so a file that was unparsable earlier is not blocked
        // from being written forever.
        quick_input::store().lock().unwrap().reload();

        self.install_escape_interceptor(cx);
        self.ensure_quick_input_editor(window, cx, String::new());
        cx.notify();
    }

    /// Installs the modal's Esc handler for as long as it is open. Dropping the
    /// subscription removes it again.
    fn install_escape_interceptor(&mut self, cx: &mut Context<Self>) {
        if self.quick_input.escape_interceptor.is_some() {
            return;
        }
        let listener = cx.listener(|this, event: &KeystrokeEvent, window, cx| {
            if !this.quick_input.open || event.keystroke.key != "escape" {
                return;
            }
            // The interceptor is application-wide. When two split panes both
            // have the modal open, only the one owning the keyboard reacts.
            let owns_keyboard = this
                .text_box(cx)
                .map(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
                .unwrap_or(false);
            if !owns_keyboard {
                return;
            }
            // Claim the key before the focused editor sees it; otherwise
            // `editor::Cancel` consumes Esc and the modal stays open.
            cx.stop_propagation();
            this.cancel_quick_input(window, cx);
        });
        let subscription = cx.intercept_keystrokes(listener);
        self.quick_input.escape_interceptor = Some(subscription);
    }

    pub(crate) fn close_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_input.open = false;
        self.quick_input.escape_interceptor = None;
        self.quick_input.editing = None;
        self.quick_input.input = None;
        self.quick_input.row_editor = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn ensure_quick_input_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        text: String,
    ) -> Entity<Editor> {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            if !text.is_empty() {
                editor.set_text(text, window, cx);
            }
            editor
        });
        editor.update(cx, |editor, cx| {
            editor.focus_handle(cx).focus(window, cx);
        });
        self.quick_input.input = Some(editor.clone());
        editor
    }

    /// The text box that should own the keyboard right now.
    fn text_box(&self, _cx: &App) -> Option<Entity<Editor>> {
        if self.quick_input.editing.is_some() {
            self.quick_input.row_editor.clone()
        } else {
            self.quick_input.input.clone()
        }
    }

    fn quick_input_text(&self, cx: &App) -> String {
        self.quick_input
            .input
            .as_ref()
            .map(|editor| editor.read(cx).text(cx))
            .unwrap_or_default()
    }

    fn row_editor_text(&self, cx: &App) -> String {
        self.quick_input
            .row_editor
            .as_ref()
            .map(|editor| editor.read(cx).text(cx))
            .unwrap_or_default()
    }

    /// Enter with text appends a phrase; Enter with an empty box sends the
    /// highlighted phrase. While a row is being edited, Enter saves it.
    pub fn submit_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.quick_input.open {
            return;
        }

        if let Some(index) = self.quick_input.editing {
            let text = self.row_editor_text(cx);
            if text.trim().is_empty() {
                self.cancel_quick_input_edit(window, cx);
                return;
            }
            {
                let mut store = quick_input::store().lock().unwrap();
                store.update(index, text);
            }
            self.cancel_quick_input_edit(window, cx);
            return;
        }

        let text = self.quick_input_text(cx);
        if text.trim().is_empty() {
            self.send_quick_input(self.quick_input.selected, window, cx);
            return;
        }

        {
            let mut store = quick_input::store().lock().unwrap();
            if let Some(index) = store.add(text) {
                self.quick_input.selected = index;
                // The new row does not exist in the layout yet, so scrolling to
                // its index is a no-op. Pinning to the bottom is resolved during
                // prepaint against the refreshed content height instead.
                self.quick_input.scroll.scroll_to_bottom();
            }
        }

        if let Some(editor) = self.quick_input.input.clone() {
            editor.update(cx, |editor, cx| editor.set_text("", window, cx));
        }
        cx.notify();
    }

    pub fn cancel_quick_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_input.editing.is_some() {
            self.cancel_quick_input_edit(window, cx);
        } else {
            self.close_quick_input(window, cx);
        }
    }

    fn send_quick_input(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        // Re-read right before sending: the checkbox may have been toggled in
        // another pane, or the file edited by hand, since the modal opened.
        let (text, direct_execute) = {
            let mut store = quick_input::store().lock().unwrap();
            store.reload();
            (
                store.item(index).map(str::to_string),
                store.direct_execute(),
            )
        };
        let Some(text) = text else {
            return;
        };

        // `Terminal::paste` adds bracketed-paste guards when the CLI asked for
        // them and never appends an Enter, so the phrase lands in the input box
        // without being sent.
        self.terminal.update(cx, |terminal, _cx| terminal.paste(&text));

        // Enter goes out on its own, a beat later: written back-to-back the two
        // chunks usually reach the pty in one read, and the CLI sees the CR as
        // part of the paste payload instead of a key press.
        if direct_execute {
            let terminal = self.terminal.clone();
            cx.spawn(async move |_this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(DELAYED_ENTER_MS))
                    .await;
                terminal.update(cx, |terminal, _cx| terminal.input(b"\r".to_vec()));
            })
            .detach();
        }

        self.close_quick_input(window, cx);
    }

    pub fn move_quick_input_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.quick_input.open || self.quick_input.editing.is_some() {
            return;
        }
        let count = quick_input::store().lock().unwrap().len();
        if count == 0 {
            return;
        }
        let current = self.quick_input.selected.min(count - 1) as isize;
        self.quick_input.selected = (current + delta).clamp(0, count as isize - 1) as usize;
        self.quick_input
            .scroll
            .scroll_to_item(self.quick_input.selected);
        cx.notify();
    }

    fn remove_quick_input(&mut self, index: usize, cx: &mut Context<Self>) {
        {
            let mut store = quick_input::store().lock().unwrap();
            if !store.remove(index) {
                return;
            }
            let count = store.len();
            if self.quick_input.selected >= count && count > 0 {
                self.quick_input.selected = count - 1;
            }
        }
        // Everything after the removed row shifted down by one, so the edit
        // target has to follow; otherwise Enter would save into the wrong
        // phrase.
        match self.quick_input.editing {
            Some(editing) if editing == index => {
                self.quick_input.editing = None;
                self.quick_input.row_editor = None;
            }
            Some(editing) if editing > index => {
                self.quick_input.editing = Some(editing - 1);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Swaps a row for a text box in place, so the edit happens where the user
    /// is looking instead of in the "add" box at the top.
    fn begin_quick_input_edit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = quick_input::store()
            .lock()
            .unwrap()
            .item(index)
            .map(str::to_string)
        else {
            return;
        };

        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(text, window, cx);
            editor
        });
        editor.update(cx, |editor, cx| {
            editor.focus_handle(cx).focus(window, cx);
        });

        self.quick_input.editing = Some(index);
        self.quick_input.selected = index;
        self.quick_input.row_editor = Some(editor);
        cx.notify();
    }

    fn cancel_quick_input_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_input.editing = None;
        self.quick_input.row_editor = None;
        if let Some(editor) = self.quick_input.input.clone() {
            editor.update(cx, |editor, cx| {
                editor.focus_handle(cx).focus(window, cx);
            });
        }
        cx.notify();
    }

    /// Keys the modal owns. Registered in `init` as runtime bindings scoped to
    /// `QuickInput`, which beat the editor's own bindings.
    pub fn handle_quick_input_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.quick_input.open {
            return false;
        }
        match event.keystroke.key.as_str() {
            "up" => {
                self.move_quick_input_selection(-1, cx);
                true
            }
            "down" => {
                self.move_quick_input_selection(1, cx);
                true
            }
            "enter" => {
                self.submit_quick_input(window, cx);
                true
            }
            "escape" => {
                self.cancel_quick_input(window, cx);
                true
            }
            "delete" | "backspace" if self.quick_input.editing.is_none()
                && self.quick_input_text(cx).is_empty() =>
            {
                self.remove_quick_input(self.quick_input.selected, cx);
                true
            }
            _ => false,
        }
    }

    fn activate_quick_input_row(
        &mut self,
        index: usize,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quick_input.selected = index;
        if click_count >= 2 {
            self.send_quick_input(index, window, cx);
        } else {
            cx.notify();
        }
    }

    pub fn render_quick_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.quick_input.open {
            return Vec::new();
        }

        // Nothing in the keymap can help if focus drifted away, so pull it back
        // to whichever text box should own the keyboard.
        if let Some(editor) = self.text_box(cx) {
            let handle = editor.read(cx).focus_handle(cx);
            if !handle.is_focused(window) {
                handle.focus(window, cx);
            }
        }

        let border_color = cx.theme().colors().border;
        let panel_background = cx.theme().colors().element_background;
        let muted_text = cx.theme().colors().text_muted;
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) * MODAL_WIDTH_RATIO).max(MODAL_MIN_WIDTH);
        let max_height = f32::from(viewport.height) * MODAL_MAX_HEIGHT_RATIO;

        // Remembered only so closing the modal can hand focus back to the
        // terminal; the modal itself no longer tracks a focus handle.
        if self.quick_input.focus_handle.is_none() {
            self.quick_input.focus_handle = Some(cx.focus_handle());
        }

        let (items, direct_execute) = {
            let store = quick_input::store().lock().unwrap();
            (store.items().to_vec(), store.direct_execute())
        };
        let visible_rows = items.len().clamp(1, MAX_VISIBLE_ROWS);
        let list_height = if items.is_empty() {
            EMPTY_LIST_HEIGHT
        } else {
            ROW_HEIGHT * visible_rows as f32
        };
        let modal_height = (HEADER_HEIGHT + INPUT_HEIGHT + OPTIONS_HEIGHT + list_height)
            .min(max_height);
        let selected = self.quick_input.selected;
        let editing = self.quick_input.editing;
        let row_editor = self.quick_input.row_editor.clone();

        // Rows hang directly off the scrolling div: `ScrollHandle::scroll_to_item`
        // indexes direct children, so an extra wrapper would map every index to
        // the wrapper's box and make selection scrolling a no-op.
        let mut rows = div()
            .id("quick-input-list")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.quick_input.scroll)
            .px_1()
            .pr(px(10.0));

        if items.is_empty() {
            rows = rows.child(
                div()
                    .flex_shrink(0.)
                    .px_2()
                    .py_3()
                    .text_sm()
                    .text_color(muted_text)
                    .child(localization::localized_str!(
                        "No saved phrases yet. Type one above and press Enter."
                    )),
            );
        } else {
            let view = cx.entity();
            let mut row_elements = Vec::with_capacity(items.len());
            for (index, text) in items.iter().enumerate() {
                row_elements.push(render_quick_input_row(
                    index,
                    text,
                    index == selected,
                    editing == Some(index),
                    row_editor.clone(),
                    view.downgrade(),
                    cx,
                ));
            }
            rows = rows.children(row_elements);
        }

        // `flex_1` (not the estimated height) so a short window shrinks the list
        // instead of clipping it; the estimate only sizes the modal itself.
        let list = div()
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .child(rows)
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Vertical)
                    .tracked_scroll_handle(&self.quick_input.scroll),
                window,
                cx,
            );

        let top_editor: AnyElement = self
            .quick_input
            .input
            .clone()
            .map(|editor| editor.into_any_element())
            .unwrap_or_else(|| div().into_any_element());

        vec![
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(panel_background.opacity(0.55))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // Reaching the backdrop means the click landed outside
                        // the panel, which is the familiar way to dismiss it.
                        cx.stop_propagation();
                        this.close_quick_input(window, cx);
                    }),
                )
                .child(
                    div()
                        .id("quick-input-modal")
                        // Clicks inside the panel must not bubble to the
                        // backdrop, or every click would dismiss the modal.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(width))
                        .h(px(modal_height))
                        .flex()
                        .flex_col()
                        .rounded_lg()
                        .border_1()
                        .border_color(border_color)
                        .bg(panel_background)
                        .shadow_lg()
                        .key_context("QuickInput")
                        .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            // Only swallow the keys the modal owns; everything
                            // else has to reach the text box.
                            if this.handle_quick_input_key(event, window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(border_color)
                                .child(div().text_sm().child(localization::localized_str!("Quick Input")))
                                .child(
                                    ButtonLike::new("quick-input-close")
                                        .style(ButtonStyle::Transparent)
                                        .width(px(24.0))
                                        .height(px(24.0).into())
                                        .tooltip(|_window, cx| {
                                            cx.new(|_cx| {
                                                Tooltip::new(localization::localized_str!("Close (Esc)"))
                                            })
                                            .into()
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            cx.stop_propagation();
                                            this.close_quick_input(window, cx);
                                        }))
                                        .child(
                                            Icon::new(IconName::Close)
                                                .size(IconSize::XSmall)
                                                .color(Color::Muted),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(border_color)
                                .child(div().h(px(28.0)).child(top_editor)),
                        )
                        .child(list)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(border_color)
                                .child(
                                    Checkbox::new(
                                        "quick-input-direct-execute",
                                        if direct_execute {
                                            ToggleState::Selected
                                        } else {
                                            ToggleState::Unselected
                                        },
                                    )
                                    .label(localization::localized_str!("Run immediately"))
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(
                                        |_this, state: &ToggleState, _window, cx| {
                                            quick_input::store()
                                                .lock()
                                                .unwrap()
                                                .set_direct_execute(
                                                    *state == ToggleState::Selected,
                                                );
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        ]
    }

}

/// One phrase row. A free function so the row builder can run with a plain
/// `&mut App`; callbacks go back through the weak view handle.
fn render_quick_input_row(
    index: usize,
    text: &str,
    is_selected: bool,
    is_editing: bool,
    row_editor: Option<Entity<Editor>>,
    view: WeakEntity<TerminalView>,
    cx: &mut App,
) -> AnyElement {
    let selected_background = cx.theme().colors().element_selected;
    let hover_background = cx.theme().colors().element_hover;
    let focus_border = cx.theme().colors().border_focused;
    let text_color = cx.theme().colors().text;

    // Settings rows mark the keyboard-reachable control with a focus border;
    // the list uses the same cue so selection reads the same everywhere.
    let mut row = div()
        .id(("quick-input-row", index))
        .flex()
        .flex_shrink(0.)
        .items_center()
        .gap_1()
        .px_2()
        .py_1p5()
        .min_h(px(ROW_HEIGHT))
        .rounded_sm()
        .border_1()
        .border_color(if is_selected {
            focus_border
        } else {
            focus_border.opacity(0.0)
        })
        .cursor_pointer()
        .on_click({
            let view = view.clone();
            move |event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                view.update(cx, |this, cx| {
                    this.activate_quick_input_row(index, event.click_count(), window, cx)
                })
                .ok();
            }
        });

    if is_selected {
        row = row.bg(selected_background);
    } else {
        row = row.hover(move |style| style.bg(hover_background));
    }

    if is_editing && let Some(editor) = row_editor {
        return row
            .child(div().flex_1().h(px(24.0)).child(editor))
            .child(quick_input_delete_button(index, view))
            .into_any_element();
    }

    // Long phrases wrap instead of being clipped: the list height is a scroll
    // viewport, so a taller row just scrolls.
    // `min_w(0)` lets the flex item shrink below its text's single-line width;
    // without it the row keeps one line and the tail is clipped.
    let label: AnyElement = div()
        .flex_1()
        .min_w(px(0.0))
        .whitespace_normal()
        .text_sm()
        .text_color(text_color)
        .child(text.to_string())
        .into_any_element();

    row.child(label)
        .child(quick_input_edit_button(index, view.clone()))
        .child(quick_input_delete_button(index, view))
        .into_any_element()
}

fn quick_input_edit_button(index: usize, view: WeakEntity<TerminalView>) -> AnyElement {
    ButtonLike::new(("quick-input-edit", index))
        .style(ButtonStyle::Transparent)
        .width(px(24.0))
        .height(px(24.0).into())
        .tooltip(|_window, cx| {
            cx.new(|_cx| Tooltip::new(localization::localized_str!("Edit")))
                .into()
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            view.update(cx, |this, cx| {
                this.begin_quick_input_edit(index, window, cx)
            })
            .ok();
        })
        .child(Icon::new(IconName::Pencil).size(IconSize::XSmall))
        .into_any_element()
}

fn quick_input_delete_button(index: usize, view: WeakEntity<TerminalView>) -> AnyElement {
    ButtonLike::new(("quick-input-delete", index))
        .style(ButtonStyle::Transparent)
        .width(px(24.0))
        .height(px(24.0).into())
        .tooltip(|_window, cx| {
            cx.new(|_cx| Tooltip::new(localization::localized_str!("Delete")))
                .into()
        })
        .on_click(move |_, _window, cx| {
            cx.stop_propagation();
            view.update(cx, |this, cx| this.remove_quick_input(index, cx))
                .ok();
        })
        .child(Icon::new(IconName::Trash).size(IconSize::XSmall))
        .into_any_element()
}
