//! The per-manga "brain": a character bank (names, aliases, faces) plus running story notes.
//! Keeps names consistent across pages and chapters (the Magiv2 "character bank" idea).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::store::{studio_root, write_json_atomic};
use crate::types::Character;

const NOTES_MAX: usize = 4000;
const MAX_TRAITS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Brain {
    pub manga_id: String,
    pub manga_title: String,
    pub characters: Vec<Character>,
    /// Running world / plot memory, one `- ` line per entry, oldest first.
    pub notes: String,
    /// Directory override (tests); `None` = [`Brain::dir`].
    #[serde(skip)]
    pub(crate) root: Option<PathBuf>,
}

/// Filesystem-safe form of a manga id.
pub fn safe_id(id: &str) -> String {
    let s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(80).collect();
    if s.is_empty() {
        "manga".into()
    } else {
        s
    }
}

pub(crate) fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(24).collect();
    if s.is_empty() {
        "char".into()
    } else {
        s
    }
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl Brain {
    pub fn dir(manga_id: &str) -> PathBuf {
        studio_root().join(safe_id(manga_id))
    }

    /// Load `brain.json` (or start empty). The title is refreshed from the caller.
    pub fn load(manga_id: &str, title: &str) -> Brain {
        Self::load_in(Self::dir(manga_id), manga_id, title)
    }

    /// Like [`Brain::load`] with an explicit directory.
    pub fn load_in(dir: PathBuf, manga_id: &str, title: &str) -> Brain {
        let mut b: Brain = std::fs::read(dir.join("brain.json")).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        b.manga_id = manga_id.to_string();
        // Face paths are stored absolute; after the data folder moved, find them in this brain's faces/.
        for f in b.characters.iter_mut().flat_map(|c| c.faces.iter_mut()) {
            if !f.is_file() {
                if let Some(p) = f.file_name().map(|n| dir.join("faces").join(n)).filter(|p| p.is_file()) {
                    *f = p;
                }
            }
        }
        if !title.is_empty() {
            b.manga_title = title.to_string();
        }
        b.root = Some(dir);
        b
    }

    pub fn root_dir(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| Self::dir(&self.manga_id))
    }

    pub fn faces_dir(&self) -> PathBuf {
        self.root_dir().join("faces")
    }

    pub fn save(&self) -> Result<()> {
        write_json_atomic(&self.root_dir().join("brain.json"), self)
    }

    fn idx_of(&self, id: &str) -> Option<usize> {
        self.characters.iter().position(|c| c.id == id)
    }

    /// Case-insensitive lookup by name or alias.
    pub fn find(&self, name_or_alias: &str) -> Option<&Character> {
        self.find_idx(name_or_alias).map(|i| &self.characters[i])
    }

    fn find_idx(&self, name: &str) -> Option<usize> {
        let n = name.trim();
        if n.is_empty() {
            return None;
        }
        self.characters.iter().position(|c| eq_ci(&c.name, n)).or_else(|| self.characters.iter().position(|c| c.aliases.iter().any(|a| eq_ci(a, n))))
    }

    /// User rename: locks the character and keeps the old name as an alias.
    pub fn rename(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if let Some(i) = self.idx_of(id).filter(|_| !name.is_empty()) {
            let c = &mut self.characters[i];
            if !eq_ci(&c.name, name) {
                let old = std::mem::replace(&mut c.name, name.to_string());
                c.aliases.retain(|a| !eq_ci(a, name));
                if !c.aliases.iter().any(|a| eq_ci(a, &old)) {
                    c.aliases.push(old);
                }
            }
            c.user_locked = true;
        }
    }

    /// Fold `drop_id` into `keep_id` (same person detected twice).
    pub fn merge(&mut self, keep_id: &str, drop_id: &str) {
        if keep_id == drop_id {
            return;
        }
        let (Some(k), Some(d)) = (self.idx_of(keep_id), self.idx_of(drop_id)) else { return };
        let dropped = self.characters.remove(d);
        let k = if d < k { k - 1 } else { k };
        let keep = &mut self.characters[k];
        let mut names = vec![dropped.name];
        names.extend(dropped.aliases);
        for n in names {
            if !eq_ci(&n, &keep.name) && !keep.aliases.iter().any(|a| eq_ci(a, &n)) {
                keep.aliases.push(n);
            }
        }
        for t in dropped.traits {
            if !keep.traits.contains(&t) && keep.traits.len() < MAX_TRAITS {
                keep.traits.push(t);
            }
        }
        keep.faces.extend(dropped.faces);
        keep.appearances += dropped.appearances;
        if keep.description.is_empty() {
            keep.description = dropped.description;
        }
        keep.first_seen = match (keep.first_seen.take(), dropped.first_seen) {
            (Some(a), Some(b)) => Some(if b.0 == a.0 && b.1 < a.1 { b } else { a }),
            (a, b) => a.or(b),
        };
        keep.user_locked |= dropped.user_locked;
    }

    pub fn remove(&mut self, id: &str) {
        self.characters.retain(|c| c.id != id);
    }

    /// Canonical display name for any known name/alias (unknown names pass through).
    pub fn canonical(&self, name: &str) -> String {
        self.find(name).map(|c| c.name.clone()).unwrap_or_else(|| name.trim().to_string())
    }

    fn new_id(&self, name: &str) -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        loop {
            let mut h = DefaultHasher::new();
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
            N.fetch_add(1, Ordering::Relaxed).hash(&mut h);
            let id = format!("{}-{:04x}", slug(name), h.finish() & 0xffff);
            if self.idx_of(&id).is_none() {
                return id;
            }
        }
    }

    /// Resolve a character the model reported: returns `(id, created)`.
    /// `was` is a placeholder name used earlier that now has a real name.
    pub fn observe(&mut self, name: &str, was: Option<&str>, description: &str, traits: &[String]) -> (String, bool) {
        let name = name.trim();
        let was = was.map(str::trim).filter(|w| !w.is_empty() && !eq_ci(w, name));
        let found = self.find_idx(name);
        let (i, created) = match (found, was.and_then(|w| self.find_idx(w))) {
            (Some(i), _) => (i, false),
            (None, Some(i)) => {
                let c = &mut self.characters[i];
                if c.user_locked {
                    c.aliases.push(name.to_string());
                } else {
                    let old = std::mem::replace(&mut c.name, name.to_string());
                    c.aliases.retain(|a| !eq_ci(a, name));
                    c.aliases.push(old);
                }
                (i, false)
            }
            (None, None) => {
                let id = self.new_id(name);
                self.characters.push(Character { id, name: name.to_string(), ..Default::default() });
                (self.characters.len() - 1, true)
            }
        };
        let c = &mut self.characters[i];
        if let Some(w) = was {
            if !eq_ci(w, &c.name) && !c.aliases.iter().any(|a| eq_ci(a, w)) {
                c.aliases.push(w.to_string());
            }
        }
        let description = description.trim();
        if c.description.is_empty() && !description.is_empty() {
            c.description = description.to_string();
        }
        for t in traits {
            let t = t.trim();
            if !t.is_empty() && c.traits.len() < MAX_TRAITS && !c.traits.iter().any(|x| eq_ci(x, t)) {
                c.traits.push(t.to_string());
            }
        }
        (c.id.clone(), created)
    }

    /// Count one appearance on a page.
    pub fn mark_seen(&mut self, id: &str, chapter_id: &str, page: usize) {
        if let Some(i) = self.idx_of(id) {
            let c = &mut self.characters[i];
            c.appearances += 1;
            if c.first_seen.is_none() {
                c.first_seen = Some((chapter_id.to_string(), page));
            }
        }
    }

    pub(crate) fn character(&self, id: &str) -> Option<&Character> {
        self.idx_of(id).map(|i| &self.characters[i])
    }

    pub(crate) fn add_face(&mut self, id: &str, path: PathBuf) {
        if let Some(i) = self.idx_of(id) {
            self.characters[i].faces.push(path);
        }
    }

    /// Append one memory line, trimming the oldest lines past 4000 chars.
    pub fn add_note(&mut self, line: &str) {
        let line = line.trim().replace('\n', " ");
        if line.is_empty() {
            return;
        }
        if !self.notes.is_empty() && !self.notes.ends_with('\n') {
            self.notes.push('\n');
        }
        self.notes.push_str("- ");
        self.notes.push_str(line.trim_start_matches("- "));
        while self.notes.chars().count() > NOTES_MAX {
            match self.notes.find('\n') {
                Some(p) => {
                    self.notes.drain(..=p);
                }
                None => {
                    let keep: Vec<char> = self.notes.chars().rev().take(NOTES_MAX).collect();
                    self.notes = keep.into_iter().rev().collect();
                    break;
                }
            }
        }
    }

    /// Next unused `faces/<id>_<n>.jpg` path.
    pub(crate) fn next_face_path(&self, id: &str, reserved: &[PathBuf]) -> PathBuf {
        let dir = self.faces_dir();
        (0..).map(|n| dir.join(format!("{id}_{n}.jpg"))).find(|p| !p.exists() && !reserved.contains(p)).unwrap()
    }
}
