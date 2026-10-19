//! Small persistent JSON cache (`~/.mangavox/cache/data`) for catalogs, details and stream links.
//! One file per key: the first line is the unix time it was written, the rest is the JSON value.
//! Readers decide what "stale" means from the returned age (stale-while-revalidate).
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub struct DataCache {
    dir: PathBuf,
    mem: Mutex<HashMap<String, (u64, Arc<str>)>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl DataCache {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), mem: Mutex::new(HashMap::new()) }
    }

    /// The app-wide cache under `~/.mangavox/cache/data`.
    pub fn shared() -> &'static DataCache {
        static C: OnceLock<DataCache> = OnceLock::new();
        C.get_or_init(|| DataCache::at(crate::paths::root().join("cache").join("data")))
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:x}.json", md5::compute(key.as_bytes())))
    }

    fn raw(&self, key: &str) -> Option<(u64, Arc<str>)> {
        if let Some(hit) = self.mem.lock().unwrap().get(key) {
            return Some(hit.clone());
        }
        let text = std::fs::read_to_string(self.file(key)).ok()?;
        let (t, body) = text.split_once('\n')?;
        let hit = (t.trim().parse().ok()?, Arc::<str>::from(body));
        self.mem.lock().unwrap().insert(key.to_string(), hit.clone());
        Some(hit)
    }

    /// Cached value and its age, however old it is.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<(T, Duration)> {
        let (t, body) = self.raw(key)?;
        let v = serde_json::from_str(&body).ok()?;
        Some((v, Duration::from_secs(now().saturating_sub(t))))
    }

    /// Age of the cached entry, without decoding it.
    pub fn age(&self, key: &str) -> Option<Duration> {
        self.raw(key).map(|(t, _)| Duration::from_secs(now().saturating_sub(t)))
    }

    /// Store `value`. Empty lists are skipped: they are usually a transient upstream hiccup.
    pub fn put<T: Serialize>(&self, key: &str, value: &T) {
        let Ok(body) = serde_json::to_string(value) else { return };
        if body == "[]" {
            return;
        }
        let t = now();
        let path = self.file(key);
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&tmp, format!("{t}\n{body}")).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        self.mem.lock().unwrap().insert(key.to_string(), (t, body.into()));
    }

    /// Drop entries not rewritten for `max_age` (titles nobody opens any more).
    pub fn prune(&self, max_age: Duration) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let cutoff = SystemTime::now() - max_age;
        for e in rd.flatten() {
            if e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let c = DataCache::at(dir.path());
        assert!(c.get::<Vec<u32>>("k").is_none());
        c.put("k", &vec![1u32, 2, 3]);
        let (v, age) = c.get::<Vec<u32>>("k").unwrap();
        assert_eq!(v, vec![1, 2, 3]);
        assert!(age < Duration::from_secs(5));
        // A fresh instance (next launch) reads it back from disk.
        let c2 = DataCache::at(dir.path());
        assert_eq!(c2.get::<Vec<u32>>("k").unwrap().0, vec![1, 2, 3]);
        assert!(c2.get::<String>("k").is_none(), "wrong type is a miss");
    }

    #[test]
    fn age_comes_from_the_stored_time() {
        let dir = tempfile::tempdir().unwrap();
        let c = DataCache::at(dir.path());
        std::fs::write(c.file("old"), format!("{}\n\"x\"", now() - 3600)).unwrap();
        let (v, age) = c.get::<String>("old").unwrap();
        assert_eq!(v, "x");
        assert!(age >= Duration::from_secs(3600));
    }
}
