//! Crash-safe torrent session state: durable writes, and start-up healing of
//! state files a crash or power cut left empty, zero-filled or truncated.
//!
//! A broken state file must never stop playback: it is moved aside as
//! `<name>.corrupt-<unix-time>` (the newest [`KEEP_CORRUPT`] are kept for
//! diagnosis) and the session starts fresh.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// librqbit's session database (torrent list).
pub(crate) const SESSION_DB: &str = "session.json";
/// librqbit-dht routing table.
pub(crate) const DHT_STATE: &str = "dht.json";
/// The last listen port (written by the engine).
pub(crate) const LISTEN_PORT: &str = "listen-port";

/// Quarantined copies kept per file name.
const KEEP_CORRUPT: usize = 3;
/// Temp files older than this are leftovers of a crashed write.
const STALE_TMP: Duration = Duration::from_secs(10 * 60);

fn log() -> &'static mv_core::logging::Logger {
    mv_core::logging::global()
}

fn warn(msg: &str) {
    tracing::warn!("{msg}");
    log().warn("torrent", msg);
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Replaces `path` with `bytes` so a crash or power cut at any point leaves
/// either the old or the new content, never an empty or zero-filled file:
/// write `<path>.tmp` in the same folder, fsync and close it, then rename it
/// over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = with_suffix(path, ".tmp");
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Moves a broken state file aside as `<name>.corrupt-<unix-time>` and prunes
/// older quarantined copies of it. Returns the new path.
pub(crate) fn quarantine(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut dst = with_suffix(path, &format!(".corrupt-{now}"));
    let mut n = 1;
    while dst.exists() {
        dst = with_suffix(path, &format!(".corrupt-{now}-{n}"));
        n += 1;
    }
    let moved = match fs::rename(path, &dst) {
        Ok(()) => Some(dst),
        Err(e) => {
            // Could not move it (locked?): deleting still unblocks the start.
            warn(&format!("could not quarantine {} ({e}); deleting it", path.display()));
            let _ = fs::remove_file(path);
            None
        }
    };
    prune_quarantined(path);
    moved
}

/// Keeps only the newest [`KEEP_CORRUPT`] `<name>.corrupt-*` copies.
fn prune_quarantined(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    let prefix = format!("{name}.corrupt-");
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut copies: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), e.path()))
        .collect();
    copies.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, p) in copies.into_iter().skip(KEEP_CORRUPT) {
        let _ = fs::remove_file(p);
    }
}

/// Why a state file cannot be used, or `None` when it is fine (or absent).
fn check_file(path: &Path, valid: impl Fn(&[u8]) -> bool) -> Option<String> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("unreadable: {e}")),
    };
    if bytes.is_empty() {
        return Some("empty".into());
    }
    if bytes.iter().all(|&b| b == 0) {
        return Some(format!("{} zero bytes", bytes.len()));
    }
    (!valid(&bytes)).then(|| format!("{} bytes of invalid content", bytes.len()))
}

fn json_object_with(bytes: &[u8], key: Option<&str>) -> bool {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(serde_json::Value::Object(m)) => key.is_none_or(|k| m.get(k).is_some_and(|v| v.is_object())),
        _ => false,
    }
}

pub(crate) fn valid_session_db(bytes: &[u8]) -> bool {
    json_object_with(bytes, Some("torrents"))
}

fn valid_dht_state(bytes: &[u8]) -> bool {
    json_object_with(bytes, None)
}

fn valid_listen_port(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).ok().and_then(|s| s.trim().parse::<u16>().ok()).is_some()
}

/// Quarantines a state file with the reason in the log.
pub(crate) fn quarantine_with_reason(path: &Path, reason: &str) {
    match quarantine(path) {
        Some(dst) => warn(&format!(
            "torrent state file {} was {reason}; moved it to {} and starting fresh",
            path.display(),
            dst.display()
        )),
        None => warn(&format!("torrent state file {} was {reason}; starting fresh", path.display())),
    }
}

/// Checks the session state files before the session starts: any that is
/// empty, zero-filled or not parseable is quarantined. Also removes temp files
/// left behind by writes a crash interrupted. Returns the files quarantined.
pub(crate) fn heal_session_dir(dir: &Path) -> Vec<PathBuf> {
    let mut healed = vec![];
    let checks: [(&str, fn(&[u8]) -> bool); 3] =
        [(SESSION_DB, valid_session_db), (DHT_STATE, valid_dht_state), (LISTEN_PORT, valid_listen_port)];
    for (name, valid) in checks {
        let path = dir.join(name);
        if let Some(reason) = check_file(&path, valid) {
            quarantine_with_reason(&path, &reason);
            healed.push(path);
        }
    }
    remove_stale_temp_files(dir);
    healed
}

/// `session.json.tmp`, `<hash>.torrent.tmp`, `dht.json.tmp.<pid>`, ... older
/// than [`STALE_TMP`] (a running session's in-flight write is never that old).
fn remove_stale_temp_files(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
        let is_tmp = name.ends_with(".tmp") || name.starts_with("dht.json.tmp.");
        if !is_tmp {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= STALE_TMP);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("listen-port");
        write_atomic(&p, b"6881").unwrap();
        write_atomic(&p, b"51413").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"51413");
        assert!(!with_suffix(&p, ".tmp").exists());
    }

    /// A write interrupted before its rename (the power cut case) leaves only
    /// a stray temp file: the real file still holds the old content, and the
    /// next start removes the stray once it is stale.
    #[test]
    fn interrupted_write_keeps_old_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(SESSION_DB);
        write_atomic(&p, br#"{"torrents":{}}"#).unwrap();
        // Simulate the crash: the temp file got (partly) written, the rename never ran.
        let tmp = with_suffix(&p, ".tmp");
        fs::write(&tmp, [0u8; 7]).unwrap();
        assert_eq!(fs::read(&p).unwrap(), br#"{"torrents":{}}"#);
        assert!(heal_session_dir(dir.path()).is_empty(), "the real file is intact");
        // Fresh temp files are left alone (could be a running write) ...
        assert!(tmp.exists());
        // ... stale ones are removed.
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options().write(true).open(&tmp).unwrap().set_modified(old).unwrap();
        heal_session_dir(dir.path());
        assert!(!tmp.exists());
        // A new write over the stray temp file still succeeds.
        fs::write(&tmp, b"junk").unwrap();
        write_atomic(&p, br#"{"torrents":{"0":{}}}"#).unwrap();
        assert_eq!(fs::read(&p).unwrap(), br#"{"torrents":{"0":{}}}"#);
    }

    #[test]
    fn heal_quarantines_broken_files_and_keeps_good_ones() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::write(d.join(SESSION_DB), [0u8; 15]).unwrap(); // what the power cut left
        fs::write(d.join(DHT_STATE), b"{\"addr\":\"[::]:1\",\"tab").unwrap(); // truncated
        fs::write(d.join(LISTEN_PORT), b"6881\n").unwrap(); // fine
        let healed = heal_session_dir(d);
        assert_eq!(healed, vec![d.join(SESSION_DB), d.join(DHT_STATE)]);
        assert!(!d.join(SESSION_DB).exists());
        assert!(!d.join(DHT_STATE).exists());
        assert_eq!(fs::read(d.join(LISTEN_PORT)).unwrap(), b"6881\n");
        let names: Vec<String> = fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        assert!(names.iter().any(|n| n.starts_with("session.json.corrupt-")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("dht.json.corrupt-")), "{names:?}");
    }

    #[test]
    fn heal_flags_empty_garbage_and_wrong_shape() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for bad in [&b""[..], b"garbage", b"[]", b"{\"other\":1}", b"{\"torrents\":[]}"] {
            fs::write(d.join(SESSION_DB), bad).unwrap();
            assert_eq!(heal_session_dir(d), vec![d.join(SESSION_DB)], "{:?}", String::from_utf8_lossy(bad));
        }
        fs::write(d.join(LISTEN_PORT), b"99999").unwrap();
        assert_eq!(heal_session_dir(d), vec![d.join(LISTEN_PORT)]);
    }

    #[test]
    fn quarantine_keeps_only_the_newest_copies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(SESSION_DB);
        for _ in 0..(KEEP_CORRUPT + 3) {
            fs::write(&p, b"x").unwrap();
            assert!(quarantine(&p).is_some());
        }
        let copies = fs::read_dir(dir.path()).unwrap().flatten().filter(|e| {
            e.file_name().to_str().is_some_and(|n| n.starts_with("session.json.corrupt-"))
        });
        assert_eq!(copies.count(), KEEP_CORRUPT);
        assert!(quarantine(&p).is_none(), "nothing to move");
    }
}
