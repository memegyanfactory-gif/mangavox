//! Disk cache for remote artwork (posters, backdrops, logos, episode stills).
//! Files are `md5(url).<ext>` with the extension taken from the image bytes. A URL is resolved
//! once per session: a file on disk is used from then on, otherwise the caller keeps the remote
//! URL for this session and the file is downloaded in the background for the next launch.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use tokio::sync::Semaphore;

const MAX_BYTES: usize = 15 * 1024 * 1024;

pub struct ImageCache {
    dir: PathBuf,
    /// stem -> file, read from the folder on first use.
    index: OnceLock<Mutex<HashMap<String, PathBuf>>>,
    /// url -> what this session decided to show (keeps a card from switching source mid-session).
    decided: Mutex<HashMap<String, Option<PathBuf>>>,
    /// URLs already queued this session (a failed download is not retried until next launch).
    tried: Mutex<HashSet<String>>,
    permits: Semaphore,
    http: OnceLock<reqwest::Client>,
}

pub fn stem(url: &str) -> String {
    format!("{:x}", md5::compute(url.as_bytes()))
}

/// File extension for the image bytes; `None` for formats we don't keep.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    match b {
        [0xff, 0xd8, 0xff, ..] => Some("jpg"),
        [0x89, b'P', b'N', b'G', ..] => Some("png"),
        [b'G', b'I', b'F', b'8', ..] => Some("gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("webp"),
        _ => None,
    }
}

impl ImageCache {
    pub fn new(dir: impl Into<PathBuf>, concurrency: usize) -> Self {
        Self {
            dir: dir.into(),
            index: OnceLock::new(),
            decided: Mutex::new(HashMap::new()),
            tried: Mutex::new(HashSet::new()),
            permits: Semaphore::new(concurrency.max(1)),
            http: OnceLock::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index(&self) -> &Mutex<HashMap<String, PathBuf>> {
        self.index.get_or_init(|| {
            let mut m = HashMap::new();
            for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
                let p = e.path();
                if let (Some(s), Some(_)) = (p.file_stem().and_then(|s| s.to_str()), p.extension().filter(|x| *x != "part")) {
                    m.insert(s.to_string(), p.clone());
                }
            }
            Mutex::new(m)
        })
    }

    /// Load the folder index now (call from a background thread at startup).
    pub fn warm(&self) {
        let _ = self.index();
    }

    /// Cached file for `url`, if one is on disk (no I/O after the index is loaded).
    pub fn lookup(&self, url: &str) -> Option<PathBuf> {
        self.index().lock().unwrap().get(&stem(url)).cloned()
    }

    /// Session-stable choice for `url`: `(file, first_time)`. `None` means show the remote URL.
    pub fn resolve(&self, url: &str) -> (Option<PathBuf>, bool) {
        if let Some(d) = self.decided.lock().unwrap().get(url) {
            return (d.clone(), false);
        }
        let d = self.lookup(url);
        self.decided.lock().unwrap().insert(url.to_string(), d.clone());
        (d, true)
    }

    /// Claim `url` for download; false when it is cached or was already queued this session.
    pub fn claim(&self, url: &str) -> bool {
        url.starts_with("http") && self.lookup(url).is_none() && self.tried.lock().unwrap().insert(url.to_string())
    }

    fn client(&self) -> &reqwest::Client {
        self.http.get_or_init(|| {
            reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default()
        })
    }

    /// Download a claimed `url` into the cache (at most `concurrency` at a time).
    pub async fn download(&self, url: String) -> Option<PathBuf> {
        let _p = self.permits.acquire().await.ok()?;
        let resp = self.client().get(&url).send().await.ok()?.error_for_status().ok()?;
        let bytes = resp.bytes().await.ok()?;
        if bytes.len() > MAX_BYTES {
            return None;
        }
        let ext = sniff(&bytes)?;
        let s = stem(&url);
        let path = self.dir.join(format!("{s}.{ext}"));
        std::fs::create_dir_all(&self.dir).ok()?;
        let tmp = self.dir.join(format!("{s}.{}.part", std::process::id()));
        std::fs::write(&tmp, &bytes).ok()?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        self.index().lock().unwrap().insert(s, path.clone());
        Some(path)
    }

    /// Mark a file as recently used (eviction is LRU by modification time).
    pub fn touch(path: &Path) {
        let fresh = std::fs::metadata(path).and_then(|m| m.modified()).is_ok_and(|m| m.elapsed().unwrap_or_default() < Duration::from_secs(86_400));
        if !fresh {
            if let Ok(f) = std::fs::File::options().write(true).open(path) {
                let _ = f.set_modified(SystemTime::now());
            }
        }
    }

    /// Delete the least recently used files until the folder is under `max_bytes` (down to 80%),
    /// and anything unused for `max_age`. Returns the bytes left.
    pub fn evict(&self, max_bytes: u64, max_age: Duration) -> u64 {
        let mut files: Vec<(SystemTime, u64, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path()))
            })
            .collect();
        files.sort_by_key(|f| f.0);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let target = if total > max_bytes { max_bytes / 10 * 8 } else { u64::MAX };
        let cutoff = SystemTime::now() - max_age;
        for (m, len, p) in files {
            if total <= target && m >= cutoff {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total -= len;
                if let Some(s) = p.file_stem().and_then(|s| s.to_str()) {
                    self.index().lock().unwrap().remove(s);
                }
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_formats() {
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("jpg"));
        assert_eq!(sniff(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"<html>"), None);
    }

    #[test]
    fn resolve_is_stable_per_session() {
        let dir = std::env::temp_dir().join(format!("mv-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let url = "https://example.com/a.jpg";
        let c = ImageCache::new(&dir, 2);
        assert_eq!(c.resolve(url), (None, true));
        assert!(c.claim(url));
        assert!(!c.claim(url), "queued once per session");
        // Pretend the download finished: this session keeps the remote URL, the next one uses the file.
        let p = dir.join(format!("{}.jpg", stem(url)));
        std::fs::write(&p, [0xff, 0xd8, 0xff]).unwrap();
        c.index().lock().unwrap().insert(stem(url), p.clone());
        assert_eq!(c.resolve(url), (None, false));
        let next = ImageCache::new(&dir, 2);
        assert_eq!(next.resolve(url), (Some(p.clone()), true));
        assert_eq!(next.evict(0, Duration::from_secs(3600)), 0);
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn downloads_real_artwork() {
        let dir = std::env::temp_dir().join(format!("mv-img-net-{}", std::process::id()));
        let c = ImageCache::new(&dir, 2);
        let url = "https://images.metahub.space/poster/small/tt0111161/img";
        assert!(c.claim(url));
        let p = c.download(url.to_string()).await.expect("downloaded");
        assert!(p.exists() && p.extension().is_some());
        assert_eq!(ImageCache::new(&dir, 2).lookup(url), Some(p));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
