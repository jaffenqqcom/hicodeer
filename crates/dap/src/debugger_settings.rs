use dap_types::SteppingGranularity;
use settings::{RegisterSetting, Settings, SettingsContent};

#[derive(Debug, RegisterSetting)]
pub struct DebuggerSettings {
    /// Determines the stepping granularity.
    ///
    /// Default: line
    pub stepping_granularity: SteppingGranularity,
    /// Whether the breakpoints should be reused across Zed sessions.
    ///
    /// Default: true
    pub save_breakpoints: bool,
    /// Whether to show the debug button in the status bar.
    ///
    /// Default: true
    pub button: bool,
    /// Time in milliseconds until timeout error when connecting to a TCP debug adapter
    ///
    /// Default: 2000ms
    pub timeout: u64,
    /// Whether to log messages between active debug adapters and Zed
    ///
    /// Default: true
    pub log_dap_communications: bool,
    /// Whether to format dap messages in when adding them to debug adapter logger
    ///
    /// Default: true
    pub format_dap_log_messages: bool,
    /// The dock position of the debug panel
    ///
    /// Default: Bottom
    pub dock: settings::DockPosition,
}

// [OHOS PORT BEGIN] The debugger settings section is not compiled on OHOS, so the
// values that used to come from `settings.json` are fixed to the previous defaults.
// Restore `from_settings` from upstream to bring the section back.
const DEFAULT_DAP_TIMEOUT_MS: u64 = 2000;

impl Settings for DebuggerSettings {
    fn from_settings(_content: &SettingsContent) -> Self {
        Self {
            stepping_granularity: dap_types::SteppingGranularity::Line,
            save_breakpoints: true,
            button: false,
            timeout: DEFAULT_DAP_TIMEOUT_MS,
            log_dap_communications: true,
            format_dap_log_messages: true,
            dock: settings::DockPosition::Bottom,
        }
    }
}
// [OHOS PORT END]

