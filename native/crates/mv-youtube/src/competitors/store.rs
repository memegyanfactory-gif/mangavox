//! Files of the competitor tracker, written atomically (temp file + rename) and compact: one
//! `channels.json`, one `videos/<channel id>.json` per channel, `runs.json`, `state.json`, and the
//! thumbnails as `thumbs/<video id>/<unix>.jpg`.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{CompChannel, CompVideo, RunSummary, State, ThumbVersion};

/// Runs kept in `runs.json`.
const MAX_RUNS: usize = 1000;

/// Read-modify-write of `channels.json` happens one at a time inside this process.
static CHANNELS_LOCK: Mutex<()> = Mutex::new(());

/// `<data>/youtube/competitors`
pub fn dir() -> PathBuf {
    mv_core::paths::root().join("youtube").join("competitors")
}

#[derive(Clone, Debug)]
pub struct CompStore {
    root: PathBuf,
}

impl Default for CompStore {
    fn default() -> Self {
        Self::new(dir())
    }
}

/// A file-name-safe form of an id (YouTube ids are letters, digits, `-` and `_`).
fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

impl CompStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every tracked channel (hidden ones too); empty when the file is missing or unreadable.
    pub fn channels(&self) -> Vec<CompChannel> {
        self.read("channels.json")
    }

    pub fn save_channels(&self, c: &[CompChannel]) -> Result<()> {
        let _guard = CHANNELS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        self.write("channels.json", c)
    }

    /// Saves a run's channels without losing what the user changed meanwhile: pins and hides on
    /// disk win, and channels added meanwhile are kept.
    pub fn merge_save_channels(&self, mut mine: Vec<CompChannel>) -> Result<()> {
        let _guard = CHANNELS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        for d in self.read::<Vec<CompChannel>>("channels.json") {
            match mine.iter_mut().find(|c| c.id == d.id) {
                Some(c) => {
                    c.pinned = d.pinned;
                    c.hidden = d.hidden;
                }
                None => mine.push(d),
            }
        }
        self.write("channels.json", &mine)
    }

    /// A channel's videos, newest published first.
    pub fn videos(&self, channel_id: &str) -> Vec<CompVideo> {
        let mut v: Vec<CompVideo> = self.read(&format!("videos/{}.json", safe(channel_id)));
        v.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        v
    }

    pub fn save_videos(&self, channel_id: &str, v: &[CompVideo]) -> Result<()> {
        self.write(&format!("videos/{}.json", safe(channel_id)), v)
    }

    /// Passes, oldest first, the last 1000.
    pub fn runs(&self) -> Vec<RunSummary> {
        self.read("runs.json")
    }

    pub fn push_run(&self, r: &RunSummary) -> Result<()> {
        let mut runs = self.runs();
        runs.push(r.clone());
        let extra = runs.len().saturating_sub(MAX_RUNS);
        runs.drain(..extra);
        self.write("runs.json", &runs)
    }

    pub fn state(&self) -> State {
        self.read("state.json")
    }

    pub fn save_state(&self, s: &State) -> Result<()> {
        self.write("state.json", s)
    }

    /// Absolute path of a saved thumbnail.
    pub fn thumb_path(&self, t: &ThumbVersion) -> PathBuf {
        self.root.join(&t.file)
    }

    /// Saves a thumbnail's bytes; the path relative to the folder, for [`ThumbVersion::file`].
    pub fn save_thumb(&self, video_id: &str, at: i64, bytes: &[u8]) -> Result<String> {
        let rel = format!("thumbs/{}/{at}.jpg", safe(video_id));
        let path = self.root.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap_or(&self.root)).with_context(|| format!("create {}", path.display()))?;
        let tmp = path.with_extension("jpg.tmp");
        std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(rel)
    }

    pub fn set_pinned(&self, channel_id: &str, pinned: bool) -> Result<()> {
        self.update_channel(channel_id, |c| c.pinned = pinned)
    }

    /// Hidden channels are kept but never scanned, listed or found again.
    pub fn set_hidden(&self, channel_id: &str, hidden: bool) -> Result<()> {
        self.update_channel(channel_id, |c| c.hidden = hidden)
    }

    fn update_channel(&self, channel_id: &str, f: impl FnOnce(&mut CompChannel)) -> Result<()> {
        let _guard = CHANNELS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut all: Vec<CompChannel> = self.read("channels.json");
        if let Some(c) = all.iter_mut().find(|c| c.id == channel_id) {
            f(c);
            self.write("channels.json", &all)?;
        }
        Ok(())
    }

    fn read<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        std::fs::read(self.root.join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.root.join(name);
        let parent = path.parent().unwrap_or(&self.root);
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(value)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::competitors::{Change, Point};

    fn ch(id: &str) -> CompChannel {
        CompChannel { id: id.into(), title: format!("T {id}"), subs_history: vec![Point { at: 1, value: 2 }], ..Default::default() }
    }

    #[test]
    fn round_trips_in_a_tempdir() {
        let d = tempfile::tempdir().unwrap();
        let s = CompStore::new(d.path().join("comp"));
        assert!(s.channels().is_empty() && s.runs().is_empty());
        assert_eq!(s.state(), State::default());

        s.save_channels(&[ch("UC1"), ch("UC2")]).unwrap();
        assert_eq!(s.channels().len(), 2);
        assert!(!s.root().join("channels.json.tmp").exists());

        let v = |id: &str, at: i64| CompVideo { id: id.into(), channel_id: "UC1".into(), published_at: at, titles: vec![Change { at, value: "t".into() }], ..Default::default() };
        s.save_videos("UC1", &[v("a", 10), v("b", 30), v("c", 20)]).unwrap();
        let ids: Vec<String> = s.videos("UC1").into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["b", "c", "a"]);
        assert!(s.videos("UC9").is_empty());

        let rel = s.save_thumb("vid", 77, b"jpegbytes").unwrap();
        assert_eq!(rel, "thumbs/vid/77.jpg");
        let t = ThumbVersion { file: rel, ..Default::default() };
        assert_eq!(std::fs::read(s.thumb_path(&t)).unwrap(), b"jpegbytes");
    }

    #[test]
    fn pin_and_hide_survive_a_runs_save() {
        let d = tempfile::tempdir().unwrap();
        let s = CompStore::new(d.path());
        s.save_channels(&[ch("UC1")]).unwrap();
        let mine = s.channels();
        s.set_pinned("UC1", true).unwrap();
        s.save_channels(&[ch("UC1"), ch("UC2")]).unwrap();
        s.set_pinned("UC1", true).unwrap();
        s.set_hidden("UC2", true).unwrap();
        // A run that started before the pin saves its older copy plus a new channel.
        let mut stale = mine;
        stale.push(ch("UC3"));
        s.merge_save_channels(stale).unwrap();
        let all = s.channels();
        assert_eq!(all.len(), 3);
        assert!(all.iter().find(|c| c.id == "UC1").unwrap().pinned);
        assert!(all.iter().find(|c| c.id == "UC2").unwrap().hidden);
    }

    #[test]
    fn runs_keep_the_last_thousand() {
        let d = tempfile::tempdir().unwrap();
        let s = CompStore::new(d.path());
        for i in 0..1003 {
            s.push_run(&RunSummary { at: i, ..Default::default() }).unwrap();
        }
        let r = s.runs();
        assert_eq!((r.len(), r[0].at, r[999].at), (1000, 3, 1002));
    }

    #[test]
    fn ids_are_file_safe_and_corrupt_files_are_empty() {
        assert_eq!(safe("../a/b"), "___a_b");
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("channels.json"), "{nope").unwrap();
        assert!(CompStore::new(d.path()).channels().is_empty());
    }
}
