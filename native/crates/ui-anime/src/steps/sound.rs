//! Step 7, Sound (design 9.10) [sound]: the episode's script with a voice for every line. W3-S5.
//!
//! The centre is the script, scene by scene: each line with its speaker, feeling, words, a play
//! button and its state (voiced, no voice yet, or outdated when its words, feeling or voice
//! changed after it was voiced). "Voice all lines" voices every line without a current voice in
//! one queue job. The left column is the cast: each character's voice (change it, hear a sample)
//! and which voice engines are on this PC. Opening a line shows it on the right with its voice,
//! "Voice again" and every version it ever had.
//!
//! Every character without a voice gets one from the engine's default cast (Studio's cast first,
//! then distinct installed voices); a voice already chosen is never changed by the screen.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::bail;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::queue::{self, Job, JobKind, JobState};
use mv_anime::voices::{self, NARRATOR};
use mv_anime::{Character, CharVoice, Emotion, EmotionKind, Line, LineKind, Status, Store, TakeSource, VoiceCast};
use mv_studio::tts;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{
    btn, empty_view, error_view, icon_button, icon_ph, icon_tag, inline_alert, menu_button, ph, ph_icon, progress_bar, size, skeleton_block, skeleton_line, spinner, tag, tip, toast, AlertKind,
    ButtonKind, MenuItem, Toast, Tokens, Tone, Weight,
};

use super::shared::{cached_take, scene_order, TakeCache};
use super::{Step, StepCtx};
use crate::backends;
use crate::history::{history_button, History};
use crate::Turns;
use crate::style::{LIST_W, POLL_MS};
use crate::widgets::{
    asks_first, column_body, column_head, confirm_bulk, count, error_block, hover_actions, hue_for, info_line, label_caps, locked_chip, marker, outdated_tag, section, spec, strength_dots, workspace, LineTone,
};

/// The cast's character markers.
const MARKER: f32 = 28.;

// ---------------------------------------------------------------- pure parts

/// A catalog voice's name without its language ("English — Heart (US)" → "Heart (US)").
fn voice_name(id: &str) -> String {
    match tts::find_voice(id) {
        Some(v) => v.label.split_once(" — ").map(|(_, n)| n).unwrap_or(v.label).trim().to_string(),
        None if id.trim().is_empty() => "No voice".into(),
        // A voice this version does not list: never its raw id.
        None => "Voice not in this version".into(),
    }
}

/// What a voice sounds like ("Kokoro · warm female" → "warm female").
fn voice_desc(id: &str) -> String {
    tts::find_voice(id).map(|v| v.desc.split_once(" · ").map(|(_, d)| d).unwrap_or(v.desc).trim().to_string()).unwrap_or_default()
}

/// The engine that speaks a voice, by name ("Kokoro").
fn engine_name(id: &str) -> &'static str {
    tts::engine_label(tts::engine_hint(id))
}

/// "Heart (US) · Kokoro"; "Choose a voice" when none is chosen (Bible › Voices says the same).
fn voice_line(id: &str) -> String {
    if id.trim().is_empty() {
        return "Choose a voice".into();
    }
    format!("{} · {}", voice_name(id), engine_name(id))
}

fn emotion_word(k: EmotionKind) -> &'static str {
    match k {
        EmotionKind::Neutral | EmotionKind::Unknown => "Neutral",
        EmotionKind::Joy => "Joy",
        EmotionKind::Sadness => "Sadness",
        EmotionKind::Anger => "Anger",
        EmotionKind::Fear => "Fear",
        EmotionKind::Surprise => "Surprise",
        EmotionKind::Disgust => "Disgust",
        EmotionKind::Resolve => "Resolve",
        EmotionKind::Pride => "Pride",
        EmotionKind::Grief => "Grief",
        EmotionKind::Despair => "Despair",
        EmotionKind::Relief => "Relief",
        EmotionKind::Awe => "Awe",
        EmotionKind::Tender => "Tender",
        EmotionKind::Menace => "Menace",
    }
}

/// Where a line's voice stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineState {
    /// A sound effect or an empty line: never voiced.
    Silent,
    NoVoice,
    Current,
    /// Its words, feeling or voice changed after it was voiced.
    Outdated,
}

/// `now`: the hash of what would be sent for the line now (`None`: never voiced); `made`: the
/// hash its voice in use was made from; `file`: that voice's file is there. A locked line keeps
/// its voice ("Keep"), so it is never outdated.
fn line_state(now: Option<&str>, made: Option<&str>, file: bool, locked: bool) -> LineState {
    match (now, made) {
        (None, _) => LineState::Silent,
        (Some(_), None) => LineState::NoVoice,
        (Some(_), Some(_)) if !file => LineState::NoVoice,
        (Some(n), Some(m)) if n != m && !locked => LineState::Outdated,
        _ => LineState::Current,
    }
}

/// "Voice all lines" voices it (the engine skips locked lines).
fn needs_voicing(state: LineState, locked: bool) -> bool {
    matches!(state, LineState::NoVoice | LineState::Outdated) && !locked
}

/// Which voice engines are on this PC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Engines {
    kokoro: bool,
    chatterbox: bool,
    piper: bool,
}

impl Engines {
    /// From what is installed (the same answer Bible › Voices reads).
    fn from(v: &backends::InstalledVoices) -> Engines {
        Engines { kokoro: v.kokoro, chatterbox: v.chatterbox, piper: v.piper }
    }

    /// From the ids of the installed voice packs.
    #[cfg(test)]
    fn of(packs: &[&str]) -> Engines {
        Engines {
            kokoro: packs.iter().any(|p| p.starts_with("kokoro")),
            chatterbox: packs.contains(&"chatterbox"),
            piper: packs.iter().any(|p| p.starts_with("piper")),
        }
    }

    fn any(self) -> bool {
        self.kokoro || self.chatterbox || self.piper
    }

    /// The voice's own engine is not installed, so a stand-in speaks it.
    fn stands_in(self, voice: &str) -> bool {
        match tts::engine_hint(voice) {
            "chatterbox" => !self.chatterbox,
            "piper" => !self.piper,
            "kokoro" => !self.kokoro,
            _ => false,
        }
    }
}

/// The voices a character may be given: installed, in the project's language, grouped by engine
/// (Kokoro, Chatterbox, Piper), each as (engine label, voice ids).
#[cfg(test)]
fn voice_options(language: &str, installed: &HashSet<String>) -> Vec<(&'static str, Vec<&'static str>)> {
    backends::InstalledVoices::from_ids(installed).options(language)
}

/// Someone who should have a voice has none: the narrator, a cast row left blank, or a bible
/// character who speaks in `speakers` without a row.
pub(super) fn needs_casting(cast: &VoiceCast, characters: &[Character], speakers: &HashSet<String>) -> bool {
    cast.narrator.voice_id.trim().is_empty()
        || cast.voices.iter().any(|v| v.voice_id.trim().is_empty())
        || characters.iter().any(|c| speakers.contains(&c.id) && !cast.voices.iter().any(|v| v.character == c.id))
}

/// The default cast around every voice already chosen: chosen voices stay as they are (locked or
/// not), blanks get distinct installed voices.
pub(super) fn fill_cast(current: &VoiceCast, characters: &[Character], studio: &mv_recap::series::VoiceCast, installed: &HashSet<String>) -> VoiceCast {
    let mut pinned = current.clone();
    let pin = |v: &mut CharVoice| {
        if !v.voice_id.trim().is_empty() {
            v.locked = true;
        }
    };
    pin(&mut pinned.narrator);
    pinned.voices.iter_mut().for_each(pin);
    let mut cast = voices::recast(&pinned, characters, studio, installed);
    // Put back whether the user chose each voice.
    cast.narrator.locked = current.narrator.locked && !current.narrator.voice_id.trim().is_empty();
    for v in &mut cast.voices {
        v.locked = current.voices.iter().find(|c| c.character == v.character).is_some_and(|c| c.locked && !c.voice_id.trim().is_empty());
    }
    cast
}

/// The latest voice jobs of this episode: the "all lines" batch, each line's own, each
/// character's audition.
#[derive(Clone, Debug, Default)]
struct Jobs {
    all: Option<Job>,
    lines: HashMap<String, Job>,
    auditions: HashMap<String, Job>,
}

fn jobs_of(jobs: &[Job], project: &str, episode: &str) -> Jobs {
    let mut mine: Vec<&Job> = jobs.iter().filter(|j| j.project == project).collect();
    mine.sort_by_key(|j| j.id);
    let mut out = Jobs::default();
    for j in mine {
        match &j.kind {
            JobKind::Voice { episode: e, line } if e == episode => {
                if line.trim().is_empty() || line == "*" {
                    out.all = Some(j.clone());
                } else {
                    out.lines.insert(line.clone(), j.clone());
                }
            }
            JobKind::Audition { character, .. } => {
                out.auditions.insert(character.clone(), j.clone());
            }
            _ => {}
        }
    }
    out
}

fn waiting_or_running(j: Option<&Job>) -> bool {
    j.is_some_and(|j| matches!(j.state, JobState::Queued | JobState::Running | JobState::Paused))
}

/// The error of a failed job the user has not hidden.
fn failed(j: Option<&Job>) -> Option<String> {
    j.filter(|j| j.state == JobState::Failed && !j.hidden).map(|j| j.error.clone().unwrap_or_else(|| "It stopped without saying why.".into()))
}

/// A readable name from a speaker id the bible does not know ("ju-hyuk-s-father-613d" →
/// "Ju hyuk's father"): the trailing code goes, the words stay. Never shows the raw id.
fn speaker_words(id: &str) -> String {
    let mut parts: Vec<&str> = id.split(['-', '_', ' ']).filter(|p| !p.is_empty()).collect();
    if parts.len() > 1 && parts.last().is_some_and(|p| p.len() >= 3 && p.chars().all(|c| c.is_ascii_hexdigit()) && p.chars().any(|c| c.is_ascii_digit())) {
        parts.pop();
    }
    let mut words: Vec<String> = Vec::new();
    for p in parts {
        match words.last_mut() {
            Some(prev) if p.eq_ignore_ascii_case("s") => prev.push_str("'s"),
            _ => words.push(p.to_lowercase()),
        }
    }
    let text = words.join(" ");
    let mut chars = text.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => "Someone".into(),
    }
}

/// "about 354 MB".
fn megabytes(bytes: u64) -> String {
    format!("about {} MB", (bytes as f64 / 1_000_000.).round() as u64)
}

// ---------------------------------------------------------------- reading the episode

/// A line's voice in use.
#[derive(Clone)]
struct TakeInfo {
    file: PathBuf,
    seconds: f32,
    /// The engine that really spoke it.
    engine: String,
}

#[derive(Clone)]
struct LineRow {
    id: String,
    /// "3.4": scene, then the line's place among the scene's spoken lines.
    number: String,
    who: String,
    /// The speaker's colour (bible order); `None` for the narrator.
    hue: Option<usize>,
    thought: bool,
    text: String,
    emotion: Emotion,
    /// The voice the line is spoken with now.
    voice: String,
    state: LineState,
    locked: bool,
    take: Option<TakeInfo>,
    takes: Vec<String>,
    selected: Option<String>,
    override_voice: Option<String>,
    /// The speaker's own voice (what "no override" means).
    cast_voice: String,
}

struct SceneGroup {
    /// "Scene 3", or "Other lines".
    label: String,
    title: String,
    rows: Range<usize>,
}

#[derive(Clone)]
struct CastRow {
    /// Character id, or `NARRATOR`.
    key: String,
    name: String,
    hue: Option<usize>,
    voice: String,
    /// The user chose the voice.
    chosen: bool,
    lines: usize,
    /// The latest sample of the voice in use.
    sample: Option<PathBuf>,
}

struct Data {
    lines: Vec<LineRow>,
    scenes: Vec<SceneGroup>,
    cast: Vec<CastRow>,
    engines: Engines,
    options: Vec<(&'static str, Vec<&'static str>)>,
    /// Speakers who are not in the bible, with their line counts (the narrator speaks them).
    strangers: Vec<(String, usize)>,
    has_story: bool,
    episode_name: String,
    /// Size of the recommended voice pack.
    kokoro_size: u64,
}

enum Load {
    Loading,
    Ready(Box<Data>),
    Failed(String),
}

/// One writer at a time for this screen's edits (story and cast are read fresh under it).
type WriteLock = Arc<Mutex<()>>;

/// Reads the script, gives a voice to everyone who has none, and works out each line's state.
/// Blocking.
fn load(store: &Store, ep: &str, cache: &TakeCache, lock: &WriteLock) -> anyhow::Result<Data> {
    let series = store.series()?;
    let episode = store.episode(ep)?;
    let story = store.story(ep)?;
    let mut bible = store.bible()?;
    let available = backends::installed_voices();
    let engines = Engines::from(&available);
    let installed = available.ids.clone();
    let speakers: HashSet<String> = story.lines.iter().filter_map(|l| l.speaker.clone()).collect();
    if !installed.is_empty() && needs_casting(&bible.cast, &bible.characters, &speakers) {
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let studio = mv_recap::series::VoiceCast::load(&series.manga_id);
        // Written only when it fills something (a write makes every screen read again), and
        // then filled on the cast as it is on disk under its own lock, so a voice changed in
        // Bible meanwhile is kept.
        if fill_cast(&bible.cast, &bible.characters, &studio, &installed) != bible.cast {
            bible.cast = store.update_cast(|c| *c = fill_cast(c, &bible.characters, &studio, &installed))?;
        }
    }
    let cast = &bible.cast;
    let index: HashMap<&str, usize> = bible.characters.iter().enumerate().map(|(i, c)| (c.id.as_str(), i)).collect();
    let name_of = |id: &str| bible.character(id).map(|c| c.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| speaker_words(id));
    let kinds: HashMap<&str, voices::Setting> = story.scenes.iter().map(|s| (s.id.as_str(), voices::Setting::from(s))).collect();
    let story_ids: Vec<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
    let mut named: Vec<String> = Vec::new();
    for l in &story.lines {
        if !named.contains(&l.scene) {
            named.push(l.scene.clone());
        }
    }
    let order = scene_order(&episode.scene_order, &story_ids, &named);

    let mut lines: Vec<LineRow> = Vec::new();
    let mut scenes: Vec<SceneGroup> = Vec::new();
    for (i, scene_id) in order.iter().enumerate() {
        let scene = story.scenes.iter().find(|s| &s.id == scene_id);
        let start = lines.len();
        for l in story.lines.iter().filter(|l| &l.scene == scene_id) {
            let Some(text) = voices::said(l) else { continue };
            let kind = kinds.get(l.scene.as_str()).copied().unwrap_or_default();
            let req = voices::request(store, cast, kind, l);
            let take = l.voice.selected.as_deref().and_then(|id| cached_take(store, cache, id));
            let file = take.as_ref().map(|t| store.path(&t.file)).filter(|p| p.is_file());
            let state = line_state(req.as_ref().map(|r| r.hash.as_str()), take.as_ref().map(|t| t.request_hash.as_str()), file.is_some(), l.voice.locked);
            let info = match (&take, file) {
                (Some(t), Some(file)) => Some(TakeInfo {
                    file,
                    seconds: t.seconds,
                    engine: match &t.source {
                        TakeSource::Voice { engine, .. } => engine.clone(),
                        _ => String::new(),
                    },
                }),
                _ => None,
            };
            let mut plain = l.clone();
            plain.voice.voice_override = None;
            let speaker = l.speaker.as_deref().filter(|s| !s.trim().is_empty());
            let place = lines.len() - start + 1;
            lines.push(LineRow {
                id: l.id.clone(),
                number: format!("{}.{}", i + 1, place),
                who: speaker.map(name_of).unwrap_or_else(|| "Narrator".into()),
                hue: speaker.and_then(|s| index.get(s).copied()),
                thought: l.kind == LineKind::Thought,
                text: text.to_string(),
                emotion: l.emotion.clone(),
                voice: req.map(|r| r.voice).unwrap_or_else(|| voices::voice_for(cast, l).voice_id),
                state,
                locked: l.voice.locked,
                take: info,
                takes: l.voice.takes.clone(),
                selected: l.voice.selected.clone(),
                override_voice: l.voice.voice_override.clone().filter(|o| !o.trim().is_empty()),
                cast_voice: voices::voice_for(cast, &plain).voice_id,
            });
        }
        if lines.len() > start {
            let (label, title) = match scene {
                Some(s) => (format!("Scene {}", i + 1), s.title.trim().to_string()),
                None => ("Other lines".to_string(), String::new()),
            };
            scenes.push(SceneGroup { label, title, rows: start..lines.len() });
        }
    }

    // The cast: the narrator, then everyone who speaks in this episode, in bible order.
    let sample_of = |cv: &CharVoice| -> Option<PathBuf> {
        cv.auditions.iter().rev().filter_map(|id| cached_take(store, cache, id)).find(|t| matches!(&t.source, TakeSource::Voice { voice, .. } if *voice == cv.voice_id)).map(|t| store.path(&t.file)).filter(|p| p.is_file())
    };
    let count_of = |key: Option<&str>| story.lines.iter().filter(|l| voices::said(l).is_some() && l.speaker.as_deref().filter(|s| !s.trim().is_empty()) == key).count();
    let mut rows: Vec<CastRow> = Vec::new();
    let narrated = count_of(None);
    // The voice shown is the one saved in the cast (what Bible › Voices shows too); a row with
    // none says "Choose a voice" instead of borrowing Studio's default.
    if narrated > 0 || speakers.is_empty() {
        let n = &cast.narrator;
        rows.push(CastRow { key: NARRATOR.into(), name: "Narrator".into(), hue: None, voice: n.voice_id.trim().to_string(), chosen: n.locked, lines: narrated, sample: sample_of(n) });
    }
    let keys: Vec<&str> = bible.characters.iter().map(|c| c.id.as_str()).filter(|id| speakers.contains(*id)).collect();
    for key in keys {
        let own = cast.voices.iter().find(|v| v.character == key);
        rows.push(CastRow {
            key: key.to_string(),
            name: name_of(key),
            hue: index.get(key).copied(),
            voice: own.map(|v| v.voice_id.trim().to_string()).unwrap_or_default(),
            chosen: own.is_some_and(|v| v.locked),
            lines: count_of(Some(key)),
            sample: own.and_then(|v| sample_of(v)),
        });
    }
    // Speakers the bible does not know are not part of the cast: the narrator speaks their lines.
    let mut strangers: Vec<(String, usize)> = speakers.iter().filter(|s| !s.trim().is_empty() && !index.contains_key(s.as_str())).map(|s| (speaker_words(s), count_of(Some(s.as_str())))).filter(|(_, n)| *n > 0).collect();
    strangers.sort();
    let episode_name = if episode.title.trim().is_empty() { ep.to_string() } else { episode.title.trim().to_string() };
    Ok(Data {
        lines,
        scenes,
        cast: rows,
        engines,
        options: available.options(&series.settings.language),
        strangers,
        has_story: !story.lines.is_empty(),
        episode_name,
        kokoro_size: mv_studio::voices::find_pack("kokoro").map(|p| p.size()).unwrap_or(0),
    })
}

/// Changes one line of the script, read fresh and written under the story file's own lock, so
/// a job's new voices are kept.
fn edit_line(lock: &WriteLock, store: &Store, ep: &str, id: &str, f: &dyn Fn(&mut Line)) -> anyhow::Result<()> {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    // Checked first, so a line gone meanwhile is said plainly (nothing is written then).
    if !store.story(ep)?.lines.iter().any(|l| l.id == id) {
        bail!("This line is no longer in the script; the story may have been adapted again.");
    }
    store.update_story(ep, |story| {
        if let Some(l) = story.lines.iter_mut().find(|l| l.id == id) {
            f(l);
        }
    })?;
    Ok(())
}

/// Gives `key` (a character id or `NARRATOR`) the voice `voice`, chosen by the user; `None`
/// hands the choice back to the default cast. Written under the cast file's own lock (Bible
/// writes it the same way).
fn edit_cast(lock: &WriteLock, store: &Store, key: &str, voice: Option<&str>) -> anyhow::Result<()> {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    store.update_cast(|cast| set_cast_voice(cast, key, voice))?;
    Ok(())
}

fn set_cast_voice(cast: &mut VoiceCast, key: &str, voice: Option<&str>) {
    let entry: &mut CharVoice = if key == NARRATOR {
        if cast.narrator.character.is_empty() {
            cast.narrator.character = NARRATOR.into();
        }
        &mut cast.narrator
    } else {
        let i = match cast.voices.iter().position(|v| v.character == key) {
            Some(i) => i,
            None => {
                cast.voices.push(CharVoice { character: key.to_string(), speed: 1., ..Default::default() });
                cast.voices.len() - 1
            }
        };
        &mut cast.voices[i]
    };
    match voice {
        Some(v) => {
            if entry.voice_id != v {
                // A clone clip belongs to the voice it was made for.
                entry.reference = None;
            }
            entry.voice_id = v.to_string();
            entry.locked = true;
        }
        None => {
            entry.voice_id.clear();
            entry.reference = None;
            entry.locked = false;
        }
    }
}

fn failure(res: anyhow::Result<anyhow::Result<()>>) -> Option<String> {
    match res {
        Ok(Ok(())) => None,
        Ok(Err(e)) | Err(e) => Some(format!("{e:#}")),
    }
}

fn open_audio_settings(window: &mut Window, cx: &mut App) {
    backends::open_audio_settings(window, cx);
}

// ---------------------------------------------------------------- the screen

pub struct SoundStep {
    ctx: StepCtx,
    store: Store,
    takes: TakeCache,
    writes: WriteLock,
    /// This screen's changes are saved in the order they were made.
    turns: Turns,
    data: Load,
    refresh_error: Option<String>,
    loading: bool,
    again: bool,
    /// (store version, queue version, voice packs version) last seen.
    seen: (u64, u64, u64),
    jobs: Jobs,
    held: bool,
    /// The clip the audio player is playing.
    playing: Option<PathBuf>,
    /// The clip this step started last (stopped when the step goes away).
    started: Option<PathBuf>,
    /// The project changed while this step was not drawn: read it again when it is next drawn.
    stale: bool,
    /// The open line (shown on the right).
    selected: Option<String>,
    history: Option<Entity<History>>,
    history_line: Option<String>,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let store = Store::open(&ctx.project);
        let mut step = SoundStep {
            ctx,
            store,
            takes: Arc::new(Mutex::new(HashMap::new())),
            writes: Arc::new(Mutex::new(())),
            turns: Turns::default(),
            data: Load::Loading,
            refresh_error: None,
            loading: false,
            again: false,
            seen: (mv_anime::store::version(), queue::version(), mv_studio::voices::version()),
            jobs: Jobs::default(),
            held: false,
            playing: None,
            started: None,
            stale: false,
            selected: None,
            history: None,
            history_line: None,
        };
        step.refresh_jobs();
        step.reload(cx);
        step.poll(window, cx);
        step
    })
    .into()
}

impl SoundStep {
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            if this.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = ui_studio::audio::now_playing();
        if now != self.playing {
            self.playing = now;
            cx.notify();
        }
        let v = (mv_anime::store::version(), queue::version(), mv_studio::voices::version());
        if v == self.seen {
            return;
        }
        let (files, jobs, packs) = (v.0 != self.seen.0, v.1 != self.seen.1, v.2 != self.seen.2);
        self.seen = v;
        if jobs {
            self.refresh_jobs();
        }
        if files || packs {
            // Read again when next drawn: a step that is not on screen is never drawn, so it
            // does not re-read the script on every voice a running job writes.
            self.stale = true;
        }
        cx.notify();
    }

    fn refresh_jobs(&mut self) {
        let q = queue::snapshot();
        self.held = q.paused || q.interrupted;
        self.jobs = jobs_of(&q.jobs, &self.ctx.project, &self.ctx.episode);
    }

    /// Reads the script again in the background (a second call while reading runs once more after).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let (store, ep, takes, lock) = (self.store.clone(), self.ctx.episode.clone(), self.takes.clone(), self.writes.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep, &takes, &lock)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    Ok(Ok(d)) => {
                        this.data = Load::Ready(Box::new(d));
                        this.refresh_error = None;
                    }
                    Ok(Err(e)) | Err(e) => {
                        let msg = format!("{e:#}");
                        mv_core::logging::global().warn("anime", &format!("sound: could not read the script: {msg}"));
                        if matches!(this.data, Load::Ready(_)) {
                            this.refresh_error = Some(msg);
                        } else {
                            this.data = Load::Failed(msg);
                        }
                    }
                }
                this.sync_history(cx);
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    fn row(&self, id: &str) -> Option<&LineRow> {
        match &self.data {
            Load::Ready(d) => d.lines.iter().find(|r| r.id == id),
            _ => None,
        }
    }

    /// Keeps the history of the open line in step with it.
    fn sync_history(&mut self, cx: &mut Context<Self>) {
        let Some((id, number, takes, selected)) = self.selected.as_deref().and_then(|id| self.row(id)).map(|r| (r.id.clone(), r.number.clone(), r.takes.clone(), r.selected.clone())) else {
            if matches!(self.data, Load::Ready(_)) {
                self.selected = None;
            }
            self.history = None;
            self.history_line = None;
            return;
        };
        if self.history_line.as_deref() == Some(id.as_str()) {
            if let Some(h) = &self.history {
                h.update(cx, |h, cx| {
                    h.set(takes, selected);
                    cx.notify();
                });
            }
            return;
        }
        let pick = cx.entity().downgrade();
        let on = id.clone();
        let project = self.ctx.project.clone();
        let history = cx.new(|_| {
            History::new(&project, takes, selected).title(format!("Line {number}")).on_select(move |take, _w, cx| {
                let _ = pick.update(cx, |this, cx| this.choose(&on, take, cx));
            })
        });
        self.history = Some(history);
        self.history_line = Some(id);
    }

    fn open_line(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected != id {
            self.selected = id;
            self.sync_history(cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ changes

    /// Runs a file change in the background, after every earlier one (in the order the user
    /// made them, so two quick choices never land the wrong way round); says so when it fails.
    fn save(&mut self, what: &'static str, f: impl FnOnce(&WriteLock, &Store) -> anyhow::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        let (store, lock) = (self.store.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| f(&lock, &store)));
        cx.spawn(async move |this, cx| {
            let res = task.await;
            if let Some(e) = failure(res) {
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("{what}: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
    }

    fn choose(&mut self, line: &str, take: String, cx: &mut Context<Self>) {
        let (ep, id) = (self.ctx.episode.clone(), line.to_string());
        self.save(
            "That version could not be put in use",
            move |lock, store| {
                edit_line(lock, store, &ep, &id, &|l| {
                    if l.voice.takes.contains(&take) {
                        l.voice.selected = Some(take.clone());
                        l.voice.status = Status::NeedsReview;
                    }
                })
            },
            cx,
        );
    }

    fn set_override(&mut self, line: &str, voice: Option<String>, cx: &mut Context<Self>) {
        let (ep, id) = (self.ctx.episode.clone(), line.to_string());
        self.save("The line's voice could not be changed", move |lock, store| edit_line(lock, store, &ep, &id, &|l| l.voice.voice_override = voice.clone()), cx);
    }

    fn set_cast_voice(&mut self, key: &str, voice: Option<&'static str>, cx: &mut Context<Self>) {
        let key = key.to_string();
        self.save("The voice could not be changed", move |lock, store| edit_cast(lock, store, &key, voice), cx);
    }

    fn episode_name(&self) -> String {
        match &self.data {
            Load::Ready(d) => d.episode_name.clone(),
            _ => self.ctx.episode.clone(),
        }
    }

    /// "Voice all lines": a handful start at once; more first ask, with the count and the time
    /// (like every bulk action).
    fn ask_voice_all(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !asks_first(n) {
            self.voice_all(cx);
            return;
        }
        let weak = cx.entity().downgrade();
        let what = format!("Each of the {} without a current voice is spoken with its character's voice. Locked lines stay as they are.", count(n, "line"));
        confirm_bulk(n, "voice", format!("Voice {}?", count(n, "line")), what, format!("Voice {}", count(n, "line")), window, cx, move |_, cx| {
            let _ = weak.update(cx, |this, cx| this.voice_all(cx));
        });
    }

    fn voice_all(&mut self, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        let label = format!("Voice the lines of {}", self.episode_name());
        queue::enqueue(JobKind::Voice { episode: self.ctx.episode.clone(), line: "*".into() }, &self.ctx.project, label);
        self.refresh_jobs();
        cx.notify();
    }

    /// Lines of `rows` that need a voice and are not waiting or being voiced already.
    fn to_voice<'a>(&self, rows: impl IntoIterator<Item = &'a LineRow>, only_outdated: bool) -> Vec<String> {
        rows.into_iter()
            .filter(|r| needs_voicing(r.state, r.locked) && (!only_outdated || r.state == LineState::Outdated) && !waiting_or_running(self.jobs.lines.get(&r.id)))
            .map(|r| r.id.clone())
            .collect()
    }

    /// Voices some lines (a scene, the changed ones), one queue job each; more than a few ask
    /// first with the count, like every bulk action.
    fn ask_voice_lines(&mut self, ids: Vec<String>, what: String, window: &mut Window, cx: &mut Context<Self>) {
        let n = ids.len();
        if n == 0 {
            return;
        }
        if !asks_first(n) {
            self.voice_lines(ids, cx);
            return;
        }
        let weak = cx.entity().downgrade();
        let body = format!("Each is spoken with its character's voice; voices they have now stay in Earlier versions. {what}");
        confirm_bulk(n, "voice", format!("Voice {}?", count(n, "line")), body, format!("Voice {}", count(n, "line")), window, cx, move |_, cx| {
            let ids = ids.clone();
            let _ = weak.update(cx, |this, cx| this.voice_lines(ids, cx));
        });
    }

    fn voice_lines(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        for id in ids {
            let Some((number, again)) = self.row(&id).map(|r| (r.number.clone(), r.take.is_some())) else { continue };
            let label = if again { format!("Voice line {number} again") } else { format!("Voice line {number}") };
            queue::enqueue(JobKind::Voice { episode: self.ctx.episode.clone(), line: id }, &self.ctx.project, label);
        }
        self.refresh_jobs();
        cx.notify();
    }

    fn voice_line(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((number, again)) = self.row(id).map(|r| (r.number.clone(), r.take.is_some())) else { return };
        backends::ensure_runner(cx);
        let label = if again { format!("Voice line {number} again") } else { format!("Voice line {number}") };
        queue::enqueue(JobKind::Voice { episode: self.ctx.episode.clone(), line: id.to_string() }, &self.ctx.project, label);
        self.refresh_jobs();
        cx.notify();
    }

    fn audition(&mut self, key: &str, name: &str, voice: &str, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        queue::enqueue(JobKind::Audition { character: key.to_string(), voice: voice.to_string() }, &self.ctx.project, format!("Sample of {} for {name}", voice_name(voice)));
        self.refresh_jobs();
        cx.notify();
    }

    fn retry_failed(&mut self, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        let ids: Vec<u64> = self.jobs.all.iter().chain(self.jobs.lines.values()).filter(|j| j.state == JobState::Failed && !j.hidden).map(|j| j.id).collect();
        for id in ids {
            queue::retry(id);
        }
    }

    fn play(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        ui_studio::audio::toggle(&file);
        self.playing = ui_studio::audio::now_playing();
        if self.playing.as_deref() == Some(file.as_path()) {
            self.started = Some(file);
        }
        cx.notify();
    }
}

/// The project was closed or another one opened: a line or sample this step started stops with
/// it (a clip another screen plays is left alone).
impl Drop for SoundStep {
    fn drop(&mut self) {
        if let Some(c) = self.started.take() {
            if ui_studio::audio::now_playing().as_deref() == Some(c.as_path()) {
                ui_studio::audio::stop();
            }
        }
    }
}

// ---------------------------------------------------------------- view

/// A whole-step state centred in the page area.
fn centred(el: impl IntoElement) -> Div {
    div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(el)
}

/// The narrator's marker: a book on a neutral disc.
fn narrator_marker(px_size: f32) -> Div {
    div().flex().flex_none().items_center().justify_center().size(px(px_size)).rounded_full().bg(Tokens::active()).child(icon_ph(ph::BOOK_OPEN, Tokens::text_2(), icon_px::SM))
}

/// The box that shows the batch while it waits or runs.
fn progress_card() -> Div {
    div().flex().flex_col().gap(px(space::SM)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border())
}

impl Render for SoundStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
        let body = match &self.data {
            Load::Loading => self.skeleton(window).into_any_element(),
            Load::Failed(e) => centred(error_view(format!("The script could not be read: {e}")).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.data = Load::Loading;
                this.reload(cx);
                cx.notify();
            })))
            .into_any_element(),
            Load::Ready(d) if d.lines.is_empty() => self.no_script(d).into_any_element(),
            Load::Ready(d) => self.ready(d, window, cx).into_any_element(),
        };
        div().size_full().child(body)
    }
}

impl SoundStep {
    fn skeleton(&self, window: &Window) -> Div {
        let list = div().flex().flex_col().size_full().child(column_head("Cast")).child(div().flex().flex_col().gap(px(space::LG)).p(px(space::MD)).children((0..4).map(|_| {
            div().flex().items_center().gap(px(space::SM)).child(skeleton_block(px(MARKER), px(MARKER))).child(skeleton_line(px(LIST_W / 2.)))
        })));
        let script = div().flex().flex_col().size_full().child(column_head("Script")).child(div().flex().flex_col().gap(px(space::LG)).p(px(space::MD)).children((0..8).map(|_| {
            div().flex().items_start().gap(px(space::MD)).child(skeleton_block(px(MARKER), px(MARKER))).child(div().flex().flex_col().flex_1().gap(px(space::XS)).child(skeleton_line(px(LIST_W / 2.))).child(skeleton_line(relative(0.8))))
        })));
        workspace(window, Some(list.into_any_element()), script, None)
    }

    fn no_script(&self, d: &Data) -> Div {
        let go = self.ctx.go.clone();
        let (title, body) = if d.has_story {
            ("Nothing to say yet", "The script of this episode has only sound effects, which are not voiced. Add spoken lines in the Story step.")
        } else {
            ("No script yet", "Sound gives every line of the episode's script a voice. Write the story in the Story step first, then come back.")
        };
        centred(empty_view(ph::WAVEFORM, title, body).action(btn("so-go-story", "Go to Story", ButtonKind::Primary).icon(Step::Story.icon()).on_click(move |_, w, cx| go(Step::Story, w, cx))))
    }

    fn ready(&self, d: &Data, window: &Window, cx: &mut Context<Self>) -> Div {
        let list = self.cast_list(d, cx);
        let script = self.script(d, cx);
        let panel = self.selected.as_deref().and_then(|id| d.lines.iter().find(|r| r.id == id)).map(|r| self.line_panel(d, r, cx).into_any_element());
        workspace(window, Some(list.into_any_element()), script, panel)
    }

    // ------------------------------------------------------------ the cast

    fn cast_list(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let mut body = column_body("so-cast").gap(px(space::LG));
        for (i, c) in d.cast.iter().enumerate() {
            body = body.child(self.cast_row(i, c, d, cx));
        }
        if !d.strangers.is_empty() {
            let who: Vec<String> = d.strangers.iter().map(|(name, n)| format!("{name} ({})", count(*n, "line"))).collect();
            body = body.child(info_line(ph::INFO, LineTone::Neutral, format!("Not in the bible, so the narrator speaks their lines: {}.", who.join(", "))));
        }
        body = body.child(self.engines(d));
        div().flex().flex_col().size_full().child(column_head("Cast").child(spec(count(d.cast.len(), "voice")))).child(body)
    }

    fn cast_row(&self, i: usize, c: &CastRow, d: &Data, cx: &mut Context<Self>) -> Div {
        let mark = match c.hue {
            Some(h) => marker(h, &c.name, MARKER),
            None => narrator_marker(MARKER),
        };
        let weak = cx.entity().downgrade();
        let mut items = Vec::new();
        if d.options.is_empty() {
            items.push(MenuItem::heading("No voices are installed on this PC yet"));
            items.push(MenuItem::new("Open Settings › Audio", |w, cx| backends::open_audio_settings(w, cx)).icon(ph::GEAR));
        } else {
            let (w, key) = (weak.clone(), c.key.clone());
            items.push(
                MenuItem::new("Pick for me", move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.set_cast_voice(&key, None, cx));
                })
                .icon(ph::SPARKLE)
                .checked(!c.chosen),
            );
        }
        for (engine, ids) in &d.options {
            items.push(MenuItem::separator());
            items.push(MenuItem::heading(*engine));
            for id in ids {
                let (w, key, pick) = (weak.clone(), c.key.clone(), *id);
                items.push(
                    MenuItem::new(format!("{} · {}", voice_name(id), voice_desc(id)), move |_, cx| {
                        let _ = w.update(cx, |this, cx| this.set_cast_voice(&key, Some(pick), cx));
                    })
                    .checked(c.chosen && c.voice == *id),
                );
            }
        }
        let picker = menu_button(("so-cast-voice", i), ph::MICROPHONE, format!("The voice of {}", c.name), items).label(voice_line(&c.voice)).small().anchor(Anchor::TopLeft).min_width(260.);
        let busy = self.jobs.auditions.get(&c.key);
        let sampling = waiting_or_running(busy);
        let sample = c.sample.clone().map(|file| {
            let playing = self.playing.as_deref() == Some(file.as_path());
            icon_button(("so-sample", i), if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop".to_string() } else { format!("Hear {} in this voice", c.name) })
                .small()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play(file.clone(), cx)))
        });
        let audition = if sampling {
            div().flex().items_center().justify_center().size(px(MARKER)).child(spinner(("so-sample-spin", i), Tokens::accent(), icon_px::SM)).into_any_element()
        } else if !d.engines.any() {
            icon_button(("so-audition", i), ph::MICROPHONE_STAGE, "Make a sample").small().disabled("Install a voice first").into_any_element()
        } else if c.voice.is_empty() {
            icon_button(("so-audition", i), ph::MICROPHONE_STAGE, "Make a sample").small().disabled("Choose a voice first").into_any_element()
        } else {
            let (key, name, voice) = (c.key.clone(), c.name.clone(), c.voice.clone());
            icon_button(("so-audition", i), ph::MICROPHONE_STAGE, format!("Make a sample: {}'s first line in this voice", c.name))
                .small()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.audition(&key, &name, &voice, cx)))
                .into_any_element()
        };
        // A voice whose engine is missing is said once, above the script, not on every row.
        let note = failed(busy).map(|e| info_line(ph::WARNING_CIRCLE, LineTone::Danger, format!("The sample could not be made: {e}")));
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(mark)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(c.name.clone()))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(count(c.lines, "line"))),
                    )
                    .children(sample)
                    .child(audition),
            )
            .child(div().flex().child(picker))
            .children(note)
    }

    fn engines(&self, d: &Data) -> Div {
        let row = |name: &'static str, on: bool, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(div().flex().flex_col().flex_1().min_w_0().child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(name)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(what)))
                .child(if on { icon_tag(ph::CHECK, "Installed", Tone::Success) } else { tag("Not installed", Tone::Neutral) })
        };
        section("Voice engines on this PC")
            .child(row("Kokoro", d.engines.kokoro, "Natural voices, quick on any PC"))
            .child(row("Chatterbox", d.engines.chatterbox, "Acts each line's feeling; best with a graphics card"))
            .child(row("Piper", d.engines.piper, "Light and very fast"))
            .child(div().child(btn("so-settings", "Voice settings", ButtonKind::Ghost).small().icon(ph::GEAR).tooltip("Download or remove voices in Settings › Audio").on_click(|_, w, cx| open_audio_settings(w, cx))))
    }

    // ------------------------------------------------------------ the script

    fn script(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let todo: Vec<&LineRow> = d.lines.iter().filter(|r| needs_voicing(r.state, r.locked)).collect();
        let voiced = d.lines.iter().filter(|r| r.state == LineState::Current).count();
        let batch = waiting_or_running(self.jobs.all.as_ref());
        let all = btn("so-voice-all", "Voice all lines", ButtonKind::Primary).small().icon(ph::WAVEFORM).loading(batch);
        let all_done = !batch && todo.is_empty() && !d.lines.is_empty() && d.lines.iter().all(|r| r.state == LineState::Current);
        let all = if all_done {
            // Every line has its voice: the primary button moves on, like every step's.
            let go = self.ctx.go.clone();
            btn("so-next", "Continue to Cut", ButtonKind::Primary).small().icon(Step::Cut.icon()).tooltip("Every line has a voice; watch the episode in the Cut step").on_click(move |_, w, cx| go(Step::Cut, w, cx))
        } else if !d.engines.any() {
            all.disabled("Install a voice first: Settings › Audio")
        } else if batch {
            all.tooltip("Voicing the lines now")
        } else if todo.is_empty() {
            all.disabled("Every line has a current voice")
        } else {
            let n = todo.len();
            all.tooltip(format!("Voices the {} without a current voice, in one go", count(n, "line"))).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.ask_voice_all(n, window, cx)))
        };
        let head = column_head("Script").child(spec(format!("{voiced} of {} voiced", d.lines.len()))).child(all);
        let mut body = column_body("so-script").gap(px(space::LG));
        for n in self.notices(d, todo.len(), cx) {
            body = body.child(n);
        }
        for g in &d.scenes {
            body = body.child(self.scene_section(g, d, cx));
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }

    fn notices(&self, d: &Data, todo: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(e) = &self.refresh_error {
            out.push(
                error_block(format!("The latest changes could not be read: {e}"))
                    .child(btn("so-refresh", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))))
                    .into_any_element(),
            );
        }
        if !d.engines.any() {
            out.push(
                inline_alert(
                    AlertKind::Warning,
                    "No voices installed",
                    format!("Sound needs a voice on this PC to speak the lines. Download Kokoro (recommended, {}) in Settings › Audio, then come back.", megabytes(d.kokoro_size)),
                )
                .action("Open Settings › Audio", |_, w, cx| open_audio_settings(w, cx))
                .into_any_element(),
            );
        } else if d.lines.iter().any(|r| tts::engine_hint(&r.voice) == "chatterbox") && !d.engines.chatterbox {
            out.push(
                inline_alert(AlertKind::Warning, "Chatterbox is not installed", "Lines cast with an expressive voice are spoken by a stand-in voice. Install Chatterbox in Settings › Audio to hear them acted.")
                    .action("Open Settings › Audio", |_, w, cx| open_audio_settings(w, cx))
                    .into_any_element(),
            );
        }
        if let Some(j) = self.jobs.all.as_ref().filter(|j| matches!(j.state, JobState::Queued | JobState::Running | JobState::Paused)) {
            out.push(self.batch_card(j).into_any_element());
        } else if let Some(e) = failed(self.jobs.all.as_ref()) {
            out.push(
                inline_alert(AlertKind::Error, "Some lines could not be voiced", e)
                    .action("Try again", cx.listener(|this, _: &ClickEvent, _, cx| this.retry_failed(cx)))
                    .into_any_element(),
            );
        }
        let failed_lines = self.jobs.lines.values().filter(|j| j.state == JobState::Failed && !j.hidden).count();
        if failed_lines > 0 {
            out.push(
                inline_alert(AlertKind::Warning, format!("{} could not be voiced", count(failed_lines, "line")), "Point at the red mark on a line to see why.")
                    .action("Try them again", cx.listener(|this, _: &ClickEvent, _, cx| this.retry_failed(cx)))
                    .into_any_element(),
            );
        }
        let outdated = d.lines.iter().filter(|r| r.state == LineState::Outdated).count();
        let busy = waiting_or_running(self.jobs.all.as_ref());
        if outdated > 0 && !busy {
            let changed = self.to_voice(d.lines.iter(), true);
            let action = (!changed.is_empty() && d.engines.any()).then(|| {
                let n = changed.len();
                btn("so-voice-changed", if n == 1 { "Voice the changed line".to_string() } else { format!("Voice the {n} changed lines") }, ButtonKind::Secondary)
                    .small()
                    .icon(ph::ARROWS_CLOCKWISE)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.ask_voice_lines(changed.clone(), "Only the lines that changed are voiced.".into(), window, cx)))
            });
            out.push(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().min_w_0().child(info_line(
                        ph::ARROWS_CLOCKWISE,
                        LineTone::Warning,
                        format!("{} changed after {} voiced; their current voices stay in Earlier versions.", count(outdated, "line"), if outdated == 1 { "it was" } else { "they were" }),
                    )))
                    .children(action)
                    .into_any_element(),
            );
        }
        if todo == 0 && !busy && d.engines.any() && d.lines.iter().all(|r| r.state == LineState::Current) {
            out.push(info_line(ph::CHECK_CIRCLE, LineTone::Success, "Every line has a voice. Watch the episode with its voices in the Cut step.").into_any_element());
        }
        out
    }

    fn batch_card(&self, j: &Job) -> Div {
        let id = j.id;
        let card = progress_card();
        match j.state {
            JobState::Running => {
                let pct = (j.progress.clamp(0., 1.) * 100.).round() as u32;
                let line = if j.line.trim().is_empty() { "Starting the voice engine…".to_string() } else { j.line.clone() };
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(spinner("so-batch-spin", Tokens::accent(), icon_px::SM))
                        .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Voicing the lines"))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{pct}%"))),
                )
                .child(progress_bar("so-batch-progress", j.progress, !self.held))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(line))
                        .child(btn("so-batch-stop", "Stop", ButtonKind::Ghost).small().icon(ph::STOP).tooltip("Stop; the lines voiced so far are kept").on_click(move |_, _, _| queue::cancel(id))),
                )
            }
            _ => {
                let paused = j.state == JobState::Paused;
                let text = if paused {
                    "The voices are paused in the queue"
                } else if self.held {
                    "Waiting: the queue is paused"
                } else {
                    "Waiting for its turn in the queue"
                };
                let mut row = div().flex().gap(px(space::SM));
                if paused {
                    row = row.child(btn("so-batch-resume", "Resume", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(move |_, _, cx| {
                        backends::ensure_runner(cx);
                        queue::resume(id);
                    }));
                } else if self.held {
                    row = row.child(btn("so-batch-resume-all", "Resume the queue", ButtonKind::Secondary).small().icon(ph::PLAY).on_click(|_, _, cx| {
                        backends::ensure_runner(cx);
                        queue::resume_all();
                    }));
                }
                row = row.child(btn("so-batch-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id)));
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::SM))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(text)),
                )
                .child(row)
            }
        }
    }

    fn scene_section(&self, g: &SceneGroup, d: &Data, cx: &mut Context<Self>) -> Div {
        let rows = &d.lines[g.rows.clone()];
        let voiced = rows.iter().filter(|r| r.state == LineState::Current).count();
        // "Voice this scene": its lines without a current voice (not the whole episode).
        let todo = if waiting_or_running(self.jobs.all.as_ref()) || !d.engines.any() { Vec::new() } else { self.to_voice(rows.iter(), false) };
        let voice_scene = (!todo.is_empty()).then(|| {
            let n = todo.len();
            let what = if g.title.is_empty() { format!("Only the lines of {} are voiced.", g.label.to_lowercase()) } else { format!("Only the lines of {} ({}) are voiced.", g.label.to_lowercase(), g.title) };
            btn(SharedString::from(format!("so-voice-scene-{}", g.rows.start)), "Voice this scene", ButtonKind::Ghost)
                .small()
                .icon(ph::WAVEFORM)
                .tooltip(format!("Voices the {} of this scene without a current voice", count(n, "line")))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.ask_voice_lines(todo.clone(), what.clone(), window, cx)))
        });
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::SM))
            .child(label_caps(&g.label))
            .when(!g.title.is_empty(), |h| h.child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(g.title.clone())))
            .child(div().flex_1())
            .children(voice_scene)
            .child(spec(format!("{voiced} of {} voiced", rows.len())));
        let mut lines = div().flex().flex_col().gap(px(space::XXS));
        for i in g.rows.clone() {
            lines = lines.child(self.line_row(i, &d.lines[i], cx));
        }
        div().flex().flex_col().gap(px(space::XS)).child(head).child(lines)
    }

    fn line_row(&self, i: usize, r: &LineRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let on = self.selected.as_deref() == Some(r.id.as_str());
        let group = SharedString::from(format!("so-line-{i}"));
        let own = self.jobs.lines.get(&r.id);
        let play = match &r.take {
            Some(t) => {
                let file = t.file.clone();
                let playing = self.playing.as_deref() == Some(file.as_path());
                icon_button(("so-play", i), if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop" } else { "Play this line" }).small().selected(playing).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.play(file.clone(), cx);
                }))
            }
            None => icon_button(("so-play", i), ph::PLAY, "Play this line").small().disabled("Not voiced yet"),
        };
        let who_colour = match r.hue {
            Some(h) => Tokens::hue(hue_for(h)),
            None => Tokens::text_2(),
        };
        let feeling = (!matches!(r.emotion.kind, EmotionKind::Neutral | EmotionKind::Unknown)).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(space::XS))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(emotion_word(r.emotion.kind)))
                .child(strength_dots(r.emotion.strength.clamp(1, 3), 3))
        });
        let top = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(div().flex_none().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(who_colour).child(r.who.clone()))
            .when(r.thought, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("thinking")))
            .children(feeling)
            .child(div().flex_1())
            .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(r.number.clone()));
        let words = div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(r.text.clone());
        let mark = self.line_mark(i, r, own);
        let mut actions = hover_actions(group.clone()).p(px(space::XXS)).rounded(px(radius::MD)).bg(Tokens::popover());
        let busy = waiting_or_running(own);
        if engines_ready(self) && !busy {
            let id = r.id.clone();
            actions = actions.child(
                icon_button(("so-again", i), ph::ARROW_CLOCKWISE, if r.take.is_some() { "Voice this line again" } else { "Voice this line" }).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.voice_line(&id, cx);
                })),
            );
        }
        let id = r.id.clone();
        actions = actions.child(history_button(("so-versions", i), on).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            let next = if this.selected.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
            this.open_line(next, cx);
        })));
        let open = r.id.clone();
        div()
            .id(("so-line", i))
            .relative()
            .group(group)
            .flex()
            .items_start()
            .gap(px(space::MD))
            .px(px(space::SM))
            .py(px(space::SM))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .map(|d| if on { d.bg(Tokens::active()) } else { d.hover(|s| s.bg(Tokens::hover())) })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_line(Some(open.clone()), cx)))
            .child(play)
            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(space::XXS)).child(top).child(words))
            .child(div().flex_none().pt(px(space::XXS)).child(mark))
            .child(actions)
    }

    /// What a line needs now, at the right of its row.
    fn line_mark(&self, i: usize, r: &LineRow, own: Option<&Job>) -> AnyElement {
        let in_batch = waiting_or_running(self.jobs.all.as_ref()) && needs_voicing(r.state, r.locked);
        if let Some(j) = own.filter(|j| j.state == JobState::Running) {
            let line = if j.line.trim().is_empty() { "Voicing it now".to_string() } else { j.line.clone() };
            return div().id(("so-mark", i)).flex().items_center().gap(px(space::XS)).tooltip(tip(line)).child(spinner(("so-line-spin", i), Tokens::accent(), icon_px::SM)).into_any_element();
        }
        if waiting_or_running(own) || in_batch {
            return div().id(("so-mark", i)).tooltip(tip("Waiting to be voiced")).child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::SM)).into_any_element();
        }
        if let Some(e) = failed(own) {
            return div()
                .id(("so-mark", i))
                .flex()
                .items_center()
                .justify_center()
                .size(px(space::XL))
                .rounded_full()
                .bg(Tokens::danger_soft())
                .tooltip(tip(format!("Could not be voiced: {e}")))
                .child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM))
                .into_any_element();
        }
        let seconds = r.take.as_ref().map(|t| t.seconds).filter(|s| s.is_finite() && *s > 0.).map(|s| spec(format!("{s:.1} s")));
        let base = div().flex().items_center().gap(px(space::XS)).when(r.locked, |d| d.child(locked_chip()));
        match r.state {
            LineState::Outdated => base.child(div().id(("so-mark", i)).tooltip(tip("Its words, feeling or voice changed after it was voiced")).child(outdated_tag())).into_any_element(),
            LineState::NoVoice => base.child(tag("No voice yet", Tone::Neutral)).into_any_element(),
            _ => base.children(seconds).into_any_element(),
        }
    }

    // ------------------------------------------------------------ the open line

    fn line_panel(&self, d: &Data, r: &LineRow, cx: &mut Context<Self>) -> Div {
        let close = icon_button("so-panel-close", ph::X, "Close").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_line(None, cx)));
        let head = column_head(&format!("Line {}", r.number)).child(close);
        let who = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(match r.hue {
                Some(h) => marker(h, &r.who, MARKER),
                None => narrator_marker(MARKER),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(r.who.clone()))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if matches!(r.emotion.kind, EmotionKind::Neutral | EmotionKind::Unknown) {
                        "No particular feeling".to_string()
                    } else {
                        format!("{} · {}", emotion_word(r.emotion.kind), ["mild", "medium", "strong"][(r.emotion.strength.clamp(1, 3) - 1) as usize])
                    })),
            );
        let words = div().text_size(px(size::MD)).line_height(px(size::MD + space::SM)).text_color(Tokens::text_1()).whitespace_normal().child(r.text.clone());

        // The voice: the speaker's own, or one for this line only.
        let weak = cx.entity().downgrade();
        let mut items = Vec::new();
        {
            let (w, id) = (weak.clone(), r.id.clone());
            items.push(
                MenuItem::new(format!("{}'s voice ({})", r.who, voice_name(&r.cast_voice)), move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.set_override(&id, None, cx));
                })
                .icon(ph::USER)
                .checked(r.override_voice.is_none()),
            );
        }
        for (engine, ids) in &d.options {
            items.push(MenuItem::separator());
            items.push(MenuItem::heading(*engine));
            for v in ids {
                let (w, id, pick) = (weak.clone(), r.id.clone(), v.to_string());
                items.push(
                    MenuItem::new(format!("{} · {}", voice_name(v), voice_desc(v)), move |_, cx| {
                        let _ = w.update(cx, |this, cx| this.set_override(&id, Some(pick.clone()), cx));
                    })
                    .checked(r.override_voice.as_deref() == Some(*v)),
                );
            }
        }
        let picker = menu_button("so-line-voice", ph::MICROPHONE, "The voice of this line", items).label(voice_line(&r.voice)).small().anchor(Anchor::TopLeft).min_width(260.);
        let mut voice = section("Voice").child(div().flex().child(picker));
        if r.override_voice.is_some() {
            voice = voice.child(info_line(ph::INFO, LineTone::Neutral, format!("Only this line uses it; {}'s other lines keep their own voice.", r.who)));
        }
        if let Some(t) = &r.take {
            let expected = tts::engine_hint(&r.voice);
            let spoken = t.engine.trim();
            if !spoken.is_empty() && spoken != "fake" && !spoken.starts_with(expected) {
                voice = voice.child(info_line(ph::WARNING, LineTone::Warning, format!("This version was spoken by a stand-in ({}) because {} was not available.", tts::engine_label(spoken), engine_name(&r.voice))));
            }
        } else if d.engines.stands_in(&r.voice) {
            voice = voice.child(info_line(ph::WARNING, LineTone::Warning, format!("{} is not installed, so a stand-in voice will speak it.", engine_name(&r.voice))));
        }

        let own = self.jobs.lines.get(&r.id);
        let busy = waiting_or_running(own) || (waiting_or_running(self.jobs.all.as_ref()) && needs_voicing(r.state, r.locked));
        let play = match &r.take {
            Some(t) => {
                let file = t.file.clone();
                let playing = self.playing.as_deref() == Some(file.as_path());
                btn("so-panel-play", if playing { "Stop" } else { "Play" }, ButtonKind::Secondary).small().icon(if playing { ph::STOP } else { ph::PLAY }).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play(file.clone(), cx)))
            }
            None => btn("so-panel-play", "Play", ButtonKind::Secondary).small().icon(ph::PLAY).disabled("Not voiced yet"),
        };
        let again = btn("so-panel-again", if r.take.is_some() { "Voice again" } else { "Voice it" }, ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).loading(busy);
        let again = if !d.engines.any() {
            again.disabled("Install a voice first")
        } else if busy {
            again
        } else {
            let id = r.id.clone();
            again.tooltip("Make a new version; the one in use stays in Earlier versions").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.voice_line(&id, cx)))
        };
        let state = match r.state {
            LineState::Outdated => Some(info_line(ph::ARROWS_CLOCKWISE, LineTone::Warning, "Outdated: its words, feeling or voice changed after it was voiced. Voice it again to update it.")),
            LineState::NoVoice => Some(info_line(ph::INFO, LineTone::Neutral, "Not voiced yet.")),
            _ => r.take.as_ref().map(|t| info_line(ph::CHECK_CIRCLE, LineTone::Success, format!("Voiced · {:.1} s", t.seconds))),
        };
        let mut body = column_body("so-panel")
            .gap(px(space::LG))
            .child(div().flex().flex_col().gap(px(space::MD)).child(who).child(words).children(state))
            .child(div().flex().flex_wrap().gap(px(space::SM)).child(play).child(again))
            .child(voice);
        if let Some(h) = &self.history {
            body = body.child(h.clone());
        }
        div().flex().flex_col().size_full().child(head).child(body)
    }
}

/// A voice engine is installed (the screen's data is read).
fn engines_ready(s: &SoundStep) -> bool {
    matches!(&s.data, Load::Ready(d) if d.engines.any())
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{emotion_word, fill_cast, jobs_of, line_state, megabytes, needs_casting, needs_voicing, scene_order, voice_name, voice_options, Engines, LineState};
    use mv_anime::queue::{Job, JobKind, JobState};
    use mv_anime::{CharVoice, Character, EmotionKind, VoiceCast};
    use std::collections::HashSet;

    #[test]
    fn a_line_is_outdated_only_when_what_makes_it_changed() {
        assert_eq!(line_state(None, Some("a"), true, false), LineState::Silent);
        assert_eq!(line_state(Some("a"), None, false, false), LineState::NoVoice);
        assert_eq!(line_state(Some("a"), Some("a"), false, false), LineState::NoVoice, "its file is gone");
        assert_eq!(line_state(Some("a"), Some("a"), true, false), LineState::Current);
        assert_eq!(line_state(Some("b"), Some("a"), true, false), LineState::Outdated);
        assert_eq!(line_state(Some("b"), Some("a"), true, true), LineState::Current, "a kept line stays as it is");
        assert!(needs_voicing(LineState::NoVoice, false));
        assert!(needs_voicing(LineState::Outdated, false));
        assert!(!needs_voicing(LineState::NoVoice, true), "the engine skips locked lines");
        assert!(!needs_voicing(LineState::Current, false));
        assert!(!needs_voicing(LineState::Silent, false));
    }

    #[test]
    fn voices_in_plain_words() {
        assert_eq!(voice_name("af_heart"), "Heart (US)");
        assert_eq!(voice_name("someone_new"), "Voice not in this version", "never a raw id");
        assert_eq!(super::speaker_words("ju-hyuk-s-father-613d"), "Ju hyuk's father");
        assert_eq!(super::speaker_words("debt_collector"), "Debt collector");
        assert_eq!(super::speaker_words("--"), "Someone");
        assert_eq!(voice_name(""), "No voice");
        assert_eq!(emotion_word(EmotionKind::Fear), "Fear");
        assert_eq!(megabytes(353_746_785), "about 354 MB");
    }

    #[test]
    fn engines_come_from_the_installed_packs() {
        let e = Engines::of(&["kokoro-lite", "piper-en-amy"]);
        assert!(e.kokoro && e.piper && !e.chatterbox && e.any());
        assert!(e.stands_in("cb_narrator_m"));
        assert!(!e.stands_in("af_heart"));
        assert!(!Engines::of(&[]).any());
    }

    #[test]
    fn only_installed_voices_of_the_language_are_offered() {
        let installed: HashSet<String> = ["af_heart", "am_michael", "hf_alpha", "cb_narrator_m"].iter().map(|s| s.to_string()).collect();
        let opts = voice_options("en", &installed);
        let ids: Vec<&str> = opts.iter().flat_map(|(_, v)| v.iter().copied()).collect();
        assert!(ids.contains(&"af_heart") && ids.contains(&"am_michael") && ids.contains(&"cb_narrator_m"));
        assert!(!ids.contains(&"hf_alpha"), "a Hindi voice is not offered for English");
        assert!(!ids.contains(&"bf_emma"), "a voice that is not installed is not offered");
        assert_eq!(opts.first().map(|(e, _)| *e), Some("Kokoro"));
        assert!(voice_options("en", &HashSet::new()).is_empty());
    }

    fn character(id: &str, name: &str) -> Character {
        Character { id: id.into(), name: name.into(), ..Default::default() }
    }

    #[test]
    fn casting_fills_blanks_and_keeps_every_chosen_voice() {
        let chars = vec![character("kai", "Kai"), character("mina", "Mina"), character("old", "Old man")];
        let installed: HashSet<String> = ["am_michael", "am_adam", "af_heart", "af_bella", "am_eric"].iter().map(|s| s.to_string()).collect();
        let current = VoiceCast {
            narrator: CharVoice { character: "narrator".into(), voice_id: "am_eric".into(), ..Default::default() },
            voices: vec![
                CharVoice { character: "kai".into(), voice_id: "am_adam".into(), ..Default::default() },
                CharVoice { character: "mina".into(), voice_id: "af_bella".into(), locked: true, ..Default::default() },
                CharVoice { character: "old".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        let speakers: HashSet<String> = ["kai", "mina", "old"].iter().map(|s| s.to_string()).collect();
        assert!(needs_casting(&current, &chars, &speakers));
        let cast = fill_cast(&current, &chars, &mv_recap::series::VoiceCast::default(), &installed);
        let voice = |id: &str| cast.voices.iter().find(|v| v.character == id).map(|v| (v.voice_id.clone(), v.locked));
        assert_eq!(voice("kai"), Some(("am_adam".into(), false)), "an automatic choice is not redone");
        assert_eq!(voice("mina"), Some(("af_bella".into(), true)), "the user's choice stays theirs");
        let old = voice("old").unwrap_or_default();
        assert!(!old.0.is_empty() && !old.1, "a blank gets a voice, not marked as chosen");
        assert_eq!(cast.narrator.voice_id, "am_eric");
        assert!(!needs_casting(&cast, &chars, &speakers));
        // A speaker of the bible with no row needs casting too.
        let partial = VoiceCast { narrator: cast.narrator.clone(), voices: cast.voices[..1].to_vec(), ..Default::default() };
        assert!(needs_casting(&partial, &chars, &speakers));
    }

    #[test]
    fn voice_jobs_of_this_episode() {
        let job = |id, kind| Job { id, project: "p".into(), kind, state: JobState::Queued, ..Default::default() };
        let jobs = vec![
            job(1, JobKind::Voice { episode: "ep01".into(), line: "*".into() }),
            job(2, JobKind::Voice { episode: "ep01".into(), line: "ln1".into() }),
            job(3, JobKind::Voice { episode: "ep01".into(), line: "ln1".into() }),
            job(4, JobKind::Voice { episode: "ep02".into(), line: "*".into() }),
            job(5, JobKind::Audition { character: "kai".into(), voice: "am_adam".into() }),
            Job { id: 6, project: "q".into(), kind: JobKind::Voice { episode: "ep01".into(), line: "".into() }, ..Default::default() },
        ];
        let j = jobs_of(&jobs, "p", "ep01");
        assert_eq!(j.all.map(|j| j.id), Some(1));
        assert_eq!(j.lines.get("ln1").map(|j| j.id), Some(3), "the latest job of a line");
        assert_eq!(j.auditions.get("kai").map(|j| j.id), Some(5));
    }

    #[test]
    fn scenes_keep_the_episode_order_then_the_rest() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(scene_order(&[], &s(&["a", "b"]), &s(&["b", "x"])), s(&["a", "b", "x"]));
    }
}
