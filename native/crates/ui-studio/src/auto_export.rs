//! Auto export: a manga's full recap saved into a folder the user picked, by itself, once all
//! the work queued for that manga in Generate is through. It is what the dashboard's Full recap
//! video → Export does, with the same choices, minus the folder prompt. Kept per manga in
//! `studio/auto_export.json`.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

/// The mangas with Auto export on, and the folder each exports into.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AutoExport {
    folders: BTreeMap<String, PathBuf>,
}

impl AutoExport {
    fn path() -> PathBuf {
        mv_core::paths::studio().join("auto_export.json")
    }

    pub fn load() -> AutoExport {
        Self::load_from(&Self::path())
    }

    fn load_from(path: &Path) -> AutoExport {
        let v: Value = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let folders = v
            .get("folders")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, d)| Some((k.clone(), PathBuf::from(d.as_str().filter(|s| !s.is_empty())?)))).collect())
            .unwrap_or_default();
        AutoExport { folders }
    }

    pub fn save(&self) -> std::io::Result<()> {
        *SHARED.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.clone());
        self.save_to(&Self::path())
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let folders: serde_json::Map<String, Value> = self.folders.iter().map(|(k, d)| (k.clone(), json!(d.to_string_lossy()))).collect();
        std::fs::write(path, serde_json::to_string_pretty(&json!({ "folders": folders }))?)
    }

    /// The folder `manga_id` exports into, when Auto export is on for it.
    pub fn folder(&self, manga_id: &str) -> Option<&Path> {
        self.folders.get(manga_id).map(PathBuf::as_path)
    }

    pub fn turn_on(&mut self, manga_id: &str, dir: PathBuf) {
        self.folders.insert(manga_id.to_string(), dir);
    }

    pub fn turn_off(&mut self, manga_id: &str) {
        self.folders.remove(manga_id);
    }
}

/// The setting as last loaded or saved, for readers outside the Studio page.
static SHARED: Mutex<Option<AutoExport>> = Mutex::new(None);

/// The folder `manga_id`'s full recap is exported into after Generate, when Auto export is on.
pub fn folder_for(manga_id: &str) -> Option<PathBuf> {
    let mut g = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(AutoExport::load).folder(manga_id).map(Path::to_path_buf)
}

/// The mangas whose queued work just went through: they had work waiting or running at the last
/// look (`before`) and have none now. `before` is None at the first look (nothing has finished
/// yet, whatever the queue holds).
pub(crate) fn finished(before: Option<&HashSet<String>>, now: &HashSet<String>) -> Vec<String> {
    let Some(before) = before else { return vec![] };
    let mut done: Vec<String> = before.difference(now).cloned().collect();
    done.sort();
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_manga_is_finished_when_its_last_queued_chapter_goes_through() {
        assert_eq!(finished(Some(&set(&["a", "b"])), &set(&["b"])), vec!["a".to_string()]);
        assert!(finished(Some(&set(&["a"])), &set(&["a"])).is_empty(), "still working");
        assert!(finished(Some(&set(&[])), &set(&["a"])).is_empty(), "just started");
    }

    #[test]
    fn nothing_counts_as_finished_at_the_first_look() {
        assert!(finished(None, &set(&[])).is_empty());
        assert!(finished(None, &set(&["a"])).is_empty());
    }

    #[test]
    fn folders_are_kept_per_manga_and_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("mv-auto-export-{}", std::process::id()));
        let file = dir.join("auto_export.json");
        let mut a = AutoExport::default();
        a.turn_on("m1", PathBuf::from(r"D:\Exports\One"));
        a.turn_on("m2", PathBuf::from("/tmp/two"));
        a.turn_off("m2");
        a.save_to(&file).unwrap();
        let b = AutoExport::load_from(&file);
        assert_eq!(b, a);
        assert_eq!(b.folder("m1"), Some(Path::new(r"D:\Exports\One")));
        assert_eq!(b.folder("m2"), None);
        assert_eq!(AutoExport::load_from(&dir.join("missing.json")), AutoExport::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
