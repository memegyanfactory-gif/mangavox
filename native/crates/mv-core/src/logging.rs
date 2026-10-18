//! JSON-lines logger compatible with `src/main/services/logger.ts`.
//!
//! Files: `<dir>/{main,renderer,ipc}.log`, one object per line:
//! `{"t":ISO,"lvl":"error","scope":"tag","msg":"...","pid":123,...extra}`.
//! Rotation at ~5 MB keeping 3 rotated files (`main.log.1` .. `main.log.3`).
//! The Electron app and this logger can share the same directory (filter by `pid`).
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP_ROTATED: usize = 3;
const RING_MAX: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogFile {
    Main,
    Renderer,
    Ipc,
}

impl LogFile {
    pub fn name(self) -> &'static str {
        match self {
            LogFile::Main => "main",
            LogFile::Renderer => "renderer",
            LogFile::Ipc => "ipc",
        }
    }
}

/// Redacts obvious secrets from free text (same patterns as the TS logger).
pub fn redact_string(s: &str) -> String {
    static RES: OnceLock<[(Regex, &'static str); 4]> = OnceLock::new();
    let res = RES.get_or_init(|| {
        [
            (Regex::new(r#"(?i)("[A-Za-z_-]*(?:api[_-]?key|apikey|token|secret|password|cookie|authorization)"\s*:\s*")[^"]+"#).unwrap(), "${1}[REDACTED]"),
            (Regex::new(r"\b(sk|pk|rk)-[A-Za-z0-9_-]{16,}").unwrap(), "${1}-[REDACTED]"),
            (Regex::new(r"(?i)(Bearer\s+)[A-Za-z0-9._~+/=-]{8,}").unwrap(), "${1}[REDACTED]"),
            (Regex::new(r#"(?i)([?&](?:api_?key|key|token|access_token|auth)=)[^&\s"]+"#).unwrap(), "${1}[REDACTED]"),
        ]
    });
    let mut out = s.to_string();
    for (re, rep) in res {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

fn is_secret_key(k: &str) -> bool {
    static SECRET: OnceLock<Regex> = OnceLock::new();
    static NOT: OnceLock<Regex> = OnceLock::new();
    let secret = SECRET.get_or_init(|| {
        Regex::new(r"(?i)(api[_-]?key|apikey|access[_-]?key|token|password|passwd|secret|cookie|authorization|credential|private[_-]?key|^auth$|^key$|session[_-]?id)").unwrap()
    });
    let not = NOT.get_or_init(|| Regex::new(r"(?i)(budget|count|limit|max|min|usage|stats|length|size|enabled|expires|type)").unwrap());
    secret.is_match(k) && !not.is_match(k)
}

/// Deep copy with secret-looking keys/values replaced (depth limited).
pub fn redact(value: &Value) -> Value {
    redact_depth(value, 0)
}

fn redact_depth(v: &Value, depth: usize) -> Value {
    match v {
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(a) if depth > 6 => json!(format!("[depth] {}", a.len())),
        Value::Array(a) => {
            let mut out: Vec<Value> = a.iter().take(50).map(|x| redact_depth(x, depth + 1)).collect();
            if a.len() > 50 {
                out.push(Value::String(format!("…+{} more", a.len() - 50)));
            }
            Value::Array(out)
        }
        Value::Object(_) if depth > 6 => Value::String("[depth]".into()),
        Value::Object(m) => {
            let mut out = Map::new();
            for (i, (k, x)) in m.iter().enumerate() {
                if i >= 60 {
                    out.insert("…".into(), Value::String("truncated".into()));
                    break;
                }
                let hide = is_secret_key(k) && !matches!(x, Value::Null | Value::Bool(_)) && x != &Value::String(String::new());
                out.insert(k.clone(), if hide { Value::String("[REDACTED]".into()) } else { redact_depth(x, depth + 1) });
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub struct Logger {
    dir: PathBuf,
    min: Level,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    sizes: HashMap<&'static str, u64>,
    ring: VecDeque<Value>,
}

impl Logger {
    pub fn new(dir: impl Into<PathBuf>, min: Level) -> Self {
        Self { dir: dir.into(), min, state: Mutex::new(State::default()) }
    }

    /// `MANGAVOX_LOG_DIR` / `MANGAVOX_LOG_LEVEL` aware, defaulting to `~/.mangavox/logs`.
    pub fn from_env() -> Self {
        let dir = std::env::var_os("MANGAVOX_LOG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::paths::root().join("logs"));
        let default = if cfg!(debug_assertions) { Level::Debug } else { Level::Info };
        let min = std::env::var("MANGAVOX_LOG_LEVEL").ok().and_then(|s| Level::parse(&s)).unwrap_or(default);
        Self::new(dir, min)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, file: LogFile) -> PathBuf {
        self.dir.join(format!("{}.log", file.name()))
    }

    fn rotate(&self, file: LogFile) {
        let base = self.path(file);
        let numbered = |i: usize| PathBuf::from(format!("{}.{i}", base.display()));
        let _ = std::fs::remove_file(numbered(KEEP_ROTATED));
        for i in (1..KEEP_ROTATED).rev() {
            if numbered(i).exists() {
                let _ = std::fs::rename(numbered(i), numbered(i + 1));
            }
        }
        let _ = std::fs::rename(&base, numbered(1));
    }

    /// Writes one record and returns it. Logging never fails the caller.
    pub fn write(&self, file: LogFile, level: Level, scope: &str, msg: &str, extra: Option<Map<String, Value>>) -> Value {
        let mut rec = Map::new();
        rec.insert("t".into(), json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)));
        rec.insert("lvl".into(), json!(level.as_str()));
        rec.insert("scope".into(), json!(scope));
        rec.insert("msg".into(), json!(redact_string(msg)));
        rec.insert("pid".into(), json!(std::process::id()));
        for (k, v) in extra.unwrap_or_default() {
            rec.entry(k).or_insert(v);
        }
        let rec = Value::Object(rec);

        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if level >= Level::Warn {
            let mut with_file = rec.clone();
            with_file["file"] = json!(file.name());
            st.ring.push_back(with_file);
            while st.ring.len() > RING_MAX {
                st.ring.pop_front();
            }
        }
        if level >= self.min {
            let mut line = serde_json::to_string(&rec).unwrap_or_default();
            line.push('\n');
            let _ = std::fs::create_dir_all(&self.dir);
            let path = self.path(file);
            let size = *st.sizes.entry(file.name()).or_insert_with(|| std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
            let mut size = size;
            if size + line.len() as u64 > MAX_BYTES {
                self.rotate(file);
                size = 0;
            }
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
                if f.write_all(line.as_bytes()).is_ok() {
                    size += line.len() as u64;
                }
            }
            st.sizes.insert(file.name(), size);
        }
        rec
    }

    pub fn log(&self, level: Level, scope: &str, msg: &str) {
        self.write(LogFile::Main, level, scope, msg, None);
    }
    pub fn debug(&self, scope: &str, msg: &str) {
        self.log(Level::Debug, scope, msg);
    }
    pub fn info(&self, scope: &str, msg: &str) {
        self.log(Level::Info, scope, msg);
    }
    pub fn warn(&self, scope: &str, msg: &str) {
        self.log(Level::Warn, scope, msg);
    }
    pub fn error(&self, scope: &str, msg: &str) {
        self.log(Level::Error, scope, msg);
    }

    /// Logs with structured extra fields (redacted), e.g. `log_with(Info, "torrent", "started", json!({"id": 1}))`.
    pub fn log_with(&self, level: Level, scope: &str, msg: &str, extra: Value) {
        let extra = match redact(&extra) {
            Value::Object(m) => Some(m),
            _ => None,
        };
        self.write(LogFile::Main, level, scope, msg, extra);
    }

    /// Recent warn/error records captured in memory (all files), oldest first.
    pub fn recent(&self, limit: usize) -> Vec<Value> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let skip = st.ring.len().saturating_sub(limit);
        st.ring.iter().skip(skip).cloned().collect()
    }

    /// Parses the last `max_bytes` of a log file (same rules as `readLogTail`).
    pub fn read_tail(&self, file: LogFile, limit: Option<usize>, min: Level, max_bytes: u64) -> Vec<Value> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(self.path(file)) else { return vec![] };
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let start = size.saturating_sub(max_bytes);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
            return vec![];
        }
        let mut text = String::from_utf8_lossy(&buf).into_owned();
        if start > 0 {
            text = text.split_once('\n').map(|x| x.1.to_string()).unwrap_or_default();
        }
        let mut out: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|r| r["lvl"].as_str().and_then(Level::parse).map(|l| l >= min).unwrap_or(false))
            .collect();
        if let Some(n) = limit {
            let skip = out.len().saturating_sub(n);
            out.drain(..skip);
        }
        out
    }
}

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// Process-wide logger (initialised from the environment on first use).
pub fn global() -> &'static Logger {
    GLOBAL.get_or_init(Logger::from_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_ts_compatible_lines() {
        let dir = tempfile::tempdir().unwrap();
        let log = Logger::new(dir.path().join("logs"), Level::Info);
        log.info("boot", "started");
        log.debug("boot", "hidden");
        log.write(LogFile::Ipc, Level::Error, "ipc", "failed", Some(serde_json::from_value(json!({"stack": "s", "msg": "ignored"})).unwrap()));
        log.log_with(Level::Warn, "net", "call", json!({"apiKey": "abc123", "maxTokens": 5, "url": "http://x/?token=zzzz"}));

        let main = std::fs::read_to_string(log.path(LogFile::Main)).unwrap();
        let lines: Vec<Value> = main.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        let r = &lines[0];
        assert_eq!((r["lvl"].as_str(), r["scope"].as_str(), r["msg"].as_str()), (Some("info"), Some("boot"), Some("started")));
        assert_eq!(r["pid"].as_u64(), Some(std::process::id() as u64));
        let t = r["t"].as_str().unwrap();
        assert!(t.ends_with('Z') && t.len() == 24, "{t}");
        assert_eq!(lines[1]["apiKey"], "[REDACTED]");
        assert_eq!(lines[1]["maxTokens"], 5);
        assert_eq!(lines[1]["url"], "http://x/?token=[REDACTED]");

        let ipc = log.read_tail(LogFile::Ipc, None, Level::Warn, 1 << 20);
        assert_eq!(ipc.len(), 1);
        assert_eq!((ipc[0]["stack"].as_str(), ipc[0]["msg"].as_str()), (Some("s"), Some("failed")));
        assert_eq!(log.recent(10).len(), 2);
        assert_eq!(log.read_tail(LogFile::Main, Some(1), Level::Debug, 1 << 20).len(), 1);
    }

    #[test]
    fn redacts_strings() {
        assert_eq!(redact_string("Authorization: Bearer abcdefghijkl"), "Authorization: Bearer [REDACTED]");
        assert_eq!(redact_string("key sk-abcdefghijklmnopqrstu"), "key sk-[REDACTED]");
        assert_eq!(redact_string(r#"{"apiKey":"hunter2"}"#), r#"{"apiKey":"[REDACTED]"}"#);
    }

    #[test]
    fn rotates_and_keeps_three() {
        let dir = tempfile::tempdir().unwrap();
        let log = Logger::new(dir.path(), Level::Info);
        let big = "x".repeat(1024 * 1024);
        for _ in 0..30 {
            log.info("t", &big);
        }
        let base = log.path(LogFile::Main);
        assert!(base.exists());
        for i in 1..=3 {
            assert!(PathBuf::from(format!("{}.{i}", base.display())).exists(), "missing .{i}");
        }
        assert!(!PathBuf::from(format!("{}.4", base.display())).exists());
        assert!(std::fs::metadata(&base).unwrap().len() <= MAX_BYTES + 2 * 1024 * 1024);
    }
}
