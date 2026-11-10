// OHOS-only: the Talk to Agent dialog.
//
// Mirrors the git commit modal: a top summary line, a read-only selection on
// the left, an editable message on the right, and a footer with two target
// pickers plus a send button. This crate only reads the Agent panel's own
// snapshot; it never talks to herdr directly.

use editor::{Editor, EditorElement, EditorMode, EditorStyle, MultiBuffer, SizingBehavior};
use gpui::*;
use language::Buffer;
use settings::Settings;
use theme::ActiveTheme;
use theme_settings::ThemeSettings;
use ui::{
    ButtonLike, ButtonSize, ElevationIndex, Icon, IconName, IconSize, KeyBinding, KeybindingHint,
    List, ListItem, ListItemSpacing, PopoverMenu, PopoverMenuHandle, Tooltip, prelude::*,
};
use workspace::ModalView;

use crate::AgentPanel;
use crate::client;
use crate::snapshot::SpaceEntry;

/// The selection the editor collected for this dialog.
#[derive(Clone)]
pub(crate) struct TalkToAgentContext {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub selected_text: String,
    pub language_name: String,
    pub is_before_change: bool,
}

/// One space and the agents the picker can offer.
#[derive(Clone)]
struct TalkSpaceChoice {
    id: SharedString,
    label: SharedString,
    selected: bool,
    agents: Vec<TalkAgentChoice>,
}

#[derive(Clone)]
struct TalkAgentChoice {
    pane_id: SharedString,
    name: SharedString,
    selected: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TalkPickerKind {
    Space,
    Agent,
}

#[derive(Clone)]
struct TalkPickerItem {
    id: SharedString,
    label: SharedString,
    selected: bool,
}

pub(crate) struct TalkToAgentModal {
    sizes: ModalSizes,
    context: TalkToAgentContext,
    message_editor: Entity<Editor>,
    preview_editor: Entity<Editor>,
    spaces: Vec<TalkSpaceChoice>,
    selected_space: Option<usize>,
    selected_agent: Option<SharedString>,
    space_picker: PopoverMenuHandle<TalkTargetPicker>,
    agent_picker: PopoverMenuHandle<TalkTargetPicker>,
    error: Option<SharedString>,
    sending: bool,
}

impl TalkToAgentModal {
    pub(crate) fn new(
        panel: Entity<AgentPanel>,
        context: TalkToAgentContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sizes = modal_sizes(window, cx);
        let spaces = collect_spaces(&panel, cx);
        let (selected_space, selected_agent) = initial_selection(&spaces);

        let preview_buffer = cx.new(|cx| Buffer::local(context.selected_text.clone(), cx));
        let preview_multibuffer = cx.new(|cx| MultiBuffer::singleton(preview_buffer, cx));
        let preview_editor = cx.new(|cx| {
            let mut editor = Editor::new(
                dialog_editor_mode(),
                preview_multibuffer,
                None,
                window,
                cx,
            );
            // Read-only but selectable, so the user can copy the reference.
            editor.set_read_only(true);
            editor.set_show_gutter(false, cx);
            editor.set_show_line_numbers(false, cx);
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::None, cx);
            editor.set_use_modal_editing(true);
            editor.set_show_vertical_scrollbar(true, cx);
            editor.set_show_horizontal_scrollbar(true, cx);
            editor
        });

        let message_buffer = cx.new(|cx| Buffer::local("", cx));
        let message_multibuffer = cx.new(|cx| MultiBuffer::singleton(message_buffer, cx));
        let message_editor = cx.new(|cx| {
            let mut editor = Editor::new(
                dialog_editor_mode(),
                message_multibuffer,
                None,
                window,
                cx,
            );
            editor.set_use_autoclose(false);
            editor.set_show_gutter(false, cx);
            editor.set_show_line_numbers(false, cx);
            editor.set_use_modal_editing(true);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            // Wrap at the editor width; only vertical scrolling is needed.
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            editor.set_show_vertical_scrollbar(true, cx);
            editor.set_show_horizontal_scrollbar(false, cx);
            editor.set_placeholder_text(
                &localization::localized_str!("Enter commit message"),
                window,
                cx,
            );
            editor
        });

        // Both boxes show their scrollbars whenever content overflows.
        preview_editor.update(cx, |editor, cx| {
            editor.set_force_scrollbar_visibility(true, cx);
            editor.set_vertical_scroll_margin(0, cx);
        });
        message_editor.update(cx, |editor, cx| {
            editor.set_force_scrollbar_visibility(true, cx);
            // No scroll margin, so the last visible line is usable and both
            // boxes show the same number of lines.
            editor.set_vertical_scroll_margin(0, cx);
        });

        // Deliberately no focus-out dismissal: closing a picker briefly moves
        // focus away from the message editor, which used to dismiss the whole
        // dialog as soon as a space or agent was chosen. Clicking the backdrop
        // and Escape still close the dialog.
        Self {
            sizes,
            context,
            message_editor,
            preview_editor,
            spaces,
            selected_space,
            selected_agent,
            space_picker: PopoverMenuHandle::default(),
            agent_picker: PopoverMenuHandle::default(),
            error: None,
            sending: false,
        }
    }

    fn selected_space_choice(&self) -> Option<&TalkSpaceChoice> {
        self.selected_space.and_then(|index| self.spaces.get(index))
    }

    fn selected_agent_choice(&self) -> Option<&TalkAgentChoice> {
        let pane_id = self.selected_agent.as_ref()?;
        self.selected_space_choice()?
            .agents
            .iter()
            .find(|agent| &agent.pane_id == pane_id)
    }

    fn space_items(&self) -> Vec<TalkPickerItem> {
        self.spaces
            .iter()
            .enumerate()
            .map(|(index, space)| TalkPickerItem {
                id: space.id.clone(),
                label: space.label.clone(),
                selected: self.selected_space == Some(index),
            })
            .collect()
    }

    fn agent_items(&self) -> Vec<TalkPickerItem> {
        let selected = self.selected_agent.clone();
        self.selected_space_choice()
            .map(|space| {
                space
                    .agents
                    .iter()
                    .map(|agent| TalkPickerItem {
                        id: agent.pane_id.clone(),
                        label: agent.name.clone(),
                        selected: selected.as_ref() == Some(&agent.pane_id),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn apply_pick(
        &mut self,
        kind: TalkPickerKind,
        id: SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            TalkPickerKind::Space => {
                if let Some(index) = self.spaces.iter().position(|space| space.id == id) {
                    self.selected_space = Some(index);
                    self.selected_agent = self.spaces[index]
                        .agents
                        .first()
                        .map(|agent| agent.pane_id.clone());
                }
                self.space_picker.hide(cx);
            }
            TalkPickerKind::Agent => {
                self.selected_agent = Some(id);
                self.agent_picker.hide(cx);
            }
        }
        cx.notify();
    }

    fn send(
        &mut self,
        _: &zed_actions::agent::SendTalkToAgentMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit(window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }

        let typed = self.message_editor.read(cx).text(cx);
        let typed = typed.trim();
        if typed.is_empty() {
            self.error = Some(localization::localized_str!("Enter a message to send").into());
            cx.notify();
            return;
        }

        let Some(agent) = self.selected_agent_choice() else {
            self.error = Some(localization::localized_str!("Select an agent to continue").into());
            cx.notify();
            return;
        };

        let message = compose_message(&self.context, typed);
        let target = agent.pane_id.to_string();
        self.error = None;
        self.sending = true;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { client::prompt_agent(&target, &message) })
                .await;

            this.update(cx, |modal, cx| match result {
                Ok(()) => cx.emit(DismissEvent),
                Err(error) => {
                    log::warn!("talk_to_agent: send failed: {error}");
                    modal.sending = false;
                    modal.error = Some(send_error_message(&error).into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_header(&self, _cx: &mut Context<Self>) -> AnyElement {
        let lines = if self.context.start_line == self.context.end_line {
            localization::format_message(
                "Line {line}",
                &[("line", self.context.start_line.to_string())],
            )
        } else {
            localization::format_message(
                "Line {start} to Line {end}",
                &[
                    ("start", self.context.start_line.to_string()),
                    ("end", self.context.end_line.to_string()),
                ],
            )
        };

        v_flex()
            .w_full()
            .flex_none()
            .gap_0p5()
            .child(
                Label::new(self.context.file_path.clone())
                    .size(LabelSize::Small)
                    .color(Color::Default),
            )
            .child(Label::new(lines).size(LabelSize::Small).color(Color::Muted))
            .into_any_element()
    }

    fn render_preview(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let style = ui_text_editor_style(window, cx);
        div()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(EditorElement::new(&self.preview_editor, style))
            .into_any_element()
    }

    fn render_message(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let style = ui_text_editor_style(window, cx);
        div()
            .h_full()
            .w_full()
            .overflow_hidden()
            .child(EditorElement::new(&self.message_editor, style))
            .into_any_element()
    }

    fn render_space_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = self
            .selected_space_choice()
            .map(|space| space.label.clone())
            .unwrap_or_else(|| localization::localized_str!("Select a space").into());

        let button = Button::new("talk-to-agent-space-picker", label)
            .label_size(LabelSize::Small)
            .start_icon(
                Icon::new(IconName::Sparkle)
                    .size(IconSize::Small)
                    .color(Color::Placeholder),
            );

        let modal = cx.entity().downgrade();
        let items = self.space_items();
        PopoverMenu::new("talk-to-agent-space-menu")
            .with_handle(self.space_picker.clone())
            .trigger_with_tooltip(button, |_, cx| {
                Tooltip::simple(localization::localized_str!("Switch space"), cx)
            })
            .menu(move |window, cx| {
                let modal = modal.clone();
                let items = items.clone();
                Some(cx.new(|cx| {
                    TalkTargetPicker::new(TalkPickerKind::Space, items, modal, window, cx)
                }))
            })
            .anchor(Anchor::BottomLeft)
            .offset(Point {
                x: px(0.0),
                y: px(-2.0),
            })
            .into_any_element()
    }

    fn render_agent_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = self
            .selected_agent_choice()
            .map(|agent| agent.name.clone())
            .unwrap_or_else(|| localization::localized_str!("Select an agent").into());

        let button = Button::new("talk-to-agent-agent-picker", label)
            .label_size(LabelSize::Small)
            .start_icon(
                Icon::new(IconName::Sparkle)
                    .size(IconSize::Small)
                    .color(Color::Placeholder),
            );

        let modal = cx.entity().downgrade();
        let items = self.agent_items();
        PopoverMenu::new("talk-to-agent-agent-menu")
            .with_handle(self.agent_picker.clone())
            .trigger_with_tooltip(button, |_, cx| {
                Tooltip::simple(localization::localized_str!("Switch agent"), cx)
            })
            .menu(move |window, cx| {
                let modal = modal.clone();
                let items = items.clone();
                Some(cx.new(|cx| {
                    TalkTargetPicker::new(TalkPickerKind::Agent, items, modal, window, cx)
                }))
            })
            .anchor(Anchor::BottomLeft)
            .offset(Point {
                x: px(0.0),
                y: px(-2.0),
            })
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let focus_handle = self.message_editor.focus_handle(cx);
        // Build the hint for the key this dialog actually binds. Asking the
        // keymap can return its other binding (Ctrl-Escape), and the dialog
        // always closes on Escape here.
        let escape_binding = KeyBinding::from_keystrokes(
            vec![KeybindingKeystroke::from_keystroke(Keystroke {
                modifiers: Modifiers::default(),
                key: "escape".to_string(),
                key_char: None,
            })]
            .into(),
            false,
        );
        let close_hint = KeybindingHint::new(
            escape_binding,
            cx.theme().colors().editor_background,
        )
        .suffix(localization::localized_str!("Cancel"));

        h_flex()
            .w_full()
            .h(px(FOOTER_HEIGHT))
            .flex_none()
            .gap_1()
            .justify_between()
            .child(
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .flex_shrink_1()
                    .overflow_x_hidden()
                    .child(self.render_space_picker(cx))
                    .child(self.render_agent_picker(cx)),
            )
            .child(
                h_flex()
                    .gap_4()
                    .child(div().child(close_hint).into_any_element())
                    .child(
                        ButtonLike::new("talk-to-agent-send")
                            .layer(ElevationIndex::ModalSurface)
                            .size(ButtonSize::Compact)
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Icon::new(IconName::Send)
                                            .size(IconSize::XSmall)
                                            .color(Color::Default),
                                    )
                                    .child(
                                        Label::new(localization::localized_str!("Send"))
                                            .size(LabelSize::Small),
                                    ),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.submit(window, cx);
                            }))
                            .tooltip(move |_, cx| {
                                Tooltip::for_action_in(
                                    localization::localized_str!("Send"),
                                    &zed_actions::agent::SendTalkToAgentMessage,
                                    &focus_handle,
                                    cx,
                                )
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for TalkToAgentModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.message_editor.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for TalkToAgentModal {}

impl ModalView for TalkToAgentModal {}

impl Render for TalkToAgentModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sizes = self.sizes;
        let container_padding = px(CONTAINER_PADDING);
        let editor_radius = px(MODAL_BORDER_RADIUS - CONTAINER_PADDING / 2.0);

        v_flex()
            .id("talk-to-agent-modal")
            .key_context("TalkToAgentModal")
            .on_action(cx.listener(Self::send))
            .on_action(cx.listener(|_this, _: &menu::Cancel, _, cx| {
                cx.emit(DismissEvent);
                cx.notify();
            }))
            .w(sizes.width)
            .h(sizes.height)
            .p(container_padding)
            .elevation_3(cx)
            .overflow_hidden()
            .flex_none()
            .relative()
            .bg(cx.theme().colors().elevated_surface_background)
            .rounded(px(MODAL_BORDER_RADIUS))
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(
                v_flex()
                    .id("talk-to-agent-content")
                    .p_2()
                    .size_full()
                    .gap_2()
                    .justify_between()
                    .rounded(editor_radius)
                    .overflow_hidden()
                    .bg(cx.theme().colors().editor_background)
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(self.render_header(cx))
                    .child(
                        h_flex()
                            .w_full()
                            .flex_1()
                            .min_h(px(0.0))
                            .gap_2()
                            .child(
                                div()
                                    .h_full()
                                    .flex_none()
                                    .w(preview_width(sizes))
                                    .rounded(px(EDITOR_BORDER_RADIUS))
                                    .border_1()
                                    .border_color(cx.theme().colors().border_variant)
                                    .overflow_hidden()
                                    .child(self.render_preview(window, cx)),
                            )
                            .child(
                                div()
                                    .h_full()
                                    .flex_1()
                                    .min_w_0()
                                    .rounded(px(EDITOR_BORDER_RADIUS))
                                    .border_1()
                                    .border_color(cx.theme().colors().border_variant)
                                    .overflow_hidden()
                                    .child(self.render_message(window, cx)),
                            ),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            h_flex()
                                .w_full()
                                .flex_none()
                                .gap_1()
                                .child(
                                    Icon::new(IconName::Warning)
                                        .size(IconSize::XSmall)
                                        .color(Color::Error),
                                )
                                .child(
                                    Label::new(error).size(LabelSize::Small).color(Color::Error),
                                ),
                        )
                    })
                    .child(self.render_footer(cx)),
            )
    }
}

const CONTAINER_PADDING: f32 = 5.0;
const INNER_PADDING: f32 = 8.0;
const GAP: f32 = 8.0;
const MODAL_BORDER_RADIUS: f32 = 12.0;
const EDITOR_BORDER_RADIUS: f32 = 6.0;
const FOOTER_HEIGHT: f32 = 24.0;
/// The git commit modal's editor height and its top/bottom padding.
const EDITOR_HEIGHT: f32 = 300.0;
const EDITOR_PADDING_TOP: f32 = 3.0;
const EDITOR_PADDING_BOTTOM: f32 = 6.0;
/// The git commit modal reserves this many extra lines so the editor does not
/// overflow its container.
const EDITOR_EXTRA_LINES: f32 = 1.5;
const UI_LINE_HEIGHT_SCALE: f32 = 1.4;
/// Same width as the git commit modal: 50 characters plus its padding.
const PREFERRED_CHAR_WIDTH: f32 = 50.0;
const FALLBACK_MODAL_WIDTH: f32 = 460.0;

fn dialog_editor_mode() -> EditorMode {
    EditorMode::Full {
        scale_ui_elements_with_buffer_font_size: false,
        show_active_line_background: false,
        sizing_behavior: SizingBehavior::Default,
    }
}

#[derive(Clone, Copy)]
struct ModalSizes {
    width: Pixels,
    height: Pixels,
}

/// Reproduces the git commit modal's footprint exactly, computed once when the
/// dialog is built, which is when the git commit modal computes its own.
fn modal_sizes(window: &Window, cx: &App) -> ModalSizes {
    // Sized from the UI font, which is also what both text boxes use, so the
    // dialog and the terminal quick input end up the same size.
    let settings = ThemeSettings::get_global(cx);
    let font = settings.ui_font.clone();
    let font_size = settings.ui_font_size(cx);
    let font_id = window.text_system().resolve_font(&font);
    let width = match window.text_system().em_width(font_id, font_size) {
        Ok(em_width) => em_width * PREFERRED_CHAR_WIDTH + px(CONTAINER_PADDING * 2.0),
        Err(_) => px(FALLBACK_MODAL_WIDTH),
    };

    let line_height = font_size * UI_LINE_HEIGHT_SCALE;
    let editor_area = px(EDITOR_HEIGHT + EDITOR_PADDING_TOP + EDITOR_PADDING_BOTTOM)
        + line_height * EDITOR_EXTRA_LINES;
    let height = px(CONTAINER_PADDING * 2.0)
        + px(INNER_PADDING * 2.0)
        + editor_area
        + px(GAP)
        + px(FOOTER_HEIGHT);

    ModalSizes { width, height }
}

/// The left pane takes three fifths of the editor row, matching the design.
fn preview_width(sizes: ModalSizes) -> Pixels {
    let inner = sizes.width
        - px(CONTAINER_PADDING * 2.0)
        - px(INNER_PADDING * 2.0)
        - px(GAP);
    inner * 0.6
}

fn ui_text_editor_style(_window: &Window, cx: &App) -> EditorStyle {
    let settings = ThemeSettings::get_global(cx);
    // Same font and size as the editor buffers.
    let font = settings.buffer_font.clone();
    let font_size = settings.buffer_font_size(cx);

    EditorStyle {
        background: cx.theme().colors().editor_background,
        local_player: cx.theme().players().local(),
        text: TextStyle {
            color: cx.theme().colors().text,
            font_family: font.family.clone(),
            font_fallbacks: font.fallbacks.clone(),
            font_features: font.features.clone(),
            font_size: AbsoluteLength::from(font_size),
            font_weight: font.weight,
            line_height: (font_size * settings.buffer_line_height.value()).into(),
            ..Default::default()
        },
        // Without a width the editor never draws a scrollbar at all.
        scrollbar_width: ui::EDITOR_SCROLLBAR_WIDTH,
        syntax: cx.theme().syntax().clone(),
        ..Default::default()
    }
}
fn collect_spaces(panel: &Entity<AgentPanel>, cx: &App) -> Vec<TalkSpaceChoice> {
    panel
        .read(cx)
        .spaces
        .iter()
        .map(|space| TalkSpaceChoice {
            id: SharedString::from(space.id.clone()),
            label: SharedString::from(space.label.clone()),
            selected: space.focused,
            agents: space
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    SpaceEntry::Agent(agent) => Some(TalkAgentChoice {
                        pane_id: SharedString::from(agent.pane_id.clone()),
                        name: SharedString::from(client::display_name(&agent.pane_id, &agent.name).to_owned()),
                        selected: agent.focused,
                    }),
                    SpaceEntry::Terminal(_) => None,
                })
                .collect(),
        })
        .collect()
}

fn initial_selection(spaces: &[TalkSpaceChoice]) -> (Option<usize>, Option<SharedString>) {
    let Some((index, space)) = spaces
        .iter()
        .enumerate()
        .find(|(_, space)| space.selected)
        .or_else(|| spaces.first().map(|space| (0, space)))
    else {
        return (None, None);
    };

    let agent = space
        .agents
        .iter()
        .find(|agent| agent.selected)
        .or_else(|| space.agents.first());

    (Some(index), agent.map(|agent| agent.pane_id.clone()))
}

fn send_error_message(error: &client::ClientError) -> String {
    match error {
        client::ClientError::NotReady => {
            localization::localized_str!("The agent panel is not ready yet").to_owned()
        }
        client::ClientError::Failed(_) => {
            localization::format_message("Failed to send: {error}", &[("error", error.to_string())])
        }
    }
}

struct TalkTargetPicker {
    kind: TalkPickerKind,
    items: Vec<TalkPickerItem>,
    modal: WeakEntity<TalkToAgentModal>,
    focus_handle: FocusHandle,
}

impl TalkTargetPicker {
    fn new(
        kind: TalkPickerKind,
        items: Vec<TalkPickerItem>,
        modal: WeakEntity<TalkToAgentModal>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            kind,
            items,
            modal,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for TalkTargetPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for TalkTargetPicker {}

impl Render for TalkTargetPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        v_flex()
            .id("talk-to-agent-picker")
            // Fit the longest label, with a floor and a ceiling.
            .min_w(rems(7.))
            .max_w(rems(30.))
            .max_h(rems(20.))
            .overflow_y_scroll()
            // The git branch picker sits on the popover surface; without this
            // the list rows float on a transparent background.
            .bg(cx.theme().colors().elevated_surface_background)
            .border_1()
            .border_color(cx.theme().colors().border)
            .rounded(px(6.0))
            .shadow_md()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .child(
                List::new()
                    .empty_message(localization::localized_str!("No items"))
                    .children(self.items.iter().map(|item| {
                        let modal = self.modal.clone();
                        let id = item.id.clone();
                        ListItem::new(SharedString::from(format!(
                            "talk-to-agent-pick-{}-{}",
                            match kind {
                                TalkPickerKind::Space => "space",
                                TalkPickerKind::Agent => "agent",
                            },
                            item.id
                        )))
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .toggle_state(item.selected)
                        .child(
                            Label::new(item.label.clone())
                                .size(LabelSize::Small)
                                .truncate(),
                        )
                        .on_click(move |_, window, cx| {
                            modal
                                .update(cx, |modal, cx| {
                                    modal.apply_pick(kind, id.clone(), window, cx)
                                })
                                .ok();
                        })
                    })),
            )
    }
}

/// Builds the message the agent receives: the user's request first, then the
/// file reference, the line range and the selected text in a fenced block.
fn compose_message(context: &TalkToAgentContext, typed: &str) -> String {
    let mut message = String::with_capacity(context.selected_text.len() + typed.len() + 128);
    message.push_str(typed);
    message.push_str("\n\n");

    let mut path_line =
        localization::format_message("File: {path}", &[("path", context.file_path.clone())]);
    if context.is_before_change {
        path_line.push(' ');
        path_line.push_str(localization::localized_str!("(before changes)"));
    }
    message.push_str(&path_line);
    message.push('\n');

    let line_range = if context.start_line == context.end_line {
        localization::format_message("Line {line}", &[("line", context.start_line.to_string())])
    } else {
        localization::format_message(
            "Line {start} to Line {end}",
            &[
                ("start", context.start_line.to_string()),
                ("end", context.end_line.to_string()),
            ],
        )
    };
    message.push_str(&line_range);
    message.push('\n');

    message.push_str("\u{0060}\u{0060}\u{0060}\u{0060}");
    message.push_str(&context.language_name);
    message.push('\n');
    message.push_str(&context.selected_text);
    if !context.selected_text.ends_with('\n') {
        message.push('\n');
    }
    message.push_str("\u{0060}\u{0060}\u{0060}\u{0060}");
    message
}

#[cfg(test)]
mod tests {
    // Import the items explicitly: a glob import from the parent would also
    // pull in gpui's `test` attribute macro, which shadows std's `#[test]`.
    use super::{TalkToAgentContext, compose_message};

    fn context(
        start_line: u32,
        end_line: u32,
        selected_text: &str,
        before: bool,
    ) -> TalkToAgentContext {
        TalkToAgentContext {
            file_path: "/repo/src/lib.rs".to_string(),
            start_line,
            end_line,
            selected_text: selected_text.to_string(),
            language_name: "rust".to_string(),
            is_before_change: before,
        }
    }

    #[test]
    fn composes_a_single_line_selection() {
        let message = compose_message(&context(12, 12, "let x = 1;", false), "explain this");

        assert!(message.starts_with("explain this\n\n"));
        assert!(message.contains("File: /repo/src/lib.rs\n"));
        assert!(message.contains("Line 12\n"));
        assert!(message.contains("\u{0060}\u{0060}\u{0060}\u{0060}rust\nlet x = 1;\n"));
        assert!(message.ends_with("\u{0060}\u{0060}\u{0060}\u{0060}"));
    }

    #[test]
    fn composes_a_multi_line_selection_and_marks_the_pre_change_side() {
        let message = compose_message(&context(12, 30, "fn a() {}\nfn b() {}", true), "refactor");

        assert!(message.contains("Line 12 to Line 30\n"));
        assert!(message.contains("/repo/src/lib.rs (before changes)"));
    }

    #[test]
    fn composes_a_caret_line_without_selected_text() {
        let message = compose_message(&context(7, 7, "", false), "explain this line");

        assert!(message.contains("Line 7\n"));
        assert!(message.contains("\u{0060}\u{0060}\u{0060}\u{0060}rust\n\n\u{0060}\u{0060}\u{0060}\u{0060}"));
    }

    #[test]
    fn does_not_duplicate_a_trailing_newline_in_the_selection() {
        let message = compose_message(&context(1, 1, "one\n", false), "check");

        assert!(message.contains("rust\none\n\u{0060}\u{0060}\u{0060}\u{0060}"));
    }
}