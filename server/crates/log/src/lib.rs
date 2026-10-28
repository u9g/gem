// Logging without tracing: one line of JSON per event on stderr, in the shape tracing-subscriber's
// JSON format wrote, which the launcher appends to server.log:
//
//     {"timestamp":"…","level":"INFO","fields":{"message":"…","key":"value"},"target":"gem"}
//
// RUST_LOG picks the lowest level written: error, warn, info (the default), debug or trace.
//
//     gem_log::info!("rpc answered", elapsed_ms = elapsed, caller = id);
//
// Each field's value is written with Display.

use std::{io::Write, sync::OnceLock};

use gem_json::{Map, Value};
use gem_proto::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        }
    }
}

// The most verbose level RUST_LOG lets through. A directive for a target (`livekit=warn`) is
// LiveKit's, which the room helper logs, so only bare levels count here.
fn max_level() -> Level {
    static MAX: OnceLock<Level> = OnceLock::new();
    *MAX.get_or_init(|| {
        let filter = std::env::var("RUST_LOG").unwrap_or_default();
        let mut max = Level::Info;
        for directive in filter
            .split(',')
            .map(str::trim)
            .filter(|d| !d.contains('='))
        {
            max = match directive.to_ascii_lowercase().as_str() {
                "error" => Level::Error,
                "warn" => Level::Warn,
                "info" => Level::Info,
                "debug" => Level::Debug,
                "trace" => Level::Trace,
                _ => max,
            };
        }
        max
    })
}

pub fn enabled(level: Level) -> bool {
    level <= max_level()
}

// One event's line.
pub fn line(level: Level, message: &str, fields: &[(&str, String)]) -> String {
    let mut object = Map::new();
    object.insert("message".to_string(), Value::String(message.to_string()));
    for (key, value) in fields {
        object.insert(key.to_string(), Value::String(value.clone()));
    }
    let mut event = Map::new();
    event.insert(
        "timestamp".to_string(),
        Value::String(Timestamp::now().to_rfc3339()),
    );
    event.insert("level".to_string(), Value::String(level.name().to_string()));
    event.insert("fields".to_string(), Value::Object(object));
    event.insert("target".to_string(), Value::String("gem".to_string()));
    Value::Object(event).to_string()
}

pub fn event(level: Level, message: &str, fields: &[(&str, String)]) {
    if !enabled(level) {
        return;
    }
    let mut text = line(level, message, fields);
    text.push('\n');
    let _ = std::io::stderr().write_all(text.as_bytes());
}

#[macro_export]
macro_rules! log {
    ($level:expr, $message:expr $(, $key:ident = $value:expr)* $(,)?) => {
        if $crate::enabled($level) {
            $crate::event(
                $level,
                &$message,
                &[$((stringify!($key), ::std::format!("{}", $value))),*],
            )
        }
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log!($crate::Level::Error, $($arg)*) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log!($crate::Level::Warn, $($arg)*) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log!($crate::Level::Info, $($arg)*) };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::log!($crate::Level::Debug, $($arg)*) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_tracing_json() {
        let line = line(Level::Warn, "gh failed", &[("what", "repos".to_string())]);
        let value = gem_json::parse(&line).unwrap();
        assert_eq!(value["level"], "WARN");
        assert_eq!(value["fields"]["message"], "gh failed");
        assert_eq!(value["fields"]["what"], "repos");
        assert!(value["timestamp"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn macros_take_display_fields() {
        let error = std::io::Error::other("boom");
        warn!("cannot run gh", error = error, what = "repos");
        info!(format!("{} done", 1));
        debug!("quiet", n = 3,);
    }
}
