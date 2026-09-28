// ===== [OHOS PORT] file added by the OHOS port (does not exist upstream) =====
//! OHOS platform logging: delivers Rust `log::xxx!()` records to the hilog
//! system log service through `OH_LOG_Print`, bypassing the file/stdout sinks.
//!
//! This module must not use the `log::info!()` family, which would recurse back
//! into `Zlog::log()`; use `direct_hilog_info` for early-startup diagnostics.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use log::Record;

// log_type: third-party applications always use LOG_APP (enum value 0).
const LOG_APP: i32 = 0;

// hilog LogLevel enum values: DEBUG=3 / INFO=4 / WARN=5 / ERROR=6.
const LOG_LEVEL_DEBUG: i32 = 3;
const LOG_LEVEL_INFO: i32 = 4;
const LOG_LEVEL_WARN: i32 = 5;
const LOG_LEVEL_ERROR: i32 = 6;

// Application service domain; values from 0x0001 up are available for custom use.
const HILOG_DOMAIN: u32 = 0x0001;
// hilog tag (at most 31 bytes) used to aggregate logs by application.
const HILOG_TAG: &CStr = c"Zed";
// Format string: the whole message is delivered verbatim as a public argument.
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

/// Delivers one `log::Record` to hilog. Called by `Zlog::log()` on OHOS; the
/// level mapping mirrors the hilog enum one-to-one.
pub fn submit_to_hilog(record: &Record) {
    let hi_log_level = ohos_log_level(record.level());
    let message = record.args().to_string();
    let Ok(message) = CString::new(message) else {
        // The message contains an interior NUL (extremely unlikely). There is no
        // better logging channel available here, so drop this single record.
        return;
    };
    unsafe {
        OH_LOG_Print(
            LOG_APP,
            hi_log_level,
            HILOG_DOMAIN,
            HILOG_TAG.as_ptr(),
            HILOG_FORMAT.as_ptr(),
            message.as_ptr(),
        );
    }
}

/// Maps `log::Level` to the hilog `LogLevel`: error=6 / warn=5 / info=4 /
/// debug|trace=3.
fn ohos_log_level(level: log::Level) -> i32 {
    match level {
        log::Level::Error => LOG_LEVEL_ERROR,
        log::Level::Warn => LOG_LEVEL_WARN,
        log::Level::Info => LOG_LEVEL_INFO,
        log::Level::Debug | log::Level::Trace => LOG_LEVEL_DEBUG,
    }
}

/// Calls hilog directly, bypassing the `log` macros, for early-startup diagnostics
/// (the global logger may not be registered yet).
///
/// Comparing this against `log::info!()` separates two failure modes:
/// - `direct_hilog_info` appears but `log::info!()` does not -> redirection not wired
/// - neither appears -> the hilog NDK link or the `OH_LOG_Print` FFI itself is broken
pub fn direct_hilog_info(tag: &str, message: &str) {
    let Ok(tag) = CString::new(tag) else {
        return;
    };
    let Ok(message) = CString::new(message) else {
        return;
    };
    unsafe {
        OH_LOG_Print(
            LOG_APP,
            LOG_LEVEL_INFO,
            HILOG_DOMAIN,
            tag.as_ptr(),
            HILOG_FORMAT.as_ptr(),
            message.as_ptr(),
        );
    }
}
