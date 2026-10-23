use super::App;

impl App {
    pub(crate) fn set_host_terminal_appearance_state(
        &mut self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
        explicit: bool,
    ) -> bool {
        if self.state.host_terminal_appearance == appearance
            && self.state.host_terminal_appearance_explicit == explicit
        {
            return false;
        }
        self.state.host_terminal_appearance = appearance;
        self.state.host_terminal_appearance_explicit = explicit;
        self.apply_host_terminal_appearance_to_panes();
        self.refresh_effective_app_theme()
    }

    pub(crate) fn set_host_terminal_theme(
        &mut self,
        theme: crate::terminal_theme::TerminalTheme,
    ) -> bool {
        let theme = theme.with_inverse_fallback(palette_inverse_fallback(&self.state.palette));
        if theme == self.state.host_terminal_theme {
            return false;
        }
        self.state.host_terminal_theme = theme;
        self.apply_host_terminal_theme_to_panes();
        true
    }

    pub(super) fn refresh_effective_app_theme(&mut self) -> bool {
        let (palette, theme_name) = super::resolve_effective_theme(
            &self.state.theme_runtime,
            self.state.host_terminal_appearance,
        );
        if self.state.theme_name == theme_name && self.state.palette == palette {
            return false;
        }
        self.state.theme_name = theme_name;
        self.state.palette = palette;
        self.refresh_pane_inverse_fallback();
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
        true
    }

    // Keep the reverse-video fallback (used when the host terminal never
    // reports its own default colors) in sync with the palette.
    fn refresh_pane_inverse_fallback(&mut self) {
        let theme = self
            .state
            .host_terminal_theme
            .with_inverse_fallback(palette_inverse_fallback(&self.state.palette));
        if theme == self.state.host_terminal_theme {
            return;
        }
        self.state.host_terminal_theme = theme;
        self.apply_host_terminal_theme_to_panes();
    }

    fn apply_host_terminal_appearance_to_panes(&self) {
        for runtime in self.terminal_runtimes.values() {
            runtime.apply_host_terminal_appearance(self.state.host_terminal_appearance);
        }
    }

    fn apply_host_terminal_theme_to_panes(&self) {
        for runtime in self.terminal_runtimes.values() {
            runtime.apply_host_terminal_theme(self.state.host_terminal_theme);
        }

        self.render_dirty.request_generic();
        self.render_notify.notify_one();
    }
}

fn palette_default_color(color: ratatui::style::Color) -> Option<crate::terminal_theme::RgbColor> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some(crate::terminal_theme::RgbColor { r, g, b }),
        _ => None,
    }
}

/// Palette (foreground, background) used to resolve reverse video (SGR 7)
/// when the host terminal never reports its own default colors. Consumed only
/// by the pane renderer; it is never written into the pane as its default
/// colors, so applications keep inferring the terminal appearance as before.
pub(super) fn palette_inverse_fallback(
    palette: &crate::app::state::Palette,
) -> Option<(
    crate::terminal_theme::RgbColor,
    crate::terminal_theme::RgbColor,
)> {
    let background = match palette.panel_bg {
        ratatui::style::Color::Reset => palette.surface_dim,
        color => color,
    };
    Some((
        palette_default_color(palette.text)?,
        palette_default_color(background)?,
    ))
}
