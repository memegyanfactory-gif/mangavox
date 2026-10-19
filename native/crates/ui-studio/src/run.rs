//! Studio's AI runs (Read, Story, page rewrites). One run at a time, app-wide, so it
//! keeps going when the page is left. The page polls [`version`] and re-reads a
//! [`snapshot`] when it changes, like the narration queue in `service`.
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mv_ai::{CancellationToken, Registry};
use mv_recap::{Brain, ChapterRef, ModelChoice, Progress, ReadOptions};

const MAX_LOG: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunKind {
    Read,
    Story,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct RunInfo {
    pub kind: RunKind,
    pub model: String,
    pub state: RunState,
    /// Chapter being worked on, as "Chapter 3" plus its position in the batch.
    pub chapter: Option<String>,
    pub chapter_id: Option<String>,
    pub chapter_pos: (usize, usize),
    pub done: usize,
    pub total: usize,
    pub page: Option<usize>,
    pub logs: VecDeque<String>,
    pub new_characters: Vec<String>,
}

impl RunInfo {
    pub fn running(&self) -> bool {
        self.state == RunState::Running
    }

    /// 0..=1 across the whole batch.
    pub fn progress(&self) -> f32 {
        let (i, n) = self.chapter_pos;
        if n == 0 {
            return 0.0;
        }
        let inner = if self.total > 0 { self.done as f32 / self.total as f32 } else { 0.0 };
        ((i.saturating_sub(1) as f32 + inner) / n as f32).clamp(0.0, 1.0)
    }

    fn log(&mut self, line: impl Into<String>) {
        if self.logs.len() >= MAX_LOG {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }
}

static RUN: Mutex<Option<RunInfo>> = Mutex::new(None);
static CANCEL: Mutex<Option<CancellationToken>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);
static BRAINS: Mutex<Option<HashMap<String, Arc<Mutex<Brain>>>>> = Mutex::new(None);

fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

pub fn version() -> u64 {
    VERSION.load(Ordering::SeqCst)
}

pub fn snapshot() -> Option<RunInfo> {
    RUN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn cancel() {
    if let Some(t) = CANCEL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        t.cancel();
    }
}

fn update(f: impl FnOnce(&mut RunInfo)) {
    if let Some(r) = RUN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        f(r);
    }
    bump();
}

/// The shared brain of a manga; runs and the Brain panel edit the same one.
pub fn brain(manga_id: &str, title: &str) -> Arc<Mutex<Brain>> {
    let mut g = BRAINS.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).entry(manga_id.to_string()).or_insert_with(|| Arc::new(Mutex::new(Brain::load(manga_id, title)))).clone()
}

/// Copy of a brain for rendering.
pub fn brain_snapshot(manga_id: &str, title: &str) -> Brain {
    brain(manga_id, title).lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Edits a brain and saves it.
pub fn edit_brain(manga_id: &str, title: &str, f: impl FnOnce(&mut Brain)) {
    let b = brain(manga_id, title);
    let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g);
    let _ = g.save();
    bump();
}

fn begin(kind: RunKind, model: String, chapters: usize) -> Option<CancellationToken> {
    let mut run = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if run.as_ref().is_some_and(|r| r.running()) {
        return None;
    }
    *run = Some(RunInfo {
        kind,
        model,
        state: RunState::Running,
        chapter: None,
        chapter_id: None,
        chapter_pos: (0, chapters),
        done: 0,
        total: 0,
        page: None,
        logs: VecDeque::new(),
        new_characters: vec![],
    });
    let token = CancellationToken::new();
    *CANCEL.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
    bump();
    Some(token)
}

fn finish(res: anyhow::Result<()>, cancel: &CancellationToken) {
    update(|r| {
        r.state = match res {
            Ok(()) => RunState::Done,
            Err(_) if cancel.is_cancelled() => RunState::Cancelled,
            Err(e) => RunState::Failed(format!("{e:#}")),
        };
        let line = match &r.state {
            RunState::Done => "Finished.".to_string(),
            RunState::Cancelled => "Stopped. Finished pages are saved; Read again to continue.".to_string(),
            RunState::Failed(e) => format!("Failed: {e}"),
            RunState::Running => String::new(),
        };
        r.log(line);
    });
}

fn progress_fn() -> mv_recap::ProgressFn {
    Arc::new(|p: Progress| {
        update(|r| match p {
            Progress::Page { done, total, page } => {
                r.done = done;
                r.total = total;
                r.page = Some(page);
            }
            Progress::Log(l) => r.log(l),
            Progress::Character { name, new } => {
                if new {
                    r.log(format!("New character: {name}"));
                    r.new_characters.push(name);
                }
            }
            Progress::Done | Progress::Failed(_) => {}
        })
    })
}

/// Reads `chapters` one after another; each finished page is saved, and the transcript
/// is refreshed (keeping the user's edits) after every chapter.
pub fn start_read(reg: Arc<Registry>, model: ModelChoice, label: String, chapters: Vec<ChapterRef>, force: bool) -> bool {
    let Some(cancel) = begin(RunKind::Read, label.clone(), chapters.len()) else { return false };
    update(|r| r.log(format!("Reading {} chapter{} with {label}", chapters.len(), if chapters.len() == 1 { "" } else { "s" })));
    mv_ui::bg::runtime().spawn(async move {
        let n = chapters.len();
        let mut res = Ok(());
        for (i, ch) in chapters.into_iter().enumerate() {
            update(|r| {
                r.chapter = Some(ch.label.clone());
                r.chapter_id = Some(ch.chapter_id.clone());
                r.chapter_pos = (i + 1, n);
                r.done = 0;
                r.total = 0;
                r.log(format!("{} · {}", ch.manga_title, ch.label));
            });
            let brain = brain(&ch.manga_id, &ch.manga_title);
            let out = mv_recap::read_chapter(reg.clone(), model.clone(), ch.clone(), brain, ReadOptions { force, pages: None }, progress_fn(), cancel.clone()).await;
            match out {
                Ok(read) => {
                    if let Err(e) = mv_recap::update_transcript(&ch, &read) {
                        update(|r| r.log(format!("Could not save the transcript: {e:#}")));
                    }
                }
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        finish(res, &cancel);
    });
    true
}

/// Re-reads single pages of one chapter (e.g. after the user asks to fix a page).
pub fn start_read_pages(reg: Arc<Registry>, model: ModelChoice, label: String, ch: ChapterRef, pages: Vec<usize>) -> bool {
    let Some(cancel) = begin(RunKind::Read, label, 1) else { return false };
    mv_ui::bg::runtime().spawn(async move {
        update(|r| {
            r.chapter = Some(ch.label.clone());
            r.chapter_id = Some(ch.chapter_id.clone());
            r.chapter_pos = (1, 1);
        });
        let brain = brain(&ch.manga_id, &ch.manga_title);
        let opts = ReadOptions { force: true, pages: Some(pages) };
        let res = match mv_recap::read_chapter(reg, model, ch.clone(), brain, opts, progress_fn(), cancel.clone()).await {
            Ok(read) => mv_recap::update_transcript(&ch, &read).map(|_| ()),
            Err(e) => Err(e),
        };
        finish(res, &cancel);
    });
    true
}

/// Writes the narration for `chapters` (grouped per manga so each uses its own brain) and
/// exports it for the audio / video pipeline.
pub fn start_story(reg: Arc<Registry>, model: ModelChoice, label: String, chapters: Vec<ChapterRef>, styles: Vec<String>) -> bool {
    let Some(cancel) = begin(RunKind::Story, label.clone(), chapters.len()) else { return false };
    let shown = if styles.is_empty() { "Normal narration".to_string() } else { styles.join(", ") };
    update(|r| r.log(format!("Writing the story ({shown}) with {label}")));
    mv_ui::bg::runtime().spawn(async move {
        let mut groups: Vec<(String, String, Vec<ChapterRef>)> = vec![];
        for ch in chapters {
            match groups.iter_mut().find(|g| g.0 == ch.manga_id) {
                Some(g) => g.2.push(ch),
                None => groups.push((ch.manga_id.clone(), ch.manga_title.clone(), vec![ch])),
            }
        }
        let n: usize = groups.iter().map(|g| g.2.len()).sum();
        let mut pos = 0;
        let mut res = Ok(());
        for (manga_id, title, chs) in groups {
            pos += chs.len();
            update(|r| {
                r.chapter = Some(format!("{title} · {} chapter{}", chs.len(), if chs.len() == 1 { "" } else { "s" }));
                r.chapter_id = chs.first().map(|c| c.chapter_id.clone());
                r.chapter_pos = (pos, n);
            });
            let brain = brain(&manga_id, &title);
            match mv_recap::write_story(reg.clone(), model.clone(), chs.clone(), styles.clone(), brain, progress_fn(), cancel.clone()).await {
                Ok(stories) => {
                    for (id, story) in stories {
                        if let Some(ch) = chs.iter().find(|c| c.chapter_id == id) {
                            if let Err(e) = mv_recap::export_for_pipeline(ch, &story) {
                                update(|r| r.log(format!("Could not export {}: {e:#}", ch.label)));
                            }
                        }
                    }
                }
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        finish(res, &cancel);
    });
    true
}
