// ===== [OHOS PORT BEGIN] Agent Container settings page =====
//! The Agent Container settings page.
//!
//! It gathers what used to live on the Panels page's agent block (sound,
//! notifications, sleep, integrations and history) and the IM channel controls.
//! All of it configures the container the agents run in rather than where a
//! single panel sits.
// ===== [OHOS PORT END] =====

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use agent_panel::herdr_settings::{self, ToastDelivery};
use im_channel::binding::Platform;
use im_channel::manager::{self, ScanStatus};
use im_channel::platforms::{self, BindingRoute, CredentialValues, PlatformSpec};
use im_channel::registration::BOT_NAME;
use gpui::{Entity, Window};
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Disableable as _, Divider, DividerColor, Icon,
    IconSize, Label, LabelSize, Switch, ToggleState,
};
use ui_input::InputField;

use crate::components::EnumVariantDropdown;

use super::*;

/// Width of a credential field, so the form matches the width the other pages
/// give a right-hand control.
const FORM_WIDTH_PX: f32 = 320.;
/// Side of every drawn QR code, in pixels.
///
/// The symbol is scaled to this side, so channels whose payloads need
/// different module counts still show codes of the same size.
const QR_SIDE_PX: f32 = 320.;
/// Quiet zone around the symbol, in modules.
const QR_QUIET_ZONE_MODULES: f32 = 4.;
/// How often the page re-renders while a scan is in flight, so the code, the
/// configured state and a failure all appear without the user reloading.
const SCAN_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
/// How often the connection status is re-read while a channel is selected. The
/// long connection comes up and drops on its own thread, so the row would
/// otherwise keep showing the state from the last interaction.
const CONNECTION_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Height of one grid cell, matching the medium button height the base
/// keymap selector uses.
const AGENT_CELL_HEIGHT_PX: f32 = 28.;
/// One character of inset between a cell's border and its content.
const AGENT_CELL_INSET_PX: f32 = 8.;
/// Number of columns in an agent grid. Cells share the row width equally,
/// so the table's top and left borders end exactly where the cells end
/// instead of running past the last column.
const AGENT_COLUMNS: u16 = 4;
/// The first two fields of every spec are the credentials its device flow
/// returns; anything after them is filled in on the configured card.
const DEVICE_FLOW_FIELDS: usize = 2;
/// Channels the selector offers, in the order the page lists them. Adding an
/// IM channel means adding a label and a choice here.
static CHANNEL_LABELS: [&str; 4] = ["Off", "QQ", "DingTalk", "Feishu"];
static CHANNEL_CHOICES: [Option<Platform>; 4] = [
    None,
    Some(Platform::Qq),
    Some(Platform::DingTalk),
    Some(Platform::Feishu),
];

fn sound_title() -> &'static str {
    localization::localized_str!("Sound Alerts")
}
fn sound_description() -> &'static str {
    localization::localized_str!("Play sounds when agents change state in background")
}
fn toast_title() -> &'static str {
    localization::localized_str!("Notification Popups")
}
fn toast_description() -> &'static str {
    localization::localized_str!("Show agent notifications outside the terminal")
}
fn prevent_idle_sleep_title() -> &'static str {
    localization::localized_str!("Prevent Sleep")
}
fn prevent_idle_sleep_description() -> &'static str {
    localization::localized_str!(
        "While the app is in the foreground and an agent is running a task, the system stays awake"
    )
}
fn auto_install_title() -> &'static str {
    localization::localized_str!("Agent Status Notifications")
}
fn auto_install_description() -> &'static str {
    localization::localized_str!(
        "Automatically install task status notification scripts into agents"
    )
}
fn show_history_title() -> &'static str {
    localization::localized_str!("Show History")
}
fn show_history_description() -> &'static str {
    localization::localized_str!(
        "List past sessions from the agents below. Turning this off only hides them"
    )
}

/// The controls this module contributes: the container's switches, then the IM
/// section. They live on a page of their own because they configure what runs
/// inside the agent container on every surface, not one panel.
pub(crate) fn items() -> Vec<SettingsPageItem> {
    vec![
        SettingsPageItem::SectionHeader(localization::localized_str!("Agent Notification")),
        SettingsPageItem::Custom(CustomItem {
            render: render_container_page,
            keywords: &[
                "sound",
                "notification",
                "toast",
                "integration",
                "agent",
                "history",
                "sleep",
                "awake",
            ],
        }),
        SettingsPageItem::SectionHeader(localization::localized_str!("IM Assistant")),
        SettingsPageItem::Custom(CustomItem {
            render: render_im_page,
            keywords: &[
                "im", "feishu", "lark", "dingtalk", "qq", "notify", "chat", "scan",
            ],
        }),
        SettingsPageItem::SectionHeader(localization::localized_str!("Agents list")),
        SettingsPageItem::Custom(CustomItem {
            render: render_agents_list_page,
            keywords: &[
                "agent", "agents", "compatibility", "history", "integration", "list",
            ],
        }),
    ]
}

/// The one place the settings page lists agents.
///
/// The same rows used to appear twice, once under the notification switches and
/// once under history. They are the same information, so they now live here.
fn render_agents_list_page(
    _settings_window: &SettingsWindow,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let settings = herdr_settings::snapshot(cx);
    let mut content = v_flex()
        .id("agent-list-settings")
        .w_full()
        .child(block(
            Label::new(localization::localized_str!(
                "Agents with the best compatibility"
            ))
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element(),
        ));

    if settings.integrations.is_empty() {
        content = content.child(block(
            Label::new(if settings.integrations_checked {
                localization::localized_str!("No integration targets are available.")
            } else {
                localization::localized_str!("Not checked yet.")
            })
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element(),
        ));
    } else {
        content = content.child(block(scannable_grid(cx.theme().colors().border)));
    }

    content.into_any_element()
}

fn render_container_page(
    _settings_window: &SettingsWindow,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let settings = herdr_settings::snapshot(cx);

    // The panel is what usually performs the first check; opening this page
    // on its own would otherwise sit on "Not checked yet" until the switch
    // is toggled.
    if !settings.integrations_checked
        && !cx
            .global::<agent_panel::herdr_settings::AgentIntegrations>()
            .checking
    {
        herdr_settings::refresh_integrations(cx, settings.auto_install_integrations);
    }

    let sound = Switch::new(
        "agent-terminal-sound",
        if settings.sound_enabled {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        },
    )
    .tab_index(0_isize)
    .aria_label(sound_title())
    .aria_description(sound_description())
    .on_click({
        move |state, _window, cx| {
            let enabled = *state == ToggleState::Selected;
            if let Err(error) = herdr_settings::set_sound_enabled(enabled, cx) {
                log::warn!("agent container settings: {error}");
            }
            cx.refresh_windows();
        }
    })
    .into_any_element();

    // One switch rather than the delivery modes: this build presents agent
    // notifications itself, so "on" always means the system route and the
    // terminal never draws a second copy of the same event.
    let toast = Switch::new(
        "agent-terminal-toast",
        if settings.toast == ToastDelivery::Off {
            ToggleState::Unselected
        } else {
            ToggleState::Selected
        },
    )
    .tab_index(0_isize)
    .aria_label(toast_title())
    .aria_description(toast_description())
    .on_click({
        move |state, _window, cx| {
            let enabled = *state == ToggleState::Selected;
            let delivery = if enabled {
                ToastDelivery::System
            } else {
                ToastDelivery::Off
            };
            if let Err(error) = herdr_settings::set_toast_delivery(delivery, cx) {
                log::warn!("agent container settings: {error}");
            }
            cx.refresh_windows();
        }
    })
    .into_any_element();

    let prevent_idle_sleep = Switch::new(
        "agent-terminal-prevent-idle-sleep",
        if settings.prevent_idle_sleep {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        },
    )
    .tab_index(0_isize)
    .aria_label(prevent_idle_sleep_title())
    .aria_description(prevent_idle_sleep_description())
    .on_click(move |state, window, cx| {
        let enabled = *state == ToggleState::Selected;
        if let Err(error) = update_settings_file(
            SettingsUiFile::User,
            None,
            window,
            cx,
            move |settings, _| {
                settings
                    .agent_panel
                    .get_or_insert_with(Default::default)
                    .prevent_idle_sleep = Some(enabled);
            },
        ) {
            log::warn!("agent container settings: cannot save prevent idle sleep: {error}");
        }
    })
    .into_any_element();

    let auto_install = Switch::new(
        "agent-terminal-auto-install",
        if settings.auto_install_integrations {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        },
    )
    .tab_index(0_isize)
    .aria_label(auto_install_title())
    .aria_description(auto_install_description())
    .on_click(move |state, window, cx| {
        let enabled = *state == ToggleState::Selected;
        if let Err(error) = update_settings_file(
            SettingsUiFile::User,
            None,
            window,
            cx,
            move |settings, _| {
                settings
                    .agent_panel
                    .get_or_insert_with(Default::default)
                    .auto_install_integrations = Some(enabled);
            },
        ) {
            log::warn!("agent container settings: cannot save auto install: {error}");
        }
        // The switch is also the moment to look for targets, which is what
        // the separate "check now" button used to be for.
        herdr_settings::refresh_integrations(cx, enabled);
        cx.refresh_windows();
    })
    .into_any_element();

    let show_history = Switch::new(
        "agent-terminal-show-history",
        if settings.show_history {
            ToggleState::Selected
        } else {
            ToggleState::Unselected
        },
    )
    .tab_index(0_isize)
    .aria_label(show_history_title())
    .aria_description(show_history_description())
    .on_click(move |state, window, cx| {
        let enabled = *state == ToggleState::Selected;
        if let Err(error) = update_settings_file(
            SettingsUiFile::User,
            None,
            window,
            cx,
            move |settings, _| {
                settings
                    .agent_panel
                    .get_or_insert_with(Default::default)
                    .show_history = Some(enabled);
            },
        ) {
            log::warn!("agent container settings: cannot save show history: {error}");
        }
    })
    .into_any_element();

    let mut content = v_flex()
        .id("agent-container-settings")
        .w_full()
        .child(block(setting_row(
            sound_title(),
            sound_description(),
            sound,
        )))
        .child(divider())
        .child(block(setting_row(
            toast_title(),
            toast_description(),
            toast,
        )))
        .child(divider())
        .child(block(setting_row(
            prevent_idle_sleep_title(),
            prevent_idle_sleep_description(),
            prevent_idle_sleep,
        )))
        .child(divider())
        .child(block(setting_row(
            auto_install_title(),
            auto_install_description(),
            auto_install,
        )))
        .child(divider());

    content = content
        .child(divider())
        .child(block(setting_row(
            show_history_title(),
            show_history_description(),
            show_history,
        )));

    content.into_any_element()
}

/// Which entry of the selector matches `channel`.
fn channel_index(channel: Option<Platform>) -> usize {
    CHANNEL_CHOICES
        .iter()
        .position(|choice| *choice == channel)
        .unwrap_or(0)
}

/// The state shown under the selector. Only the selected channel is described
/// here; every channel's own configuration follows below.
fn connection_text(status: &manager::ImStatus) -> &'static str {
    if !status.configured {
        localization::localized_str!("Not configured")
    } else if status.connected {
        localization::localized_str!("Connected")
    } else {
        localization::localized_str!("Not connected")
    }
}

fn render_im_page(
    _settings_window: &SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let status = match manager::status() {
        Ok(status) => status,
        Err(error) => {
            log::warn!("agent container settings: cannot read the IM configuration: {error}");
            manager::ImStatus::default()
        }
    };

    // A scan progresses on its own thread, so the page has to be told when the
    // QR code is ready and when the credentials arrive. One timer per render is
    // enough: the refresh re-enters this function, which arms the next timer.
    let scanning = manager::scan_state().is_some_and(|scan| {
        matches!(
            scan.status,
            ScanStatus::Starting | ScanStatus::Waiting { .. }
        )
    });
    let interval = if scanning {
        SCAN_REFRESH_INTERVAL
    } else if status.channel.is_some() {
        CONNECTION_REFRESH_INTERVAL
    } else {
        Duration::ZERO
    };
    // One refresh task at a time: without the flag every extra frame would
    // start another timer, and the chains would keep multiplying.
    let scheduled = window.use_keyed_state("im-status-refresh", cx, |_, _| {
        Rc::new(Cell::new(false))
    });
    if !interval.is_zero() && !scheduled.read(cx).get() {
        scheduled.read(cx).set(true);
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor().timer(interval).await;
            if let Err(error) = cx.update(|_, cx| {
                scheduled.read(cx).set(false);
                cx.refresh_windows();
            }) {
                log::debug!("agent container settings: cannot refresh the IM state: {error:?}");
            }
        })
        .detach();
    }

    let dropdown = EnumVariantDropdown::new_indexed(
        "im-channel",
        channel_index(status.channel),
        &CHANNEL_LABELS,
        move |index, _, cx| {
            let channel = CHANNEL_CHOICES[index];
            if let Err(error) = manager::set_channel(channel) {
                log::error!("agent container settings: cannot select the IM channel: {error}");
            }
            cx.refresh_windows();
        },
    )
    .title_case(false)
    .aria_label(localization::localized_str!("IM Channel"))
        .aria_description(localization::localized_str!(
            "Which IM channel receives agent notifications. Only the selected channel connects"
        ))
    .tab_index(0);

    let border = cx.theme().colors().border_variant;
    let fill = cx.theme().colors().element_background.opacity(0.2);

    let mut content = v_flex()
        .id("agent-container-im-settings")
        .w_full()
        .child(block(setting_row(
            localization::localized_str!("IM Channel"),
            localization::localized_str!(
                "Which IM channel receives agent notifications. Only the selected channel connects"
            ),
            dropdown.into_any_element(),
        )))
        .child(divider());

    // The connection state is a row of its own, beside the selector. Each
    // channel then keeps its heading on the same level, with its configuration
    // in the card below it.
    if status.channel.is_some() {
        let color = if status.connected {
            Color::Success
        } else {
            Color::Muted
        };
        content = content
            .child(block(setting_row(
                localization::localized_str!("Connection Status"),
                localization::localized_str!("Whether the selected IM channel is connected"),
                Label::new(connection_text(&status))
                    .size(LabelSize::Small)
                    .color(color)
                    .into_any_element(),
            )))
            .child(divider());
    }

    // Each channel keeps its own heading on the selector's level, with its
    // configuration nested under it.
    for platform in [Platform::Qq, Platform::DingTalk, Platform::Feishu] {
        let (header, body) = channel_section(platform, &status, window, cx);
        content = content.child(block(header));
        if let Some(body) = body {
            content = content.child(sub_card(body, border, fill));
        }
    }

    content.into_any_element()
}

/// One channel's heading row and the card beneath it.
///
/// The heading sits on the same level as the channel selector, so the three
/// channels read as siblings; everything that configures one channel lives in
/// the card under its heading.
fn channel_section(
    platform: Platform,
    status: &manager::ImStatus,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> (AnyElement, Option<AnyElement>) {
    let spec = platforms::spec(platform);
    let stored = status
        .channels
        .iter()
        .find(|channel| channel.platform == platform);

    // One input per declared field. The key includes the platform, so each
    // channel keeps its own text instead of sharing it with its neighbours.
    let inputs: Vec<Entity<InputField>> = spec
        .fields
        .iter()
        .map(|field| {
            window.use_keyed_state(
                gpui::ElementId::Name(
                    format!("im-credential-{}-{}", platform.key(), field.key).into(),
                ),
                cx,
                |window, cx| InputField::new(window, cx, field.placeholder),
            )
        })
        .collect();

    if let Some(stored) = stored {
        // The heading reports configuration only, so all three channels read
        // alike. Whether the bot has a chat to post to is said in the card.
        let state = if stored.configured {
            // Every channel needs one inbound message before it knows where to
            // post, so the heading says so instead of only reporting a state.
            localization::localized_str!(
                "Configured. Send a message from app in order to activate the IM channel"
            )
        } else {
            localization::localized_str!("Not configured")
        };
        let header = setting_row(
            platform_name(platform),
            state,
            outlined_button(
                unbind_button_id(platform),
                localization::localized_str!("Unbind"),
            )
            .on_click(move |_, _, cx| {
                if let Err(error) = manager::unbind(platform) {
                    log::error!(
                        "agent container settings: cannot clear the {} configuration: {error}",
                        platform.key()
                    );
                }
                cx.refresh_windows();
            })
            .into_any_element(),
        );
        let mut body: Vec<AnyElement> = Vec::new();
        // Fields the device flow could not return are filled in here, one row
        // each, instead of rebinding and losing the learned conversation.
        // Optional fields stay hidden: the provider derives their value, so
        // asking for them would be busywork.
        for field in spec.fields.iter().skip(DEVICE_FLOW_FIELDS) {
            if field.optional || stored.provided.iter().any(|key| key == field.key) {
                continue;
            }
            let inputs_by_field = inputs.clone();
            let field_key = field.key;
            let slot = spec
                .fields
                .iter()
                .position(|candidate| candidate.key == field.key)
                .unwrap_or(0);
            let input = inputs
                .get(slot)
                .cloned()
                .map(Entity::into_any_element)
                .unwrap_or_else(|| div().into_any_element());
            body.push(setting_row(
                field.label,
                field.description,
                h_flex()
                    .gap_2()
                    .child(div().w(px(FORM_WIDTH_PX)).child(input))
                    .child(
                        outlined_button(
                            format!("im-save-{}-{}", platform.key(), field.key),
                            localization::localized_str!("Save"),
                        )
                            .on_click(move |_, _, cx| {
                                let value = inputs_by_field
                                    .get(slot)
                                    .map(|input| input.read(cx).text(cx).trim().to_string())
                                    .unwrap_or_default();
                                if value.is_empty() {
                                    log::warn!("agent container settings: {field_key} is empty");
                                    return;
                                }
                                if let Err(error) = manager::set_field(platform, field_key, value)
                                {
                                    log::error!(
                                        "agent container settings: cannot save {field_key}: {error}"
                                    );
                                }
                                cx.refresh_windows();
                            })
                            .into_any_element(),
                    )
                    .into_any_element(),
            ));
        }
        return (header, wrap_card_body(body));
    }

    let scan = manager::scan_state().filter(|scan| scan.platform == platform);
    let scan_status = scan.as_ref().map(|scan| &scan.status);
    let (description, control) = match scan_status {
        None => (
            localization::localized_str!("Not configured").to_string(),
            bind_route_button(spec),
        ),
        Some(ScanStatus::Starting) => (
            localization::localized_str!("Requesting a QR code...").to_string(),
            outlined_button("im-cancel-scan", localization::localized_str!("Cancel"))
                .on_click(|_, _, cx| {
                    manager::cancel_scan();
                    cx.refresh_windows();
                })
                .into_any_element(),
        ),
        Some(ScanStatus::Waiting { .. }) => (
            localization::localized_str!("Waiting for the scan").to_string(),
            outlined_button("im-cancel-scan", localization::localized_str!("Cancel"))
                .on_click(|_, _, cx| {
                    manager::cancel_scan();
                    cx.refresh_windows();
                })
                .into_any_element(),
        ),
        Some(ScanStatus::Failed(message)) => (
            message.clone(),
            outlined_button("im-retry-scan", localization::localized_str!("Retry"))
                .on_click(move |_, _, cx| {
                    manager::start_scan(platform);
                    cx.refresh_windows();
                })
                .into_any_element(),
        ),
    };
    let header = setting_row(platform_name(platform), description, control);

    let mut body: Vec<AnyElement> = Vec::new();
    if let Some(ScanStatus::Waiting { content: payload }) = scan_status {
        body.push(
            v_flex()
                .gap_2()
                .items_start()
                .child(qr_view(payload))
                .child(
                    Label::new(localization::format_message(
                        "Scan with {platform} and confirm. The bot will be named {bot_name}.",
                        &[
                            ("platform", platform_name(platform).to_string()),
                            ("bot_name", BOT_NAME.to_string()),
                        ],
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element(),
        );
    }

    if spec.is_device_flow() {
        // Feishu and DingTalk hand over their credentials through the scan, so
        // there is nothing to type in; the card only ever carries the QR code.
        return (header, wrap_card_body(body));
    }

    for (field, input) in spec.fields.iter().zip(inputs.iter()) {
        body.push(setting_row(
            field.label,
            field.description,
            div()
                .w(px(FORM_WIDTH_PX))
                .flex_none()
                .child(input.clone())
                .into_any_element(),
        ));
    }

    let inputs_for_bind = inputs.clone();
    // Binding with a half-filled form can only fail; leave the button disabled
    // so the click cannot silently do nothing.
    let can_bind = credentials_complete(spec, &typed_values(&inputs, spec, cx));
    body.push(setting_row(
        localization::localized_str!("Bind"),
        localization::localized_str!("Bind this channel with the credentials above"),
        outlined_button(bind_button_id(platform), localization::localized_str!("Bind"))
            .disabled(!can_bind)
            .on_click(move |_, _, cx| {
                let values = typed_values(&inputs_for_bind, spec, cx);
                if !credentials_complete(spec, &values) {
                    log::warn!(
                        "agent container settings: {} credentials are incomplete",
                        spec.display_name
                    );
                    return;
                }
                if let Err(error) = manager::bind(platform, values) {
                    log::error!(
                        "agent container settings: cannot configure {}: {error}",
                        spec.display_name
                    );
                }
                cx.refresh_windows();
            })
            .into_any_element(),
    ));

    (header, wrap_card_body(body))
}

/// Packs the rows of one channel into its card body. A configured channel
/// whose fields are all present has nothing left to configure, so it returns
/// no card instead of drawing an empty one.
fn wrap_card_body(rows: Vec<AnyElement>) -> Option<AnyElement> {
    if rows.is_empty() {
        return None;
    }
    Some(
        v_flex()
            .w_full()
            .gap_3()
            .children(rows)
            .into_any_element(),
    )
}

/// One nested card, matching the sub-fields a dynamic setting (such as Theme
/// Mode) reveals: a dashed border over a faint fill, so the rows inside read as
/// subordinate to the heading above them.
fn sub_card(child: AnyElement, border: gpui::Hsla, fill: gpui::Hsla) -> AnyElement {
    v_flex()
        .mx_8()
        .mb_4()
        .p_4()
        .border_t_1()
        .border_b_1()
        .border_dashed()
        .border_color(border)
        .bg(fill)
        .child(child)
        .into_any_element()
}

/// The name a channel is shown under. Feishu and DingTalk have local names, so
/// they come from the catalog; QQ is written the same way everywhere.
fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Feishu => localization::localized_str!("Feishu"),
        Platform::DingTalk => localization::localized_str!("DingTalk"),
        Platform::Qq => "QQ",
    }
}

/// The button that starts a scan, or opens the page that creates the bot.
fn bind_route_button(spec: &'static PlatformSpec) -> AnyElement {
    match &spec.route {
        BindingRoute::DeviceFlow(_) => outlined_button(
            scan_button_id(spec.platform),
            localization::format_message(
                "Scan with {platform} to create the bot",
                &[("platform", platform_name(spec.platform).to_string())],
            ),
        )
        .on_click(move |_, _, cx| {
            manager::start_scan(spec.platform);
            cx.refresh_windows();
        })
        .into_any_element(),
        BindingRoute::ExternalPage(url) => outlined_button(
            external_button_id(spec.platform),
            localization::format_message(
                "Open the {platform} bot page",
                &[("platform", platform_name(spec.platform).to_string())],
            ),
        )
        .on_click(move |_, _, cx| cx.open_url(url))
        .into_any_element(),
    }
}

fn scan_button_id(platform: Platform) -> &'static str {
    match platform {
        Platform::Feishu => "im-scan-feishu",
        Platform::DingTalk => "im-scan-dingtalk",
        Platform::Qq => "im-scan-qq",
    }
}

fn external_button_id(platform: Platform) -> &'static str {
    match platform {
        Platform::Feishu => "im-open-feishu",
        Platform::DingTalk => "im-open-dingtalk",
        Platform::Qq => "im-open-qq",
    }
}

fn unbind_button_id(platform: Platform) -> &'static str {
    match platform {
        Platform::Feishu => "im-unbind-feishu",
        Platform::DingTalk => "im-unbind-dingtalk",
        Platform::Qq => "im-unbind-qq",
    }
}

fn bind_button_id(platform: Platform) -> &'static str {
    match platform {
        Platform::Feishu => "im-bind-feishu",
        Platform::DingTalk => "im-bind-dingtalk",
        Platform::Qq => "im-bind-qq",
    }
}

/// Collects the text typed into the fields of `spec`.
fn typed_values(
    inputs: &[Entity<InputField>],
    spec: &PlatformSpec,
    cx: &gpui::App,
) -> CredentialValues {
    let mut values = CredentialValues::new();
    for (field, input) in spec.fields.iter().zip(inputs) {
        values.insert(
            field.key.to_string(),
            input.read(cx).text(cx).trim().to_string(),
        );
    }
    values
}

/// True when the credentials a device flow would return are all present.
fn credentials_complete(spec: &PlatformSpec, values: &CredentialValues) -> bool {
    spec.fields
        .iter()
        .take(DEVICE_FLOW_FIELDS)
        .all(|field| values.get(field.key).is_some_and(|value| !value.is_empty()))
}

/// Draws `content` as a QR code.
///
/// The modules are plain rectangles at absolute positions: the OHOS backend
/// has no image pipeline, so an `img` element never reaches the screen, and
/// absolute placement keeps rows from picking up sub-pixel gaps.
fn qr_view(content: &str) -> AnyElement {
    let rows = match im_channel::qr::modules(content) {
        Ok(rows) => rows,
        Err(error) => {
            log::error!("agent container settings: cannot encode the QR payload: {error}");
            return Label::new(localization::localized_str!("Failed to display the QR code"))
                .size(LabelSize::Small)
                .into_any_element();
        }
    };
    let module_count = rows.len() as f32;
    let module_size = QR_SIDE_PX / (module_count + 2. * QR_QUIET_ZONE_MODULES);
    let quiet_zone = QR_QUIET_ZONE_MODULES * module_size;
    // Snap every edge to a whole pixel: the symbol is scaled to a fixed side,
    // so fractional edges would blur the modules.
    let edge = |modules: f32| (quiet_zone + modules * module_size).round();
    let mut canvas = div()
        .relative()
        .flex_none()
        .w(px(QR_SIDE_PX))
        .h(px(QR_SIDE_PX))
        .bg(gpui::white());
    for (row_index, row) in rows.iter().enumerate() {
        let mut column = 0;
        while column < row.len() {
            if !row[column] {
                column += 1;
                continue;
            }
            let run_start = column;
            while column < row.len() && row[column] {
                column += 1;
            }
            let left = edge(run_start as f32);
            let right = edge(column as f32);
            let top = edge(row_index as f32);
            let bottom = edge(row_index as f32 + 1.);
            // Rows overlap by half a pixel: adjacent quads that only touch
            // leave an antialiased seam, which shows up as light horizontal
            // lines across the symbol.
            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px((right - left).max(1.)))
                    .h(px((bottom - top).max(1.) + 0.5))
                    .bg(gpui::black()),
            );
        }
    }
    canvas.into_any_element()
}

/// The agents whose history this build reads, drawn with the same cells the
/// integration list uses. Read-only: it says what "Show History" covers,
/// so it follows the switch instead of duplicating it.
fn scannable_grid(border: gpui::Hsla) -> AnyElement {
    let mut grid = div()
        .w_full()
        .grid()
        .grid_cols(AGENT_COLUMNS)
        .border_t_1()
        .border_l_1()
        .border_color(border);
    for (cell_index, agent) in agent_panel::agent_display::SCANNABLE_AGENTS
        .iter()
        .enumerate()
    {
        grid = grid.child(scannable_cell(agent, cell_index, border));
    }
    grid.into_any_element()
}

/// One read-only cell. Same shape as an integration cell minus the installed
/// check, because there is no per-agent state to report here.
fn scannable_cell(agent: &str, cell_index: usize, border: gpui::Hsla) -> AnyElement {
    h_flex()
        .id(("scannable-agent", cell_index))
        .w_full()
        .items_center()
        .justify_start()
        .gap_1p5()
        .h(rems_from_px(AGENT_CELL_HEIGHT_PX))
        .px(rems_from_px(AGENT_CELL_INSET_PX))
        .min_w_0()
        .overflow_hidden()
        .border_b_1()
        .border_r_1()
        .border_color(border)
        .child(
            Icon::new(agent_panel::agent_display::agent_icon(agent))
                .size(IconSize::XSmall)
                .color(Color::Muted),
        )
        .child(
            div().flex_1().min_w_0().overflow_hidden().child(
                Label::new(agent_panel::agent_display::integration_name(agent))
                    .size(LabelSize::Small)
                    .truncate(),
            ),
        )
        .into_any_element()
}

/// One settings row: title and description on the left, control on the right,
/// matching the rows the rest of the settings pages draw.
fn setting_row(
    title: impl Into<gpui::SharedString>,
    description: impl Into<gpui::SharedString>,
    control: AnyElement,
) -> AnyElement {
    h_flex()
        .w_full()
        .min_w_0()
        .justify_between()
        .gap_4()
        .child(
            v_flex()
                .w_full()
                .max_w_2_3()
                .min_w_0()
                .child(Label::new(title))
                .child(
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
        .child(control)
        .into_any_element()
}

fn block(child: AnyElement) -> AnyElement {
    v_flex()
        .w_full()
        .px_8()
        .pt_4()
        .pb_4()
        .child(child)
        .into_any_element()
}

fn divider() -> AnyElement {
    h_flex()
        .w_full()
        .px_8()
        .child(Divider::horizontal().color(DividerColor::BorderFaded))
        .into_any_element()
}

fn outlined_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
) -> Button {
    Button::new(id, label)
        .style(ButtonStyle::Outlined)
        .size(ButtonSize::Default)
}
