use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const PERSISTENT_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const PERSISTENT_LOG_RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource {
    System,
    Disk,
    Network,
    Domain,
    Storage,
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

#[derive(Debug)]
pub struct PersistentLogger {
    root: PathBuf,
    file: File,
    date: String,
    path: PathBuf,
    size: u64,
    max_bytes: u64,
    retention_days: i64,
}

impl PersistentLogger {
    pub fn from_home() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Self::with_config(
            log_root(Path::new(&home)),
            SystemTime::now(),
            PERSISTENT_LOG_MAX_BYTES,
            PERSISTENT_LOG_RETENTION_DAYS,
        )
    }

    pub(crate) fn with_config(
        root: PathBuf,
        now: SystemTime,
        max_bytes: u64,
        retention_days: i64,
    ) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        let day = unix_day(now);
        cleanup_expired(&root, day, retention_days)?;
        let date = date_string(day);
        let (path, file, size) = open_current_file(&root, &date, max_bytes)?;
        Ok(Self {
            root,
            file,
            date,
            path,
            size,
            max_bytes,
            retention_days,
        })
    }

    pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
        let day = unix_day(entry.timestamp);
        let date = date_string(day);
        if date != self.date {
            cleanup_expired(&self.root, day, self.retention_days)?;
            let (path, file, size) = open_current_file(&self.root, &date, self.max_bytes)?;
            self.date = date;
            self.path = path;
            self.file = file;
            self.size = size;
        }

        let mut line = serialize_json(entry);
        line.push('\n');
        let line_size = line.len() as u64;
        if self.size > 0 && self.size.saturating_add(line_size) > self.max_bytes {
            let (path, file) = create_rotated_file(&self.root, &self.date)?;
            self.path = path;
            self.file = file;
            self.size = 0;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        self.size = self.size.saturating_add(line_size);
        Ok(())
    }

    pub fn load_history(&self, limit: usize) -> (VecDeque<LogEntry>, Vec<String>) {
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        let current_day = unix_day(SystemTime::now());
        let oldest_kept = current_day.saturating_sub(self.retention_days.saturating_sub(1));
        let directory = match fs::read_dir(&self.root) {
            Ok(directory) => directory,
            Err(error) => {
                warnings.push(format!(
                    "failed to read persistent log directory {}: {error}",
                    self.root.display()
                ));
                return (VecDeque::new(), warnings);
            }
        };

        let mut files = Vec::new();
        for directory_entry in directory {
            let directory_entry = match directory_entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(format!("failed to inspect persistent log entry: {error}"));
                    continue;
                }
            };
            let path = directory_entry.path();
            let Some((day, rotation)) = log_file_key(&path) else {
                continue;
            };
            if day < oldest_kept || day > current_day {
                continue;
            }
            files.push((day, rotation, path));
        }
        files.sort_by_key(|(day, rotation, _)| (*day, *rotation));

        for (_, _, path) in files {
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    warnings.push(format!("failed to read {}: {error}", path.display()));
                    continue;
                }
            };
            for (line_index, line) in content.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_json_entry(line) {
                    Ok(entry) => entries.push(entry),
                    Err(error) => warnings.push(format!(
                        "failed to parse {} line {}: {error}",
                        path.display(),
                        line_index + 1
                    )),
                }
            }
        }

        entries.sort_by_key(|entry| entry.timestamp);
        let skip = entries.len().saturating_sub(limit);
        (entries.into_iter().skip(skip).collect(), warnings)
    }
}

fn log_root(home: &Path) -> PathBuf {
    home.join(".local/state/ovm-top/logs")
}

fn open_current_file(root: &Path, date: &str, max_bytes: u64) -> io::Result<(PathBuf, File, u64)> {
    let base = root.join(format!("{date}.jsonl"));
    if !base.exists() || base.metadata()?.len() < max_bytes {
        return open_append(base);
    }

    let mut index = 1_u32;
    loop {
        let path = root.join(format!("{date}.{index}.jsonl"));
        if !path.exists() || path.metadata()?.len() < max_bytes {
            return open_append(path);
        }
        index = index.saturating_add(1);
    }
}

fn open_append(path: PathBuf) -> io::Result<(PathBuf, File, u64)> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(&path)?;
    set_private_permissions(&path)?;
    let size = file.metadata()?.len();
    Ok((path, file, size))
}

fn create_rotated_file(root: &Path, date: &str) -> io::Result<(PathBuf, File)> {
    let mut index = 1_u32;
    loop {
        let path = root.join(format!("{date}.{index}.jsonl"));
        let mut options = OpenOptions::new();
        options.create_new(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => {
                set_private_permissions(&path)?;
                return Ok((path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                index = index.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
    }
}

fn set_private_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn cleanup_expired(root: &Path, current_day: i64, retention_days: i64) -> io::Result<()> {
    let oldest_kept = current_day.saturating_sub(retention_days.saturating_sub(1));
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(date) = name.get(..10).filter(|_| {
            name.ends_with(".jsonl")
                && name.as_bytes().get(4) == Some(&b'-')
                && name.as_bytes().get(7) == Some(&b'-')
        }) else {
            continue;
        };
        if let Some(day) = parse_date(date)
            && day < oldest_kept
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn log_file_key(path: &Path) -> Option<(i64, u32)> {
    let name = path.file_name()?.to_str()?;
    let date = name.get(..10)?;
    let suffix = name.get(10..)?;
    let rotation = if suffix == ".jsonl" {
        0
    } else {
        suffix
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".jsonl"))
            .filter(|index| !index.is_empty())
            .and_then(|index| index.parse::<u32>().ok())?
    };
    Some((parse_date(date)?, rotation))
}

fn serialize_json(entry: &LogEntry) -> String {
    format!(
        "{{\"ts\":\"{}\",\"level\":\"{}\",\"source\":\"{}\",\"message\":\"{}\"}}",
        iso_timestamp(entry.timestamp),
        level_label(entry.level),
        source_label(entry.source),
        json_escape(&entry.message)
    )
}

fn parse_json_entry(line: &str) -> Result<LogEntry, String> {
    let mut position = 0;
    skip_whitespace(line, &mut position);
    expect_byte(line, &mut position, b'{')?;
    let mut fields = HashMap::new();
    loop {
        skip_whitespace(line, &mut position);
        if consume_byte(line, &mut position, b'}') {
            break;
        }
        let key = parse_json_string(line, &mut position)?;
        skip_whitespace(line, &mut position);
        expect_byte(line, &mut position, b':')?;
        skip_whitespace(line, &mut position);
        let value = parse_json_string(line, &mut position)?;
        fields.insert(key, value);
        skip_whitespace(line, &mut position);
        if consume_byte(line, &mut position, b'}') {
            break;
        }
        expect_byte(line, &mut position, b',')?;
    }
    skip_whitespace(line, &mut position);
    if position != line.len() {
        return Err("trailing characters after JSON object".into());
    }

    let timestamp = parse_iso_timestamp(required_field(&fields, "ts")?)?;
    let level = match required_field(&fields, "level")? {
        "INFO" => LogLevel::Info,
        "WARN" => LogLevel::Warn,
        "ERROR" => LogLevel::Error,
        value => return Err(format!("unknown level {value:?}")),
    };
    let source = match required_field(&fields, "source")? {
        "SYSTEM" => LogSource::System,
        "DISK" => LogSource::Disk,
        "NET" => LogSource::Network,
        "DOMAIN" => LogSource::Domain,
        "STORAGE" => LogSource::Storage,
        "UI" => LogSource::Ui,
        value => return Err(format!("unknown source {value:?}")),
    };
    Ok(LogEntry {
        timestamp,
        level,
        source,
        message: required_field(&fields, "message")?.to_string(),
    })
}

fn required_field<'a>(fields: &'a HashMap<String, String>, name: &str) -> Result<&'a str, String> {
    fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {name:?} field"))
}

fn skip_whitespace(value: &str, position: &mut usize) {
    while value
        .as_bytes()
        .get(*position)
        .is_some_and(u8::is_ascii_whitespace)
    {
        *position += 1;
    }
}

fn consume_byte(value: &str, position: &mut usize, expected: u8) -> bool {
    if value.as_bytes().get(*position) == Some(&expected) {
        *position += 1;
        true
    } else {
        false
    }
}

fn expect_byte(value: &str, position: &mut usize, expected: u8) -> Result<(), String> {
    consume_byte(value, position, expected)
        .then_some(())
        .ok_or_else(|| format!("expected {:?} at byte {}", expected as char, *position))
}

fn parse_json_string(value: &str, position: &mut usize) -> Result<String, String> {
    expect_byte(value, position, b'"')?;
    let mut output = String::new();
    loop {
        let character = value
            .get(*position..)
            .and_then(|rest| rest.chars().next())
            .ok_or_else(|| "unterminated JSON string".to_string())?;
        *position += character.len_utf8();
        match character {
            '"' => return Ok(output),
            '\\' => {
                let escaped = value
                    .as_bytes()
                    .get(*position)
                    .copied()
                    .ok_or_else(|| "unterminated JSON escape".to_string())?;
                *position += 1;
                match escaped {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'/' => output.push('/'),
                    b'b' => output.push('\u{08}'),
                    b'f' => output.push('\u{0c}'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    b'u' => output.push(parse_unicode_escape(value, position)?),
                    _ => return Err(format!("invalid JSON escape \\{}", escaped as char)),
                }
            }
            character if character <= '\u{1f}' => {
                return Err("unescaped control character in JSON string".into());
            }
            character => output.push(character),
        }
    }
}

fn parse_unicode_escape(value: &str, position: &mut usize) -> Result<char, String> {
    let digits = value
        .get(*position..position.saturating_add(4))
        .ok_or_else(|| "incomplete Unicode escape".to_string())?;
    let first = u16::from_str_radix(digits, 16).map_err(|_| "invalid Unicode escape")?;
    *position += 4;
    if (0xd800..=0xdbff).contains(&first) {
        if value.as_bytes().get(*position..position.saturating_add(2)) != Some(b"\\u") {
            return Err("high surrogate without low surrogate".into());
        }
        *position += 2;
        let digits = value
            .get(*position..position.saturating_add(4))
            .ok_or_else(|| "incomplete low surrogate".to_string())?;
        let second = u16::from_str_radix(digits, 16).map_err(|_| "invalid low surrogate")?;
        *position += 4;
        if !(0xdc00..=0xdfff).contains(&second) {
            return Err("invalid low surrogate".into());
        }
        let scalar = 0x1_0000 + (((first as u32 - 0xd800) << 10) | (second as u32 - 0xdc00));
        char::from_u32(scalar).ok_or_else(|| "invalid Unicode scalar".into())
    } else if (0xdc00..=0xdfff).contains(&first) {
        Err("low surrogate without high surrogate".into())
    } else {
        char::from_u32(first as u32).ok_or_else(|| "invalid Unicode scalar".into())
    }
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character <= '\u{1f}' => {
                use std::fmt::Write as _;
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output
}

fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().min(i64::MAX as u64) as i64,
        Err(error) => -(error.duration().as_secs().min(i64::MAX as u64) as i64),
    }
}

fn unix_day(time: SystemTime) -> i64 {
    unix_seconds(time).div_euclid(86_400)
}

fn date_string(day: i64) -> String {
    let (year, month, date) = civil_from_days(day);
    format!("{year:04}-{month:02}-{date:02}")
}

fn iso_timestamp(time: SystemTime) -> String {
    let seconds = unix_seconds(time);
    let day = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, date) = civil_from_days(day);
    format!(
        "{year:04}-{month:02}-{date:02}T{:02}:{:02}:{:02}Z",
        day_seconds / 3600,
        (day_seconds / 60) % 60,
        day_seconds % 60
    )
}

fn parse_iso_timestamp(value: &str) -> Result<SystemTime, String> {
    if value.len() != 20
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
        || value.as_bytes().get(19) != Some(&b'Z')
    {
        return Err(format!("invalid UTC timestamp {value:?}"));
    }
    let date = value
        .get(..10)
        .ok_or_else(|| format!("invalid date in {value:?}"))?;
    let day = parse_date(date).ok_or_else(|| format!("invalid date in {value:?}"))?;
    let hour = value
        .get(11..13)
        .ok_or_else(|| format!("invalid hour in {value:?}"))?
        .parse::<i64>()
        .map_err(|_| format!("invalid hour in {value:?}"))?;
    let minute = value
        .get(14..16)
        .ok_or_else(|| format!("invalid minute in {value:?}"))?
        .parse::<i64>()
        .map_err(|_| format!("invalid minute in {value:?}"))?;
    let second = value
        .get(17..19)
        .ok_or_else(|| format!("invalid second in {value:?}"))?
        .parse::<i64>()
        .map_err(|_| format!("invalid second in {value:?}"))?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return Err(format!("invalid time in {value:?}"));
    }
    let seconds = day
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(hour * 3600 + minute * 60 + second))
        .ok_or_else(|| format!("timestamp out of range {value:?}"))?;
    if seconds >= 0 {
        Ok(UNIX_EPOCH + std::time::Duration::from_secs(seconds as u64))
    } else {
        Ok(UNIX_EPOCH - std::time::Duration::from_secs(seconds.unsigned_abs()))
    }
}

fn civil_from_days(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let date = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, date as u32)
}

fn parse_date(value: &str) -> Option<i64> {
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let date = value.get(8..10)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&date) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + date - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let day = era * 146_097 + day_of_era - 719_468;
    (date_string(day) == value).then_some(day)
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
        LogSource::Storage => "STORAGE",
        LogSource::Ui => "UI",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("ovm-top-{name}-{}-{unique}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(day: i64, message: &str) -> LogEntry {
        LogEntry {
            timestamp: UNIX_EPOCH + Duration::from_secs((day * 86_400 + 45_296) as u64),
            level: LogLevel::Warn,
            source: LogSource::Storage,
            message: message.to_string(),
        }
    }

    #[test]
    fn serializes_valid_json_escaping_and_utc_timestamp() {
        let day = parse_date("2026-10-07").unwrap();
        let json = serialize_json(&entry(day, "quote \" slash \\ line\n控制\u{0001}"));

        assert_eq!(
            json,
            "{\"ts\":\"2026-10-07T12:34:56Z\",\"level\":\"WARN\",\"source\":\"STORAGE\",\"message\":\"quote \\\" slash \\\\ line\\n控制\\u0001\"}"
        );
    }

    #[test]
    fn builds_requested_state_directory_and_round_trips_dates() {
        assert_eq!(
            log_root(Path::new("/home/test")),
            PathBuf::from("/home/test/.local/state/ovm-top/logs")
        );
        for date in ["1970-01-01", "2024-02-29", "2026-10-07"] {
            assert_eq!(date_string(parse_date(date).unwrap()), date);
        }
        assert!(parse_date("2025-02-29").is_none());
    }

    #[test]
    fn rotates_without_overwriting_and_sets_private_permissions() {
        let directory = TestDirectory::new("rotation");
        let day = parse_date("2026-10-07").unwrap();
        let now = UNIX_EPOCH + Duration::from_secs((day * 86_400) as u64);
        let mut logger = PersistentLogger::with_config(directory.0.clone(), now, 120, 7).unwrap();

        logger
            .write(&entry(
                day,
                "first record with enough content to approach limit",
            ))
            .unwrap();
        logger
            .write(&entry(day, "second record must be preserved in rotation"))
            .unwrap();

        let base = directory.0.join("2026-10-07.jsonl");
        let rotated = directory.0.join("2026-10-07.1.jsonl");
        assert!(base.exists());
        assert!(rotated.exists());
        assert!(fs::read_to_string(base).unwrap().contains("first record"));
        assert!(
            fs::read_to_string(rotated)
                .unwrap()
                .contains("second record")
        );
        #[cfg(unix)]
        for path in [logger.path.clone(), directory.0.join("2026-10-07.jsonl")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn removes_only_logs_older_than_seven_utc_days() {
        let directory = TestDirectory::new("retention");
        fs::write(directory.0.join("2026-09-30.jsonl"), "old").unwrap();
        fs::write(directory.0.join("2026-10-01.1.jsonl"), "kept").unwrap();
        fs::write(directory.0.join("unrelated.txt"), "kept").unwrap();
        let day = parse_date("2026-10-07").unwrap();
        let now = UNIX_EPOCH + Duration::from_secs((day * 86_400) as u64);

        let _logger = PersistentLogger::with_config(directory.0.clone(), now, 1024, 7).unwrap();

        assert!(!directory.0.join("2026-09-30.jsonl").exists());
        assert!(directory.0.join("2026-10-01.1.jsonl").exists());
        assert!(directory.0.join("unrelated.txt").exists());
    }

    #[test]
    fn initialization_failure_can_fall_back_to_memory_only() {
        let directory = TestDirectory::new("fallback");
        let invalid_root = directory.0.join("not-a-directory");
        fs::write(&invalid_root, "file").unwrap();

        let result = PersistentLogger::with_config(
            invalid_root,
            UNIX_EPOCH,
            PERSISTENT_LOG_MAX_BYTES,
            PERSISTENT_LOG_RETENTION_DAYS,
        );

        assert!(result.is_err());
    }

    #[test]
    fn jsonl_history_round_trips_daily_and_rotation_files() {
        let directory = TestDirectory::new("history-round-trip");
        let day = unix_day(SystemTime::now());
        let now = UNIX_EPOCH + Duration::from_secs((day * 86_400 + 1) as u64);
        let mut logger = PersistentLogger::with_config(directory.0.clone(), now, 1, 7).unwrap();
        let first = entry(day, "escaped \"quote\" \\ path\nline 控制");
        let mut second = entry(day, "rotation entry");
        second.timestamp += Duration::from_secs(1);
        logger.write(&first).unwrap();
        logger.write(&second).unwrap();

        let (history, warnings) = logger.load_history(1000);

        assert!(warnings.is_empty());
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].message, "escaped \"quote\" \\ path\nline 控制");
        assert_eq!(history[0].level, LogLevel::Warn);
        assert_eq!(history[0].source, LogSource::Storage);
        assert_eq!(history[1].message, "rotation entry");
    }

    #[test]
    fn history_is_ordered_and_limited_to_newest_entries() {
        let directory = TestDirectory::new("history-order");
        let day = unix_day(SystemTime::now());
        let now = UNIX_EPOCH + Duration::from_secs((day * 86_400) as u64);
        let logger = PersistentLogger::with_config(directory.0.clone(), now, 1024, 7).unwrap();
        let mut newest = entry(day, "newest");
        newest.timestamp += Duration::from_secs(30);
        let mut oldest = entry(day, "oldest");
        oldest.timestamp -= Duration::from_secs(30);
        let middle = entry(day, "middle");
        fs::write(
            directory.0.join(format!("{}.1.jsonl", date_string(day))),
            format!(
                "{}\n{}\n{}\n",
                serialize_json(&newest),
                serialize_json(&oldest),
                serialize_json(&middle)
            ),
        )
        .unwrap();

        let (history, warnings) = logger.load_history(2);

        assert!(warnings.is_empty());
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            vec!["middle", "newest"]
        );
    }

    #[test]
    fn unreadable_history_warns_but_other_history_still_loads() {
        let directory = TestDirectory::new("history-fallback");
        let day = unix_day(SystemTime::now());
        let now = UNIX_EPOCH + Duration::from_secs((day * 86_400) as u64);
        let logger = PersistentLogger::with_config(directory.0.clone(), now, 1024, 7).unwrap();
        fs::write(
            directory.0.join(format!("{}.1.jsonl", date_string(day))),
            [0xff, 0xfe, 0xfd],
        )
        .unwrap();
        fs::write(
            directory.0.join(format!("{}.2.jsonl", date_string(day))),
            format!("{}\n", serialize_json(&entry(day, "available"))),
        )
        .unwrap();

        let (history, warnings) = logger.load_history(1000);

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].message, "available");
        assert_eq!(warnings.len(), 1);
    }
}
