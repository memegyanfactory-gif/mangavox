//! A Creative project: one original manga series, kept in `<data>/creative/<id>/project.json` with
//! its images beside it (`sheets/`, `pages/`, `panels/`, `clips/`, `refs/`). Image paths are
//! stored relative to the project folder, so moving the data folder keeps every image.
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Deserializer, Serialize};

/// Where every project lives.
pub fn root() -> PathBuf {
    mv_core::paths::root().join("creative")
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "Senku Ishigami" → "senku-ishigami": ids and file names.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').chars().take(40).collect::<String>();
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// Numbers the AI may send as text ("3") or not at all.
pub(crate) fn lenient_u32<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.).max(0.) as u32,
        serde_json::Value::String(s) => s.trim().trim_start_matches(|c: char| !c.is_ascii_digit()).split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or(0),
        _ => 0,
    })
}

/// Text the AI may send as a list of lines.
pub(crate) fn lenient_text<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| (!x.is_null()).then(|| x.to_string()))).collect::<Vec<_>>().join("; "),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// A whole page per image when the model can paint a portrait page, else panel by panel.
    #[default]
    Auto,
    /// One image per page: the model draws the panels, balloons and lettering.
    Page,
    /// One image per panel, laid out and lettered here.
    Panels,
}

impl RenderMode {
    pub const ALL: [RenderMode; 3] = [RenderMode::Auto, RenderMode::Page, RenderMode::Panels];
    pub fn label(self) -> &'static str {
        match self {
            RenderMode::Auto => "Auto",
            RenderMode::Page => "Whole page",
            RenderMode::Panels => "Panel by panel",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// An image model id from Studio's catalog.
    pub image_model: String,
    pub resolution: String,
    pub mode: RenderMode,
    /// Colour pages instead of black-and-white screentone.
    pub color: bool,
    /// Japanese reading order (panels right to left).
    pub rtl: bool,
    pub pages_per_chapter: u32,
    pub video_model: String,
    pub clip_seconds: u32,
    /// "16:9" or "9:16".
    pub video_aspect: String,
    /// ComfyUI address and the API-format workflow for the local video model.
    pub comfy_url: String,
    pub comfy_workflow: String,
    /// Live action: real-looking people and places instead of drawn manga (character sheets,
    /// scene pictures and clips), each character keeping their own traits.
    pub live_action: bool,
    /// The voice that speaks the scenes' lines (empty: Studio's voice).
    pub voice: String,
    /// Pictures are made on Magnific's website, in the app's browser window where the user is
    /// signed in (its unlimited offer), instead of through an API key.
    pub web_images: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            image_model: String::new(),
            resolution: "2K".into(),
            mode: RenderMode::Auto,
            color: false,
            rtl: true,
            pages_per_chapter: 18,
            video_model: "local-motion".into(),
            clip_seconds: 5,
            video_aspect: "16:9".into(),
            comfy_url: "http://127.0.0.1:8188".into(),
            comfy_workflow: String::new(),
            live_action: false,
            voice: String::new(),
            web_images: false,
        }
    }
}

/// The series bible: what keeps every chapter and page consistent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Bible {
    #[serde(deserialize_with = "lenient_text")]
    pub logline: String,
    #[serde(deserialize_with = "lenient_text")]
    pub genre: String,
    #[serde(deserialize_with = "lenient_text")]
    pub tone: String,
    #[serde(deserialize_with = "lenient_text")]
    pub themes: String,
    /// The art direction every image prompt carries (linework, screentone, faces, anatomy).
    #[serde(deserialize_with = "lenient_text")]
    pub art_style: String,
    #[serde(deserialize_with = "lenient_text")]
    pub world: String,
    /// The user's own twist on the inspiration.
    #[serde(deserialize_with = "lenient_text")]
    pub twist: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Outfit {
    #[serde(deserialize_with = "lenient_text")]
    pub name: String,
    #[serde(deserialize_with = "lenient_text")]
    pub look: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CastMember {
    pub id: String,
    #[serde(deserialize_with = "lenient_text")]
    pub name: String,
    /// protagonist, deuteragonist, antagonist, supporting, minor.
    #[serde(deserialize_with = "lenient_text")]
    pub role: String,
    #[serde(deserialize_with = "lenient_text")]
    pub age: String,
    /// "male" or "female" (their voice is cast from it; empty: guessed from the description).
    #[serde(deserialize_with = "lenient_text")]
    pub gender: String,
    /// The chapter the character first appears in; their sheets are made before it.
    #[serde(deserialize_with = "lenient_u32")]
    pub intro_chapter: u32,
    #[serde(deserialize_with = "lenient_text")]
    pub personality: String,
    /// Face, hair, eyes, build, height, marks: what must never change.
    #[serde(deserialize_with = "lenient_text")]
    pub appearance: String,
    pub outfits: Vec<Outfit>,
    /// Items and tells that identify them (a lab coat pocket of test tubes, a scar).
    #[serde(deserialize_with = "lenient_text")]
    pub signature: String,
    /// The user's own pictures of this character (relative paths).
    pub user_refs: Vec<String>,
    /// Model sheet: front / side / back turnaround in the first outfit.
    pub sheet: Option<String>,
    /// 3×3 expression sheet.
    pub expressions: Option<String>,
}

impl CastMember {
    pub fn has_sheets(&self) -> bool {
        self.sheet.is_some() && self.expressions.is_some()
    }

    /// How much of the story is theirs: 0 the one or two leads it is about, 1 the main cast,
    /// 2 those who come back now and then, 3 the rest.
    pub fn tier(&self) -> u8 {
        let r = self.role.to_lowercase();
        if r.contains("protagonist") || r.contains("deuteragonist") || r.contains("lead") {
            0
        } else if r.contains("antagonist") || r.contains("main") {
            1
        } else if r.contains("minor") || r.contains("extra") || r.contains("background") {
            3
        } else {
            2
        }
    }

    /// Their age in years, when the script gives a number ("17", "about 40", "8 years old").
    pub fn years(&self) -> Option<u32> {
        let digits: String = self.age.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok().filter(|n| *n > 0 && *n < 130)
    }

    pub fn tier_label(&self) -> &'static str {
        ["Lead", "Main cast", "Recurring", "Minor"][self.tier() as usize]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Location {
    pub id: String,
    #[serde(deserialize_with = "lenient_text")]
    pub name: String,
    #[serde(deserialize_with = "lenient_text")]
    pub look: String,
    pub sheet: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Line {
    #[serde(deserialize_with = "lenient_text")]
    pub speaker: String,
    #[serde(deserialize_with = "lenient_text")]
    pub text: String,
    /// speech, thought, shout, whisper, narration.
    #[serde(deserialize_with = "lenient_text")]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanelChar {
    #[serde(deserialize_with = "lenient_text")]
    pub name: String,
    #[serde(deserialize_with = "lenient_text")]
    pub expression: String,
    #[serde(deserialize_with = "lenient_text")]
    pub outfit: String,
    #[serde(deserialize_with = "lenient_text")]
    pub pose: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Panel {
    /// establishing, wide, medium, close-up, extreme close-up, insert.
    #[serde(deserialize_with = "lenient_text")]
    pub shot: String,
    /// eye level, low angle, high angle, bird's eye, dutch.
    #[serde(deserialize_with = "lenient_text")]
    pub camera: String,
    #[serde(deserialize_with = "lenient_text")]
    pub description: String,
    pub characters: Vec<PanelChar>,
    #[serde(deserialize_with = "lenient_text")]
    pub location: String,
    pub lines: Vec<Line>,
    #[serde(deserialize_with = "lenient_text")]
    pub sfx: String,
    /// How it moves when animated (Video tab).
    #[serde(deserialize_with = "lenient_text")]
    pub motion: String,
    pub image: Option<String>,
    pub clip: Option<String>,
    /// The user's notes for this scene's clip: what to do differently when it is made again.
    #[serde(deserialize_with = "lenient_text")]
    pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Page {
    #[serde(deserialize_with = "lenient_u32")]
    pub number: u32,
    /// A layout id from `layout::TEMPLATES`.
    #[serde(deserialize_with = "lenient_text")]
    pub layout: String,
    #[serde(deserialize_with = "lenient_text")]
    pub beat: String,
    pub panels: Vec<Panel>,
    pub image: Option<String>,
    /// The model that painted it.
    pub model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Chapter {
    #[serde(deserialize_with = "lenient_u32")]
    pub number: u32,
    #[serde(deserialize_with = "lenient_text")]
    pub title: String,
    #[serde(deserialize_with = "lenient_text")]
    pub summary: String,
    pub pages: Vec<Page>,
    pub video: Option<String>,
}

impl Chapter {
    pub fn painted(&self) -> usize {
        self.pages.iter().filter(|p| p.image.is_some()).count()
    }
    /// Cast names in the order they first show up in this chapter's panels.
    pub fn cast_names(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for c in self.pages.iter().flat_map(|p| &p.panels).flat_map(|p| &p.characters) {
            if !c.name.trim().is_empty() && !out.iter().any(|n| n.eq_ignore_ascii_case(c.name.trim())) {
                out.push(c.name.trim().to_string());
            }
        }
        out
    }
}

/// What a chat card offers to do (it spends credits, so the user confirms it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Proposal {
    /// Character model sheets and expression sheets (and the chapter's locations).
    Sheets { names: Vec<String>, #[serde(default)] locations: Vec<String> },
    Pages { chapter: u32, #[serde(default)] pages: Vec<u32> },
    Video { chapter: u32 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    #[default]
    Open,
    Started,
    Dismissed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ChatLine {
    /// "user", "assistant" or "note" (what the app did).
    pub role: String,
    pub text: String,
    pub at: u64,
    pub proposal: Option<Proposal>,
    pub state: CardState,
}

impl ChatLine {
    pub fn new(role: &str, text: impl Into<String>) -> ChatLine {
        ChatLine { role: role.into(), text: text.into(), at: now(), ..Default::default() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    /// What the user asked for, in their words.
    pub brief: String,
    /// Manga it should feel like ("Dr. Stone").
    pub inspirations: Vec<String>,
    /// The downloaded manga this is a live-action adaptation of (its own characters and story
    /// are kept), when it is one.
    pub adapts: Option<String>,
    /// Pages from the user's downloaded manga that set the look (relative, in `refs/`).
    pub style_refs: Vec<String>,
    pub bible: Bible,
    pub cast: Vec<CastMember>,
    pub locations: Vec<Location>,
    pub chapters: Vec<Chapter>,
    pub chat: Vec<ChatLine>,
    pub settings: Settings,
}

impl Project {
    pub fn new(title: &str) -> Project {
        let t = now();
        let base = slug(title);
        let mut id = format!("{base}-{t}");
        let mut n = 1;
        while root().join(&id).exists() {
            n += 1;
            id = format!("{base}-{t}-{n}");
        }
        Project { id, title: title.trim().to_string(), created: t, updated: t, ..Default::default() }
    }

    pub fn dir(&self) -> PathBuf {
        root().join(&self.id)
    }

    /// A stored relative path as a file inside the project folder. Anything that would point
    /// outside it (absolute, `..`, a drive) resolves to a file that does not exist.
    pub fn file(&self, rel: &str) -> PathBuf {
        let p = Path::new(rel);
        if p.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            self.dir().join(p)
        } else {
            self.dir().join("_outside_")
        }
    }

    /// `file` relative to the project folder when it is inside it.
    pub fn rel(&self, file: &Path) -> String {
        file.strip_prefix(self.dir()).unwrap_or(file).to_string_lossy().replace('\\', "/")
    }

    pub fn load(id: &str) -> Result<Project> {
        let path = root().join(id).join("project.json");
        let data = std::fs::read(&path).with_context(|| format!("could not read {}", path.display()))?;
        let mut p: Project = serde_json::from_slice(&data).with_context(|| format!("{} is damaged", path.display()))?;
        p.id = id.to_string();
        Ok(p)
    }

    /// Written to a temporary file first, so a crash never leaves half a project.
    pub fn save(&mut self) -> Result<()> {
        self.updated = now();
        let dir = self.dir();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("project.json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, dir.join("project.json"))?;
        Ok(())
    }

    /// Every project, newest change first.
    pub fn list() -> Vec<Project> {
        let mut out: Vec<Project> = std::fs::read_dir(root())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("project.json").is_file())
            .filter_map(|e| Project::load(&e.file_name().to_string_lossy()).ok())
            .collect();
        out.sort_by_key(|p| std::cmp::Reverse(p.updated));
        out
    }

    pub fn delete(id: &str) -> Result<()> {
        let dir = root().join(id);
        if dir.join("project.json").is_file() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn character(&self, name: &str) -> Option<&CastMember> {
        let n = name.trim();
        self.cast.iter().find(|c| c.name.eq_ignore_ascii_case(n) || c.id == slug(n)).or_else(|| {
            // "Senku" for "Senku Ishigami".
            self.cast.iter().find(|c| c.name.split_whitespace().next().is_some_and(|f| f.eq_ignore_ascii_case(n)))
        })
    }

    pub fn character_mut(&mut self, name: &str) -> Option<&mut CastMember> {
        let id = self.character(name)?.id.clone();
        self.cast.iter_mut().find(|c| c.id == id)
    }

    pub fn location(&self, name: &str) -> Option<&Location> {
        let n = name.trim();
        self.locations.iter().find(|l| l.name.eq_ignore_ascii_case(n) || l.id == slug(n))
    }

    /// The number of the first chapter (an adapted manga may start anywhere, say at 75).
    pub fn first_chapter(&self) -> u32 {
        self.chapters.iter().map(|c| c.number).min().unwrap_or(1)
    }

    /// The cast whose sheets come first: everyone a story opens with, the leads before the main
    /// cast before the rest.
    pub fn opening_cast(&self) -> Vec<String> {
        let first = self.first_chapter().max(self.cast.iter().map(|c| c.intro_chapter).min().unwrap_or(1));
        let mut cast: Vec<&CastMember> = self.cast.iter().filter(|c| c.intro_chapter <= first || c.tier() <= 1).collect();
        cast.sort_by_key(|c| c.tier());
        cast.into_iter().map(|c| c.name.clone()).collect()
    }

    pub fn chapter(&self, n: u32) -> Option<&Chapter> {
        self.chapters.iter().find(|c| c.number == n)
    }

    pub fn chapter_mut(&mut self, n: u32) -> &mut Chapter {
        if let Some(i) = self.chapters.iter().position(|c| c.number == n) {
            return &mut self.chapters[i];
        }
        self.chapters.push(Chapter { number: n, ..Default::default() });
        self.chapters.sort_by_key(|c| c.number);
        let i = self.chapters.iter().position(|c| c.number == n).unwrap_or(0);
        &mut self.chapters[i]
    }

    /// Gives every cast member and location an id, unique and stable.
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for c in &mut self.cast {
            if c.id.is_empty() {
                c.id = slug(&c.name);
            }
            let base = c.id.clone();
            let mut n = 2;
            while !seen.insert(c.id.clone()) {
                c.id = format!("{base}-{n}");
                n += 1;
            }
            if c.intro_chapter == 0 {
                c.intro_chapter = 1;
            }
        }
        let mut seen = std::collections::HashSet::new();
        for l in &mut self.locations {
            if l.id.is_empty() {
                l.id = slug(&l.name);
            }
            let base = l.id.clone();
            let mut n = 2;
            while !seen.insert(l.id.clone()) {
                l.id = format!("{base}-{n}");
                n += 1;
            }
        }
        self.chapters.retain(|c| c.number > 0);
        self.chapters.sort_by_key(|c| c.number);
        self.chapters.dedup_by_key(|c| c.number);
    }

    /// A file name in `sub/` for a new image (never overwrites, so redo keeps the old one until
    /// the project points at the new one).
    pub fn new_image(&self, sub: &str, stem: &str) -> PathBuf {
        let dir = self.dir().join(sub);
        let _ = std::fs::create_dir_all(&dir);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        dir.join(format!("{}-{t}.png", slug(stem)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Senku Ishigami"), "senku-ishigami");
        assert_eq!(slug("  Dr. Stone!! "), "dr-stone");
        assert_eq!(slug("???"), "untitled");
    }

    #[test]
    fn sloppy_ai_json_still_reads() {
        let v = serde_json::json!({
            "name": "Kai", "intro_chapter": "Chapter 3", "appearance": ["spiky white hair", "green eyes"],
            "outfits": [{ "name": "School", "look": "gakuran" }]
        });
        let c: CastMember = serde_json::from_value(v).unwrap();
        assert_eq!(c.intro_chapter, 3);
        assert_eq!(c.appearance, "spiky white hair; green eyes");
        let p: Page = serde_json::from_value(serde_json::json!({ "number": 2.0, "panels": [{ "lines": [{ "speaker": "Kai", "text": "Ten billion percent!" }] }] })).unwrap();
        assert_eq!(p.number, 2);
        assert_eq!(p.panels[0].lines[0].text, "Ten billion percent!");
    }

    #[test]
    fn stored_paths_stay_inside_the_project() {
        let p = Project { id: "x".into(), ..Default::default() };
        assert_eq!(p.file("pages/a.png"), p.dir().join("pages/a.png"));
        for bad in ["C:\\Users\\me\\secret.png", "/etc/passwd", "../other/project.json", "pages/../../x.png"] {
            assert!(p.file(bad).starts_with(p.dir()) && p.file(bad).ends_with("_outside_"), "{bad}");
        }
    }

    #[test]
    fn names_ids_and_chapters() {
        let mut p = Project { cast: vec![CastMember { name: "Kai Tanaka".into(), ..Default::default() }, CastMember { name: "Kai Tanaka".into(), ..Default::default() }], ..Default::default() };
        p.normalize();
        assert_eq!(p.cast[0].id, "kai-tanaka");
        assert_eq!(p.cast[1].id, "kai-tanaka-2");
        assert_eq!(p.cast[0].intro_chapter, 1);
        let aged = |age: &str| CastMember { age: age.into(), ..Default::default() }.years();
        assert_eq!((aged("8 years old"), aged("about 40"), aged("17"), aged("unknown"), aged("")), (Some(8), Some(40), Some(17), None, None));
        let mut late = Project::default();
        late.cast = vec![CastMember { name: "Side".into(), role: "supporting".into(), intro_chapter: 75, ..Default::default() }, CastMember { name: "Hero".into(), role: "protagonist".into(), intro_chapter: 75, ..Default::default() }, CastMember { name: "Later".into(), role: "minor".into(), intro_chapter: 80, ..Default::default() }];
        late.chapter_mut(75);
        late.chapter_mut(76);
        assert_eq!((late.first_chapter(), late.opening_cast()), (75, vec!["Hero".to_string(), "Side".to_string()]));
        let tier = |role: &str| CastMember { role: role.into(), ..Default::default() }.tier();
        assert_eq!((tier("Protagonist"), tier("antagonist"), tier("main"), tier("supporting"), tier(""), tier("minor")), (0, 1, 1, 2, 2, 3));
        assert_eq!(p.character("kai").map(|c| c.id.as_str()), Some("kai-tanaka"));
        p.chapter_mut(3).title = "Three".into();
        p.chapter_mut(1).title = "One".into();
        assert_eq!(p.chapters.iter().map(|c| c.number).collect::<Vec<_>>(), vec![1, 3]);
        let proposal: Proposal = serde_json::from_value(serde_json::json!({ "kind": "pages", "chapter": 1 })).unwrap();
        assert_eq!(proposal, Proposal::Pages { chapter: 1, pages: vec![] });
    }
}
