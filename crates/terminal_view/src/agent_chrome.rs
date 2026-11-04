//! OHOS Agent terminal chrome: the host-drawn scrollbar, right-click menu, and
//! inline pane rename, driven by the agent server JSON API.

use super::*;

/// Hooks for the OHOS Agent terminal: the agent server owns the right-click
/// menu and pane scroll state, while HiCodeer draws them. `agent_panel`
/// registers these so this crate does not depend on the agent client.
#[derive(Clone, Copy)]
pub struct AgentChromeHooks {
    pub context_menu: fn(u16, u16) -> Result<(String, Vec<PaneContextMenuEntry>), String>,
    pub invoke: fn(&str, &str) -> Result<PaneContextMenuOutcome, String>,
    pub scroll: fn() -> Result<PaneScrollMetrics, String>,
    pub set_scroll: fn(&str, u64) -> Result<(), String>,
    pub rename_pane: fn(&str, &str) -> Result<(), String>,
}

impl gpui::Global for AgentChromeHooks {}

/// Scroll metrics for the agent server's focused pane, used by the host-drawn
/// Agent scrollbar. Public and not cfg-gated so the agent client can return it
/// on every target; only OHOS terminals draw the scrollbar from it.
#[derive(Clone, Debug)]
pub struct PaneScrollMetrics {
    pub pane_id: String,
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

/// One item of the agent server's pane right-click menu. The server owns the
/// list; the host only renders it.
#[derive(Clone, Debug)]
pub struct PaneContextMenuEntry {
    pub id: String,
    pub label: String,
}

/// What the host must do after running one menu item. `Rename` asks the host to
/// open its inline editor; everything else is finished on the server.
#[derive(Clone, Debug)]
pub enum PaneContextMenuOutcome {
    Done,
    Rename { current_label: Option<String> },
}

/// State of a drag on the host-drawn Agent terminal scrollbar thumb.
#[cfg(target_env = "ohos")]
#[derive(Clone)]
pub(super) struct AgentScrollDrag {
    pane_id: String,
    start_y: f32,
    start_offset: u64,
    max: u64,
    thumb_h: f32,
    track_h: f32,
}

/// Serializes `pane.scroll` writes: the drag only ever stores the newest
/// pending offset, and a single worker task sends them one at a time, so an
/// older request can never land after a newer one. The task lives in the view,
/// so dropping the terminal cancels it.
#[cfg(target_env = "ohos")]
pub(super) struct AgentScrollWriter {
    pending: postage::watch::Sender<Option<(String, u64)>>,
    _task: Task<()>,
}

/// How often the poll refreshes the Agent scrollbar's server metrics.
#[cfg(target_env = "ohos")]
const AGENT_SCROLL_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Geometry of the host-drawn Agent terminal scrollbar, mirroring the Regular
/// style in `ui::components::scrollbar` (thumb centred inside a track, inset
/// from the right edge) but with a wider thumb and a slightly larger inset than
/// Regular's 9px/4px: at right_0 the pointer landed in the system window-resize
/// hitbox instead of on the scrollbar.
#[cfg(target_env = "ohos")]
const AGENT_SCROLLBAR_THUMB_WIDTH: Pixels = gpui::px(12.);
#[cfg(target_env = "ohos")]
const AGENT_SCROLLBAR_RIGHT_INSET: Pixels = gpui::px(6.);
#[cfg(target_env = "ohos")]
pub(super) const AGENT_SCROLLBAR_TRACK_WIDTH: Pixels = gpui::px(24.);
/// Same floor as `ui::components::scrollbar`'s MINIMUM_THUMB_SIZE: a thumb
/// scaled only by viewport/content would be a dot when the scrollback is long.
#[cfg(target_env = "ohos")]
const AGENT_SCROLLBAR_MIN_THUMB: Pixels = gpui::px(25.);

impl TerminalView {
    /// Whether the host, not this terminal's own scrollback, owns the chrome.
    #[cfg(target_env = "ohos")]
    pub(super) fn agent_chrome_active(&self) -> bool {
        self.is_agent_terminal()
    }

    #[cfg(not(target_env = "ohos"))]
    pub(super) fn agent_chrome_active(&self) -> bool {
        false
    }

    /// The host-drawn Agent terminal chrome: a scrollbar thumb driven by the
    /// agent server's pane metrics, and the inline pane-rename input.
    #[cfg(target_env = "ohos")]
    pub(super) fn render_agent_chrome(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.is_agent_terminal() {
            return Vec::new();
        }
        let thumb_color = cx.theme().colors().scrollbar_thumb_background;
        let hover_color = cx.theme().colors().scrollbar_thumb_hover_background;
        let active_color = cx.theme().colors().scrollbar_thumb_active_background;
        let track_color = cx.theme().colors().editor_background;
        let surface_color = cx.theme().colors().surface_background;
        let mut elements = Vec::new();
        if let Some(metrics) = self.agent_scroll.clone() {
            if metrics.max_offset_from_bottom > 0 {
                let bounds = self.terminal.read(cx).last_content().terminal_bounds;
                let track_h = f32::from(bounds.bounds.size.height).max(1.0);
                let content_size =
                    (metrics.max_offset_from_bottom + metrics.viewport_rows).max(1) as f32;
                // Same rule as ui::scrollbars::thumb_ranges: the thumb is
                // viewport / content of the track, never smaller than the
                // 25px floor. A pure ratio is a dot when scrollback is long.
                let thumb_h = (track_h * (metrics.viewport_rows as f32 / content_size))
                    .max(f32::from(AGENT_SCROLLBAR_MIN_THUMB))
                    .min(track_h);
                let max_top = (track_h - thumb_h).max(0.0);
                let scrolled = metrics
                    .max_offset_from_bottom
                    .saturating_sub(metrics.offset_from_bottom) as f32;
                let top =
                    (scrolled / metrics.max_offset_from_bottom as f32 * max_top).min(max_top);
                // A track behind the thumb, like the terminal's own scrollbar.
                // The thumb is inset from the right edge; drawing it at
                // right_0 put the pointer into the window-resize hitbox.
                elements.push(
                    div()
                        .id("agent-scrollbar-track")
                        .absolute()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .w(AGENT_SCROLLBAR_TRACK_WIDTH)
                        .bg(track_color)
                        .into_any_element(),
                );
                let dragging = self.agent_scroll_drag.is_some();
                let pane_id = metrics.pane_id.clone();
                let drag_start_offset = metrics.offset_from_bottom;
                let drag_max = metrics.max_offset_from_bottom;
                elements.push(
                    div()
                        .id("agent-scrollbar-thumb")
                        .absolute()
                        .right(AGENT_SCROLLBAR_RIGHT_INSET)
                        .top(gpui::px(top))
                        .w(AGENT_SCROLLBAR_THUMB_WIDTH)
                        .h(gpui::px(thumb_h))
                        .rounded_md()
                        .bg(if dragging { active_color } else { thumb_color })
                        .hover(move |style| style.bg(hover_color))
                        .cursor(CursorStyle::Arrow)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener({
                                let pane_id = pane_id.clone();
                                move |this, event: &MouseDownEvent, _, cx| {
                                    this.agent_scroll_drag = Some(AgentScrollDrag {
                                        pane_id: pane_id.clone(),
                                        start_y: f32::from(event.position.y),
                                        start_offset: drag_start_offset,
                                        max: drag_max,
                                        thumb_h,
                                        track_h,
                                    });
                                    log::info!("agent scrollbar drag started on {pane_id}");
                                    cx.notify();
                                }
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
        if let Some((_, editor)) = &self.agent_pane_rename {
            let entity = cx.entity().downgrade();
            elements.push(
                div()
                    .id("agent-pane-rename")
                    .absolute()
                    .top_1()
                    .left_0()
                    .right_0()
                    .h_8()
                    .flex()
                    .items_center()
                    .px_2()
                    .bg(surface_color)
                    .child(editor.clone())
                    .on_action({
                        let entity = entity.clone();
                        move |_: &menu::Confirm, window, cx| {
                            let _ = entity.update(cx, |view, cx| {
                                view.finish_agent_pane_rename(true, window, cx)
                            });
                        }
                    })
                    .on_action(move |_: &menu::Cancel, window, cx| {
                        let _ = entity.update(cx, |view, cx| {
                            view.finish_agent_pane_rename(false, window, cx)
                        });
                    })
                    .into_any_element(),
            );
        }
        elements
    }

    #[cfg(not(target_env = "ohos"))]
    pub(super) fn render_agent_chrome(&self, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    /// Applies a thumb drag to the agent server's scroll position. Called from
    /// the root view's mouse-move handler, so the drag keeps working while the
    /// pointer is anywhere inside the terminal view, not just over the thumb.
    #[cfg(target_env = "ohos")]
    pub(super) fn drag_agent_scroll(
        &mut self,
        y: f32,
        pressed_button: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.agent_scroll_drag.clone() else {
            return;
        };
        // The button was released outside this view (over the panel, another
        // window, or the desktop), so no mouse-up arrives here. End the drag
        // instead of leaving it latched forever.
        if pressed_button != Some(MouseButton::Left) {
            self.end_agent_scroll_drag(cx);
            return;
        }
        let max_top = (drag.track_h - drag.thumb_h).max(1.0);
        let dy = y - drag.start_y;
        // Dragging down moves the thumb towards the newest output, so
        // offset_from_bottom decreases.
        let offset = (drag.start_offset as f32 - dy / max_top * drag.max as f32)
            .clamp(0.0, drag.max as f32) as u64;
        // Move the thumb immediately; the poll will confirm the server state.
        if let Some(scroll) = self.agent_scroll.as_mut() {
            scroll.offset_from_bottom = offset;
        }
        // The writer sends these one at a time; storing the offset here
        // replaces any older pending value, so the server always ends on the
        // newest one and a burst of moves cannot reorder writes.
        self.queue_agent_scroll(&drag.pane_id, offset);
        cx.notify();
    }

    /// Stores the newest offset for the serial writer. Older pending values are
    /// dropped, so a burst of mouse-move events collapses into at most one
    /// in-flight request plus the newest one.
    #[cfg(target_env = "ohos")]
    fn queue_agent_scroll(&mut self, pane_id: &str, offset: u64) {
        let Some(writer) = self.agent_scroll_writer.as_mut() else {
            return;
        };
        *writer.pending.borrow_mut() = Some((pane_id.to_string(), offset));
    }

    /// Ends a drag and queues its final offset on the drag's own pane. The
    /// poll may have replaced `agent_scroll`'s pane while the pointer moved, so
    /// the write must not follow the polled pane id.
    #[cfg(target_env = "ohos")]
    pub(super) fn end_agent_scroll_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.agent_scroll_drag.take() else {
            return;
        };
        let offset = self.agent_scroll.as_ref().map(|m| m.offset_from_bottom);
        if let Some(offset) = offset {
            self.queue_agent_scroll(&drag.pane_id, offset);
        }
        log::info!("agent scrollbar drag ended on {}", drag.pane_id);
        cx.notify();
    }

    /// Starts the serial `pane.scroll` writer once per view. It drains only the
    /// newest pending offset, so an older write can never land after a newer
    /// one, and it stops when the view is dropped.
    #[cfg(target_env = "ohos")]
    pub(super) fn start_agent_scroll_writer(&mut self, cx: &mut Context<Self>) {
        if self.agent_scroll_writer.is_some() {
            return;
        }
        let Some(hooks) = cx.try_global::<agent_chrome::AgentChromeHooks>().copied() else {
            return;
        };
        use futures::StreamExt;

        let (pending, mut receiver) = postage::watch::channel::<Option<(String, u64)>>();
        let task = cx.spawn(async move |_, cx| {
            // The sender closing ends the loop, which also covers the view
            // being dropped. `watch` keeps only the newest value, so a burst of
            // drags collapses into one in-flight request plus the latest one.
            while receiver.next().await.is_some() {
                let Some((pane_id, offset)) = receiver.borrow().clone() else {
                    continue;
                };
                let log_pane = pane_id.clone();
                let result = cx
                    .background_spawn(async move { (hooks.set_scroll)(&pane_id, offset) })
                    .await;
                if let Err(err) = result {
                    log::warn!("agent scroll write failed for {log_pane} at {offset}: {err}");
                }
            }
        });
        self.agent_scroll_writer = Some(AgentScrollWriter {
            pending,
            _task: task,
        });
    }

    /// The Agent terminal's scrollbar is driven by the agent server, not by
    /// this terminal's scrollback, so its thumb needs the server's metrics.
    #[cfg(target_env = "ohos")]
    pub(super) fn start_agent_scroll_poll(&mut self, cx: &mut Context<Self>) {
        if self.agent_scroll_poll_task.is_some() {
            return;
        }
        let Some(hooks) = cx.try_global::<agent_chrome::AgentChromeHooks>().copied() else {
            return;
        };
        let view = cx.entity().downgrade();
        let task = cx.spawn(async move |_, cx| loop {
            let metrics = cx.background_spawn(async move { (hooks.scroll)() }).await;
            let alive = view
                .update(cx, |view, cx| {
                    match metrics {
                        Ok(mut metrics) => {
                            let drag = view.agent_scroll_drag.clone();
                            let offset = match drag {
                                // Still dragging this pane: keep the local
                                // offset so the thumb does not jump back under
                                // the pointer every poll.
                                Some(drag) if drag.pane_id == metrics.pane_id => view
                                    .agent_scroll
                                    .as_ref()
                                    .map(|current| current.offset_from_bottom)
                                    .unwrap_or(metrics.offset_from_bottom),
                                // Focus moved to another pane mid-drag. Stop
                                // writing to the old pane and follow the server.
                                Some(_) => {
                                    view.end_agent_scroll_drag(cx);
                                    metrics.offset_from_bottom
                                }
                                None => metrics.offset_from_bottom,
                            };
                            metrics.offset_from_bottom = offset;
                            view.agent_scroll = Some(metrics);
                        }
                        Err(err) => {
                            if view.agent_scroll_drag.is_none()
                                && view.agent_scroll.take().is_some()
                            {
                                log::warn!("agent scroll poll failed: {err}");
                            }
                        }
                    }
                    cx.notify();
                })
                .is_ok();
            // The view was dropped: stop polling instead of leaking a task.
            if !alive {
                break;
            }
            cx.background_executor()
                .timer(AGENT_SCROLL_POLL_INTERVAL)
                .await;
        });
        self.agent_scroll_poll_task = Some(task);
    }

    /// Right-click in the Agent terminal: the item list comes from the agent
    /// server, but the menu is HiCodeer's own ContextMenu.
    #[cfg(target_env = "ohos")]
    pub(super) fn deploy_agent_context_menu(
        &mut self,
        position: GpuiPoint<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hooks) = cx.try_global::<agent_chrome::AgentChromeHooks>().copied() else {
            return;
        };
        let bounds = self.terminal.read(cx).last_content().terminal_bounds;
        let cell_width = f32::from(bounds.cell_width).max(1.0);
        let line_height = f32::from(bounds.line_height).max(1.0);
        let row = ((f32::from(position.y) - f32::from(bounds.bounds.origin.y)) / line_height)
            .max(0.0)
            .min(u16::MAX as f32) as u16;
        let col = ((f32::from(position.x) - f32::from(bounds.bounds.origin.x)) / cell_width)
            .max(0.0)
            .min(u16::MAX as f32) as u16;
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity().downgrade();
        // A synchronous loopback here would freeze the UI for as long as the
        // bridge timeouts, so read the item list off the main thread.
        window
            .spawn(cx, async move |cx| {
                let result = cx
                    .background_spawn(async move { (hooks.context_menu)(row, col) })
                    .await;
                let (pane_id, items) = match result {
                    Ok(value) => value,
                    Err(err) => {
                        log::error!("agent pane context menu: {err}");
                        return;
                    }
                };
                if items.is_empty() {
                    return;
                }
                let _ = cx.update(|window, app| {
                    let _ = entity.update(app, |view, cx| {
                        view.show_agent_context_menu(
                            pane_id,
                            items,
                            position,
                            focus_handle,
                            window,
                            cx,
                        );
                    });
                });
            })
            .detach();
    }

    /// Builds HiCodeer's own context menu once the agent server has returned
    /// the item list. Menu actions are dispatched asynchronously for the same
    /// reason the list is fetched asynchronously.
    #[cfg(target_env = "ohos")]
    fn show_agent_context_menu(
        &mut self,
        pane_id: String,
        items: Vec<PaneContextMenuEntry>,
        position: GpuiPoint<Pixels>,
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "agent pane context menu opened for {pane_id} with {} items",
            items.len()
        );
        let entity = cx.entity().downgrade();
        let menu_focus = focus_handle.clone();
        let context_menu = ContextMenu::build(window, cx, |menu, _, _| {
            let mut menu = menu.context(menu_focus);
            for entry in items {
                let action_id = entry.id;
                let pane_id = pane_id.clone();
                let entity = entity.clone();
                // The server labels are the lookup keys; a missing translation
                // falls back to the server's English text.
                let label = localization::lookup(&entry.label)
                    .map(str::to_string)
                    .unwrap_or(entry.label);
                menu = menu.entry(label, None, move |window, cx| {
                    let Some(hooks) =
                        cx.try_global::<agent_chrome::AgentChromeHooks>().copied()
                    else {
                        return;
                    };
                    let pane_id = pane_id.clone();
                    let action_id = action_id.clone();
                    let entity = entity.clone();
                    let rename_pane_id = pane_id.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let result = cx
                                .background_spawn(async move {
                                    (hooks.invoke)(&pane_id, &action_id)
                                })
                                .await;
                            match result {
                                Ok(PaneContextMenuOutcome::Rename { current_label }) => {
                                    let _ = cx.update(|window, app| {
                                        let _ = entity.update(app, |view, cx| {
                                            view.begin_agent_pane_rename(
                                                rename_pane_id.clone(),
                                                current_label.unwrap_or_default(),
                                                window,
                                                cx,
                                            );
                                        });
                                    });
                                }
                                Ok(PaneContextMenuOutcome::Done) => {}
                                Err(err) => {
                                    log::error!("agent pane context menu: {err}")
                                }
                            }
                        })
                        .detach();
                });
            }
            menu
        });
        window.focus(&context_menu.focus_handle(cx), cx);
        let subscription = cx.subscribe_in(
            &context_menu,
            window,
            |this, _, _: &DismissEvent, window, cx| {
                if this.context_menu.as_ref().is_some_and(|menu| {
                    menu.0.focus_handle(cx).contains_focused(window, cx)
                }) {
                    cx.focus_self(window);
                }
                this.context_menu.take();
                cx.notify();
            },
        );
        self.context_menu = Some((context_menu, position, subscription));
    }

    /// Opens HiCodeer's own inline editor for the `Rename pane` menu item.
    #[cfg(target_env = "ohos")]
    fn begin_agent_pane_rename(
        &mut self,
        pane_id: String,
        current_label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!("agent pane rename opened for {pane_id}");
        let editor = cx.new(|cx| Editor::single_line(window, cx));
        editor.update(cx, |editor, cx| {
            editor.set_text(current_label, window, cx);
            editor.select_all(&SelectAll, window, cx);
            editor.focus_handle(cx).focus(window, cx);
        });
        self.agent_pane_rename = Some((pane_id, editor));
        cx.notify();
    }

    #[cfg(target_env = "ohos")]
    fn finish_agent_pane_rename(
        &mut self,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane_id, editor)) = self.agent_pane_rename.take() else {
            return;
        };
        log::info!("agent pane rename finished for {pane_id} (save={save})");
        if save {
            let label = editor.read(cx).text(cx).trim().to_string();
            if !label.is_empty() {
                if let Some(hooks) = cx.try_global::<agent_chrome::AgentChromeHooks>().copied() {
                    // Fire-and-forget: one loopback request that the UI does not
                    // wait for, so it must not block the main thread. The
                    // request is already sent once spawned; only its error is
                    // worth logging, so the handle is deliberately dropped.
                    cx.background_spawn(async move {
                        if let Err(err) = (hooks.rename_pane)(&pane_id, &label) {
                            log::error!("rename agent pane: {err}");
                        }
                    })
                    .detach();
                }
            }
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
    }
}
