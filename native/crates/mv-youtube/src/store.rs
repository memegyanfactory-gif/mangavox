//! Files under `<data>/youtube/`: `account.json`, `settings.json`, `queue.json`. Written atomically
//! (temp file + rename) so a crash never leaves half a file.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::{Account, Settings, UploadItem};

/// `<data>/youtube`
pub fn dir() -> PathBuf {
    mv_core::paths::root().join("youtube")
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Default for Store {
    fn default() -> Self {
        Self::new(dir())
    }
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Missing or unreadable file: `Account::default()`.
    pub fn account(&self) -> Account {
        self.read("account.json")
    }

    pub fn save_account(&self, a: &Account) -> Result<()> {
        self.write("account.json", a)
    }

    pub fn settings(&self) -> Settings {
        self.read("settings.json")
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.write("settings.json", s)
    }

    /// Queue in the order it was saved.
    pub fn queue(&self) -> Vec<UploadItem> {
        self.read("queue.json")
    }

    pub fn save_queue(&self, q: &[UploadItem]) -> Result<()> {
        self.write("queue.json", &q)
    }

    /// Load, change, save the queue in one step (the UI and the background runner both write it).
    pub fn update_queue<T>(&self, f: impl FnOnce(&mut Vec<UploadItem>) -> T) -> Result<T> {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut q = self.queue();
        let out = f(&mut q);
        self.save_queue(&q)?;
        Ok(out)
    }

    fn read<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        std::fs::read(self.root.join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        std::fs::create_dir_all(&self.root).with_context(|| format!("create {}", self.root.display()))?;
        let path = self.root.join(name);
        let tmp = self.root.join(format!("{name}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }
}

/// A new queue item id ("up-<millis>-<n>").
pub fn new_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("up-{millis}-{}", N.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Credentials, Tokens, UploadState};

    #[test]
    fn round_trips() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path().join("yt"));
        assert_eq!(s.account(), Account::default());
        assert_eq!(s.settings(), Settings::default());
        assert!(s.queue().is_empty());

        let a = Account {
            credentials: Credentials { client_id: "id".into(), client_secret: "sec".into() },
            tokens: Some(Tokens { access_token: "a".into(), refresh_token: "r".into(), expires_at: 5, scope: "x".into() }),
            channel_id: Some("UC1".into()),
            channel_title: Some("T".into()),
        };
        s.save_account(&a).unwrap();
        assert_eq!(s.account(), a);
        let st = Settings { publish_hour: 9, ..Settings::default() };
        s.save_settings(&st).unwrap();
        assert_eq!(s.settings(), st);
        let item = UploadItem { id: "x".into(), title: "t".into(), state: UploadState::Scheduled, chapter: Some(2.5), ..Default::default() };
        s.save_queue(std::slice::from_ref(&item)).unwrap();
        assert_eq!(s.queue(), vec![item]);
        assert!(!s.root().join("queue.json.tmp").exists());
    }

    #[test]
    fn update_queue_persists_and_returns() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let n = s.update_queue(|q| {
            q.push(UploadItem { id: "a".into(), ..Default::default() });
            q.len()
        });
        assert_eq!(n.unwrap(), 1);
        assert_eq!(s.queue()[0].id, "a");
    }

    #[test]
    fn corrupt_file_is_default() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("queue.json"), "{nope").unwrap();
        assert!(Store::new(d.path()).queue().is_empty());
    }

    #[test]
    fn ids_are_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("up-"));
    }
}
