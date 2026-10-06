use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy)]
pub enum LogSource {
    System,
    Disk,
    Network,
    Domain,
    Ui,
}

#[derive(Debug, Clone)]
pub struct LogEvent {
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub level: LogLevel,
    pub source: LogSource,
    pub message: String,
}

impl LogEvent {
    pub fn info(source: LogSource, message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Info,
            source,
            message: message.into(),
        }
    }

    pub fn warn(source: LogSource, message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Warn,
            source,
            message: message.into(),
        }
    }

    pub fn error(source: LogSource, message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Error,
            source,
            message: message.into(),
        }
    }
}

impl From<LogEvent> for LogEntry {
    fn from(event: LogEvent) -> Self {
        Self {
            timestamp: SystemTime::now(),
            level: event.level,
            source: event.source,
            message: event.message,
        }
    }
}

pub fn timestamp(entry: &LogEntry) -> String {
    let seconds = entry
        .timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day_seconds = seconds % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        day_seconds / 3600,
        (day_seconds / 60) % 60,
        day_seconds % 60
    )
}

pub fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

pub fn source_label(source: LogSource) -> &'static str {
    match source {
        LogSource::System => "SYSTEM",
        LogSource::Disk => "DISK",
        LogSource::Network => "NET",
        LogSource::Domain => "DOMAIN",
        LogSource::Ui => "UI",
    }
}
