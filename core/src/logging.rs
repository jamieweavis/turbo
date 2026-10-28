//! Routes the `log` macros to macOS unified logging, and panics with them.
//!
//! The app is launched by LaunchServices, so stderr goes nowhere. Everything goes to the
//! `com.jamieweavis.jaycast` subsystem instead, one category per module; `just logs`
//! streams it. Never log what the user typed or copied: lengths and kinds only.

use log::{Level, LevelFilter, Log, Metadata, Record};
use oslog::OsLog;
use std::sync::{Once, OnceLock};

pub const SUBSYSTEM: &str = "com.jamieweavis.jaycast";

/// Categories, named after the top-level modules. Anything else logs as "core".
const CATEGORIES: &[&str] = &[
    "core",
    "clipboard",
    "color_picker",
    "config",
    "display",
    "emoji",
    "files",
    "hyper",
    "input",
    "keep_awake",
    "launcher",
    "pasteboard",
    "snippets",
    "windows",
];

/// Installs the logger and the panic hook. Safe to call more than once.
pub fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(LevelFilter::Trace);
        }
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let location = info
                .location()
                .map_or_else(|| "unknown location".to_owned(), ToString::to_string);
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic");
            let category =
                category_for(info.location().map_or("core", |l| source_module(l.file())));
            category.fault(&format!(
                "panic on thread {} at {location}: {message}",
                thread.name().unwrap_or("unnamed")
            ));
            previous(info);
        }));
    });
}

static LOGGER: OsLogger = OsLogger;

struct OsLogger;

impl Log for OsLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Asking os_log first keeps debug logging on the event taps nearly free: the
        // message is only formatted while someone is streaming at debug level.
        category_for(module(metadata.target())).level_is_enabled(os_level(metadata.level()))
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            category_for(module(record.target()))
                .with_level(os_level(record.level()), &record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Error and Warn persist as errors, Info persists as "default", Debug and Trace are
/// kept only while streamed.
fn os_level(level: Level) -> oslog::Level {
    match level {
        Level::Error | Level::Warn => oslog::Level::Error,
        Level::Info => oslog::Level::Default,
        Level::Debug | Level::Trace => oslog::Level::Debug,
    }
}

/// `jaycast_core::input::tap` → `input`.
fn module(target: &str) -> &str {
    target.split("::").nth(1).unwrap_or("core")
}

/// `core/src/input/tap.rs` → `input`.
fn source_module(file: &str) -> &str {
    let after_src = file.split_once("src/").map_or(file, |(_, rest)| rest);
    let first = after_src.split('/').next().unwrap_or(after_src);
    first.strip_suffix(".rs").unwrap_or(first)
}

fn category_for(name: &str) -> &'static OsLog {
    static LOGS: OnceLock<Vec<OsLog>> = OnceLock::new();
    let logs = LOGS.get_or_init(|| {
        CATEGORIES
            .iter()
            .map(|category| OsLog::new(SUBSYSTEM, category))
            .collect()
    });
    let index = CATEGORIES.iter().position(|&c| c == name).unwrap_or(0);
    &logs[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_paths_map_to_categories() {
        assert_eq!(module("jaycast_core::input::tap"), "input");
        assert_eq!(module("jaycast_core::config"), "config");
        assert_eq!(module("jaycast_core"), "core");
        assert_eq!(source_module("core/src/input/tap.rs"), "input");
        assert_eq!(source_module("core/src/config.rs"), "config");
    }
}
