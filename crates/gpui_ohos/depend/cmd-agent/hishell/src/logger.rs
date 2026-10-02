//! Logging sink for the hishell bridge.
//!
//! The bridge's standard output carries the remote shell's bytes, so nothing
//! here may ever write to stdout: a record mixed into that stream would appear
//! inside the user's terminal session. On OHOS records go to hilog; on a plain
//! host they go to stderr, where only the local bring-up path looks for them.
//!
//! On OHOS a logger is always installed: a run's warnings and errors are
//! recorded even without `--log`, because the device has no console a failure
//! could be read from. `--log` widens it to the debug-level diagnostics. On a
//! plain host `--log` remains what installs a logger at all: there the run has
//! a terminal, and `main` prints the failure that ends it to stderr either way.

/// Installs the process-wide logger. `verbose` is the `--log` flag.
pub fn init(verbose: bool) {
    #[cfg(target_env = "ohos")]
    ohos::init(verbose);
    #[cfg(not(target_env = "ohos"))]
    if verbose {
        stderr_logger::init();
    }
}

#[cfg(target_env = "ohos")]
mod ohos {
    use std::ffi::{CStr, CString, c_char};

    use log::{Level, LevelFilter, Log, Metadata, Record};

    /// LOG_APP: third-party applications always use this log type.
    const LOG_APP: i32 = 0;
    /// hilog LogLevel enum: DEBUG=3 / WARN=5 / ERROR=6. INFO=4 is unused: the
    /// device filters app records below WARN, so nothing is ever recorded at it
    /// (see `hilog_level`).
    const LOG_LEVEL_DEBUG: i32 = 3;
    const LOG_LEVEL_WARN: i32 = 5;
    const LOG_LEVEL_ERROR: i32 = 6;
    /// App service domain, 0x0001 and up are user-defined.
    const HILOG_DOMAIN: u32 = 0x0001;
    /// hilog tag, kept short enough (<31 bytes) to avoid truncation.
    const HILOG_TAG: &CStr = c"hishell";
    /// Single `%{public}s` format: the whole message is one public plain-text arg.
    const HILOG_FORMAT: &CStr = c"%{public}s";

    #[link(name = "hilog_ndk.z")]
    unsafe extern "C" {
        fn OH_LOG_Print(
            log_type: i32,
            log_level: i32,
            domain: u32,
            tag: *const c_char,
            fmt: *const c_char,
            ...
        ) -> i32;
    }

    /// Maps a `log` level to its hilog LogLevel value.
    ///
    /// Records more verbose than WARN never reach this logger, the installed
    /// max level being WARN, so the `Info` arm is kept only to keep the
    /// mapping total. Error keeps its own level, so a real failure stands out
    /// from the warnings around it.
    fn hilog_level(level: Level) -> i32 {
        match level {
            Level::Error => LOG_LEVEL_ERROR,
            Level::Warn | Level::Info => LOG_LEVEL_WARN,
            Level::Debug | Level::Trace => LOG_LEVEL_DEBUG,
        }
    }

    struct HilogLogger;

    impl Log for HilogLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            let message = format!("[hishell:{}] {}", record.target(), record.args());
            // A message containing an interior NUL cannot become a C string.
            // Logging that failure here would recurse into this very logger, so
            // the record is dropped instead.
            let Ok(message_c) = CString::new(message) else {
                return;
            };
            // SAFETY: message_c is a NUL-terminated C string alive for the call;
            // the variadic argument is consumed by the `%{public}s` format.
            unsafe {
                OH_LOG_Print(
                    LOG_APP,
                    hilog_level(record.level()),
                    HILOG_DOMAIN,
                    HILOG_TAG.as_ptr(),
                    HILOG_FORMAT.as_ptr(),
                    message_c.as_ptr(),
                );
            }
        }

        fn flush(&self) {}
    }

    static LOGGER: HilogLogger = HilogLogger;

    /// Level for a run without `--log`.
    ///
    /// `HICODEER_LOG_LEVEL` is the single source of truth for the on-device
    /// hilog level: the build script exports it and every sink reads it at
    /// compile time. An unset or unrecognized value falls back to the build
    /// profile default (see [`default_level`]). `--log` still widens the sink
    /// to debug diagnostics.
    const DEFAULT_LEVEL: LevelFilter = match option_env!("HICODEER_LOG_LEVEL") {
        Some(raw) => match level_filter_from_str_const(raw) {
            Some(level) => level,
            None => default_level(),
        },
        None => default_level(),
    };

    /// Fallback level when `HICODEER_LOG_LEVEL` is unset or unrecognized: a
    /// debug build keeps `info`, a release build drops to `warn`.
    const fn default_level() -> LevelFilter {
        if cfg!(debug_assertions) {
            LevelFilter::Info
        } else {
            LevelFilter::Warn
        }
    }

    /// Parses a `HICODEER_LOG_LEVEL` value at compile time. Kept `const` and
    /// byte-based because [`DEFAULT_LEVEL`] is a `const`; the comparison is
    /// ASCII-case-insensitive, matching the values the build script documents.
    const fn level_filter_from_str_const(level: &str) -> Option<LevelFilter> {
        const fn ascii_eq_ignore_case(input: &str, expected: &str) -> bool {
            let input = input.as_bytes();
            let expected = expected.as_bytes();
            if input.len() != expected.len() {
                return false;
            }
            let mut index = 0;
            while index < input.len() {
                if input[index].to_ascii_lowercase() != expected[index] {
                    return false;
                }
                index += 1;
            }
            true
        }

        if ascii_eq_ignore_case(level, "trace") {
            Some(LevelFilter::Trace)
        } else if ascii_eq_ignore_case(level, "debug") {
            Some(LevelFilter::Debug)
        } else if ascii_eq_ignore_case(level, "info") {
            Some(LevelFilter::Info)
        } else if ascii_eq_ignore_case(level, "warn") {
            Some(LevelFilter::Warn)
        } else if ascii_eq_ignore_case(level, "error") {
            Some(LevelFilter::Error)
        } else if ascii_eq_ignore_case(level, "off") {
            Some(LevelFilter::Off)
        } else {
            None
        }
    }

    /// Installs the hilog logger as the process-wide `log` logger. `verbose`
    /// (the `--log` flag) widens it from warnings and errors to the
    /// debug-level diagnostics.
    pub fn init(verbose: bool) {
        // Installing a logger fails only when one is already installed: the
        // first installation wins and this one is intentionally ignored.
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(if verbose {
            LevelFilter::Debug
        } else {
            DEFAULT_LEVEL
        });
    }
}

#[cfg(not(target_env = "ohos"))]
mod stderr_logger {
    use log::{LevelFilter, Log, Metadata, Record};

    struct StderrLogger;

    impl Log for StderrLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            eprintln!("hishell {}: {}", record.level(), record.args());
        }

        fn flush(&self) {}
    }

    /// Installs the stderr logger as the process-wide `log` logger.
    pub fn init() {
        // Installing a logger fails only when one is already installed: the
        // first installation wins and this one is intentionally ignored.
        let _ = log::set_logger(&StderrLogger);
        log::set_max_level(LevelFilter::Info);
    }
}
