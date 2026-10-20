//! Settings → Audio, top to bottom:
//! 1. The default narrator, with its readiness and a preview.
//! 2. One card per voice engine (Kokoro, Chatterbox, Piper): status, size, quality and speed, and
//!    ONE action that runs `mv_studio::voice_setup` (Python if needed, the engine, the voice files
//!    and a check that they load) with live progress. Settings and Studio share that job registry.
//! 3. Every voice, with a sample, its readiness or the button that fixes it, and the default star.
//! 4. The Voice Lab: one line at three intensities (`tts::voice_lab`), with what the engine got.
//! 5. Advanced: Python and the studio-grade engines.
use std::collections::HashMap;

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::HubSystemFit;
use mv_studio::tts::{self, LabResult, LabSample};
use mv_studio::voice_setup::{self, SetupProgress, SetupState, Step, Target};
use mv_studio::voices::{self, DownloadState, VoicePack, PACKS};
use mv_studio::StudioError;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, confirm_inline, disclosure, filter_chip, icon_button, icon_tag, icon_toggle, inline_alert, ph, ph_icon, progress_bar, segmented, setting_row, setting_section, shimmer_text, size, skeleton_line,
    spinner, status_tag, stepper, tag, AlertKind, ButtonKind, Hue, Ph, RampExt as _, StatusKind, Tokens, Tone, Weight,
};
use ui_studio::audio as player;
use ui_studio::setup::{self, Env, VoiceReady};

use crate::ui::{command_chip, default_tag, divided, fact, format_bytes, hero_tile, meter_fact, state_pill, surface};
use crate::SettingsView;

/// Graphics memory Chatterbox needs while it narrates (measured 4.2 GB on an RTX 3080).
const CHATTERBOX_VRAM_GB: f64 = 4.0;

/// The three voice engines MangaVox can set up itself, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Kokoro,
    Chatterbox,
    Piper,
}

impl Engine {
    const ALL: [Engine; 3] = [Engine::Kokoro, Engine::Chatterbox, Engine::Piper];

    /// `kokoro_tts.py install-engine` name.
    fn id(self) -> &'static str {
        match self {
            Engine::Kokoro => "kokoro",
            Engine::Chatterbox => "chatterbox",
            Engine::Piper => "piper",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Engine::Kokoro => "Kokoro",
            Engine::Chatterbox => "Chatterbox",
            Engine::Piper => "Piper",
        }
    }

    fn of_voice(voice: &str) -> Option<Engine> {
        match tts::engine_hint(voice) {
            "kokoro" => Some(Engine::Kokoro),
            "chatterbox" => Some(Engine::Chatterbox),
            "piper" => Some(Engine::Piper),
            _ => None,
        }
    }

    /// Logo-style glyph: Kokoro is light and natural, Chatterbox acts, Piper is fast.
    fn icon(self) -> Ph {
        match self {
            Engine::Kokoro => ph::FEATHER,
            Engine::Chatterbox => ph::MASK_HAPPY,
            Engine::Piper => ph::LIGHTNING,
        }
    }

    fn hue(self) -> Hue {
        match self {
            Engine::Kokoro => Hue::Blue,
            Engine::Chatterbox => Hue::Violet,
            Engine::Piper => Hue::Amber,
        }
    }

    fn tagline(self) -> &'static str {
        match self {
            Engine::Kokoro => "Natural voices that run fast on any CPU. The best place to start.",
            Engine::Chatterbox => "Acts out each scene: calm pages stay soft, fights and reveals get real intensity. Resemble AI's open model (MIT).",
            Engine::Piper => "The lightest and fastest engine, one small download per voice. Sounds more synthetic than Kokoro.",
        }
    }

    fn languages(self) -> &'static str {
        match self {
            Engine::Chatterbox => "English",
            _ => "English, Hindi",
        }
    }

    fn packs(self) -> impl Iterator<Item = &'static VoicePack> {
        PACKS.iter().filter(move |p| voices::engine_of(p) == self.id())
    }

    /// `voice_setup` registry keys that belong to this engine (its packs and the engine alone).
    fn setup_keys(self) -> Vec<&'static str> {
        self.packs().map(|p| p.id).chain([Target::engine(self.id()).key()]).collect()
    }

    /// Catalog voices the engine speaks, once each (Kokoro's full and Lite packs share them).
    fn voices(self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = vec![];
        for p in self.packs() {
            for v in p.voice_ids() {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        out
    }
}

/// Where one engine stands, worked out from the setup jobs, downloads and the last check.
#[derive(Clone, Debug, PartialEq)]
enum Status {
    Checking,
    Ready,
    /// Voice files are here but the engine is missing or does not load yet.
    NeedsEngine,
    NotInstalled,
    Installing { stop: Stop, step: Step, text: String, fraction: Option<f32> },
    Failed { key: &'static str, message: String },
}

/// What the Cancel button of a running install stops.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stop {
    Setup(&'static str),
    /// A plain download (started from Studio): it pauses and resumes later.
    Download(&'static str),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum VoiceFilter {
    #[default]
    All,
    English,
    Hindi,
    Ready,
}

/// Scene kinds the Voice Lab offers (`tts::SCENE_KINDS`), with a label and glyph each.
const KINDS: &[(&str, &str, Ph)] = &[
    ("action", "Action", ph::LIGHTNING),
    ("fight", "Fight", ph::SWORD),
    ("emotional", "Emotional", ph::MASK_SAD),
    ("reveal", "Reveal", ph::EYE),
    ("dialogue", "Dialogue", ph::CHAT_CIRCLE_DOTS),
    ("calm", "Calm", ph::FEATHER),
    ("comedy", "Comedy", ph::SMILEY),
    ("romance", "Romance", ph::HEART),
    ("horror", "Horror", ph::GHOST),
    ("flashback", "Flashback", ph::CLOCK_COUNTER_CLOCKWISE),
    ("other", "Other", ph::DOTS_THREE),
];

/// What the Audio page knows. Pack state is cached and refreshed after every change.
pub struct AudioState {
    loaded: bool,
    /// `None` while the engine check runs.
    env: Option<Env>,
    checking: bool,
    default_voice: String,
    installed: HashMap<&'static str, bool>,
    partial: HashMap<&'static str, u64>,
    previewing: Option<&'static str>,
    /// Downloads and setup jobs seen running on the last tick, to refresh when they end.
    running: Vec<&'static str>,
    setups: Vec<&'static str>,
    /// Keeps `env` in step with Studio's checks and with voice changes.
    sync: setup::EnvSync,
    /// Graphics card and memory, for Chatterbox. `None` until detected.
    system: Option<HubSystemFit>,
    /// Kokoro variant the next setup gets.
    kokoro_lite: bool,
    /// Engine whose "Remove voices" is waiting for its inline confirm.
    confirm_remove: Option<Engine>,
    /// Piper voice whose own Remove is waiting for its inline confirm.
    confirm_voice: Option<&'static str>,
    filter: VoiceFilter,
    lab_voice: &'static str,
    lab_kind: &'static str,
    /// Intensity whose button started the render that is running.
    lab_busy: Option<u8>,
    lab: Option<LabResult>,
    lab_error: Option<String>,
    /// Chatterbox seconds of work per second of audio, measured in the Voice Lab.
    cb_rtf: Option<f64>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            loaded: false,
            env: None,
            checking: false,
            default_voice: String::new(),
            installed: HashMap::new(),
            partial: HashMap::new(),
            previewing: None,
            running: vec![],
            setups: vec![],
            sync: setup::EnvSync::default(),
            system: None,
            kokoro_lite: false,
            confirm_remove: None,
            confirm_voice: None,
            filter: VoiceFilter::All,
            lab_voice: lab_voice_ids()[0],
            lab_kind: "action",
            lab_busy: None,
            lab: None,
            lab_error: None,
            cb_rtf: None,
        }
    }
}

impl AudioState {
    /// True while something on the page changes on its own (setup, download, sample, check).
    pub fn busy(&self) -> bool {
        !self.running.is_empty()
            || !self.setups.is_empty()
            || voices::any_running()
            || voice_setup::any_running()
            || self.previewing.is_some()
            || self.lab_busy.is_some()
            || player::now_playing().is_some()
            || self.sync.pending()
    }
}

/// Voices in the Voice Lab picker: expressive first, then Kokoro, then Piper.
fn lab_voice_ids() -> Vec<&'static str> {
    let mut out = vec![];
    for e in [Engine::Chatterbox, Engine::Kokoro, Engine::Piper] {
        out.extend(e.voices());
    }
    out
}

/// The Voice Lab's voice picker, created with the page.
pub(crate) fn lab_voice_select(window: &mut Window, cx: &mut Context<SettingsView>) -> Entity<SelectState<Vec<SharedString>>> {
    let ids = lab_voice_ids();
    let labels: Vec<SharedString> = ids.iter().map(|v| SharedString::from(voice_label(v))).collect();
    let state = cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(0)), window, cx));
    cx.subscribe_in(&state, window, move |this: &mut SettingsView, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
        if let SelectEvent::Confirm(Some(label)) = ev {
            if let Some(id) = ids.iter().copied().find(|v| voice_label(v) == label.as_ref()) {
                this.audio.lab_voice = id;
                this.audio.lab_error = None;
                cx.notify();
            }
        }
    })
    .detach();
    state
}

/// A sample of `voice` is playing (sample files start with the voice's name).
fn sample_playing(voice: &str) -> bool {
    let prefix = format!("{}_", tts::safe_voice_name(voice));
    player::now_playing().is_some_and(|p| p.starts_with(tts::default_samples_dir()) && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
}

fn voice_label(id: &str) -> String {
    tts::find_voice(id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string())
}

/// "Heart (US)" from "English — Heart (US)": the tiles are already grouped and filtered by language.
fn short_name(id: &str) -> String {
    let label = voice_label(id);
    label.split_once(" — ").map(|(_, n)| n.to_string()).unwrap_or(label)
}

/// "warm female" from "Kokoro · warm female".
fn voice_style(id: &str) -> &'static str {
    tts::find_voice(id).map(|v| v.desc.split_once(" · ").map(|(_, s)| s).unwrap_or(v.desc)).unwrap_or("")
}

fn language_name(code: &str) -> &'static str {
    if code == "hi" {
        "Hindi"
    } else {
        "English"
    }
}

/// Synthesis time per second of audio, in words.
fn speed_text(rtf: f64) -> String {
    if !rtf.is_finite() || rtf <= 0.0 {
        String::new()
    } else if rtf < 1.0 {
        format!("{:.1}× faster than real time", 1.0 / rtf)
    } else {
        format!("{rtf:.1} s of work per second of audio")
    }
}

/// `{exaggeration: 0.7, cfg_weight: 0.34}` as "exaggeration 0.70 · cfg weight 0.34".
fn params_text(v: &serde_json::Value) -> String {
    let Some(map) = v.as_object() else { return String::new() };
    map.iter()
        .filter_map(|(k, v)| {
            let val = match v {
                serde_json::Value::Number(n) => format!("{:.2}", n.as_f64()?),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => return None,
            };
            Some(format!("{} {val}", k.replace('_', " ")))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn intensity_word(i: u8) -> &'static str {
    match i {
        0..=3 => "Gentle",
        4..=6 => "Building",
        _ => "Intense",
    }
}

/// The setup job a registry key belongs to (a pack id or `engine:<name>`).
fn target_for_key(key: &str) -> Option<Target> {
    match key.strip_prefix("engine:") {
        Some(name) => Some(Target::engine(name)),
        None => Target::pack(key),
    }
}

/// Why a voice cannot be played yet (`None` when it can): still checking, or its engine is
/// missing. Disabled controls say why, and the reason must match the real state.
/// NN/g, visibility of system status: <https://www.nngroup.com/articles/visibility-system-status/>.
fn not_playable(ready: Option<VoiceReady>, missing: impl Into<SharedString>) -> Option<SharedString> {
    match ready {
        None => Some("Checking the voice engines…".into()),
        Some(r) if !r.usable() => Some(missing.into()),
        Some(_) => None,
    }
}

/// Bytes `packs` take on disk, counting a file they share once (Kokoro's full and Lite packs
/// share `voices-v1.0.bin`, so adding their sizes overstated "Remove … of Kokoro voices").
fn packs_size(packs: &[&VoicePack]) -> u64 {
    let mut seen: Vec<(&str, &str)> = vec![];
    let mut total = 0;
    for f in packs.iter().flat_map(|p| p.files.iter()) {
        if !seen.contains(&(f.dir, f.name)) {
            seen.push((f.dir, f.name));
            total += f.size;
        }
    }
    total
}

/// "Resume · 120 MB of 340 MB" when part of `pack` is already on disk, else `None`.
fn resume_label(partial: u64, pack: &VoicePack) -> Option<String> {
    (partial > 0 && partial < pack.size()).then(|| format!("Resume · {} of {}", format_bytes(partial), format_bytes(pack.size())))
}

fn setup_text(p: &SetupProgress) -> String {
    match (&p.step, &p.download) {
        (Step::Model, Some(d)) => download_text(d),
        _ => p.summary(),
    }
}

fn download_text(d: &voices::Progress) -> String {
    let speed = if d.bytes_per_sec > 0.0 { format!(" · {}/s", format_bytes(d.bytes_per_sec as u64)) } else { String::new() };
    format!("Downloading the voices · {:.0}% · {} of {}{speed}", d.fraction() * 100.0, format_bytes(d.done), format_bytes(d.total))
}

impl SettingsView {
    /// First visit: read pack state, check the engines and look for a graphics card.
    pub(crate) fn audio_open(&mut self, cx: &mut Context<Self>) {
        if self.audio.loaded {
            // Back on the page: reread the default narrator and packs (Studio's Setup sheet can
            // change both), and show what Studio checked meanwhile or check again after a change.
            self.refresh_packs(cx);
            self.follow_checks(cx);
            return;
        }
        self.audio.loaded = true;
        if let Some(env) = self.audio.sync.take_newer() {
            self.audio.env = Some(env);
        }
        self.refresh_packs(cx);
        self.check_engines(cx);
        cx.spawn(async move |this, cx| {
            let sys = mv_ui::bg::blocking(ui_assistant::detect_system).await.ok();
            this.update(cx, |v, cx| {
                v.audio.system = sys;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Adopts a newer check (Studio's Setup sheet runs them too) and checks again when the
    /// installed voices changed since the one shown, so both pages show the same readiness.
    fn follow_checks(&mut self, cx: &mut Context<Self>) {
        if !self.audio.sync.pending() {
            return;
        }
        if let Some(env) = self.audio.sync.take_newer() {
            self.audio.env = Some(env);
            cx.notify();
        }
        if self.audio.sync.should_check(self.audio.checking) {
            self.refresh_packs(cx);
            self.check_engines(cx);
        }
    }

    pub(crate) fn refresh_packs(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let state = mv_ui::bg::blocking(|| {
                let installed: HashMap<_, _> = PACKS.iter().map(|p| (p.id, voices::installed(p))).collect();
                let root = voices::models_root();
                let partial: HashMap<_, _> = PACKS.iter().map(|p| (p.id, voices::partial_bytes_in(&root, p))).collect();
                (installed, partial, voices::default_voice())
            })
            .await;
            if let Ok((installed, partial, default_voice)) = state {
                this.update(cx, |v, cx| {
                    v.audio.installed = installed;
                    v.audio.partial = partial;
                    v.audio.default_voice = default_voice;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Runs Studio's environment check (Python, engine packages, models). Takes a few seconds.
    pub(crate) fn check_engines(&mut self, cx: &mut Context<Self>) {
        if self.audio.checking {
            return;
        }
        self.audio.checking = true;
        self.audio.sync.started();
        cx.spawn(async move |this, cx| {
            let env = mv_ui::bg::run(setup::check()).await.ok();
            this.update(cx, |v, cx| {
                v.audio.checking = false;
                // The newest finished check: this one, unless Studio's finished later.
                match v.audio.sync.take_newer() {
                    Some(newest) => v.audio.env = Some(newest),
                    None if v.audio.env.is_none() => v.audio.env = env,
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Called by the page ticker: repaints progress and refreshes when a download or setup ends.
    pub(crate) fn audio_tick(&mut self, cx: &mut Context<Self>) {
        self.follow_checks(cx);
        let downloads: Vec<&'static str> = PACKS.iter().filter(|p| matches!(voices::download_state(p.id), Some(DownloadState::Running(_)))).map(|p| p.id).collect();
        let setups: Vec<&'static str> = Engine::ALL.iter().flat_map(|e| e.setup_keys()).filter(|k| matches!(voice_setup::state(k), Some(SetupState::Running(_)))).collect();
        let ended = self.audio.running.iter().any(|id| !downloads.contains(id)) || self.audio.setups.iter().any(|k| !setups.contains(k));
        self.audio.running = downloads;
        self.audio.setups = setups;
        if ended {
            self.refresh_packs(cx);
            self.check_engines(cx);
        }
        cx.notify();
    }

    fn pack_installed(&self, p: &VoicePack) -> bool {
        self.audio.installed.get(p.id).copied().unwrap_or(false)
    }

    fn env_engine_flags(&self, e: Engine) -> Option<(bool, bool)> {
        let env = self.audio.env.as_ref()?;
        let g = &env.engines;
        Some(match e {
            Engine::Kokoro => (g.kokoro, g.kokoro_deps),
            Engine::Chatterbox => (g.chatterbox, g.chatterbox_deps),
            Engine::Piper => (g.piper_deps && (g.piper_en || g.piper_hi), g.piper_deps),
        })
    }

    fn engine_status(&self, e: Engine) -> Status {
        let keys = e.setup_keys();
        for &key in &keys {
            if let Some(SetupState::Running(p)) = voice_setup::state(key) {
                return Status::Installing { stop: Stop::Setup(key), step: p.step, text: setup_text(&p), fraction: p.download.as_ref().map(voices::Progress::fraction) };
            }
        }
        for pack in e.packs() {
            if let Some(DownloadState::Running(d)) = voices::download_state(pack.id) {
                return Status::Installing { stop: Stop::Download(pack.id), step: Step::Model, text: download_text(&d), fraction: Some(d.fraction()) };
            }
        }
        for &key in &keys {
            if let Some(SetupState::Failed(message)) = voice_setup::state(key) {
                return Status::Failed { key, message };
            }
        }
        let Some((ready, _)) = self.env_engine_flags(e) else { return Status::Checking };
        if ready {
            Status::Ready
        } else if e.packs().any(|p| self.pack_installed(p)) {
            Status::NeedsEngine
        } else {
            Status::NotInstalled
        }
    }

    /// The pack a fresh setup of `e` downloads.
    fn setup_pack(&self, e: Engine) -> &'static VoicePack {
        let id = match e {
            Engine::Kokoro if self.audio.kokoro_lite => "kokoro-lite",
            Engine::Kokoro => "kokoro",
            Engine::Chatterbox => "chatterbox",
            // The default narrator's own Piper voice, else one in its language.
            Engine::Piper => match voices::pack_for_voice(&self.audio.default_voice).filter(|p| voices::engine_of(p) == "piper") {
                Some(p) => p.id,
                None if tts::voice_lang(&self.audio.default_voice) == "hi" => "piper-hi-priyamvada",
                None => "piper-en-amy",
            },
        };
        voices::find_pack(id).unwrap_or(&PACKS[0])
    }

    /// The one engine whose setup button is the page's primary action (kit rule: one primary
    /// button per view, <https://m3.material.io/components/all-buttons>): the default narrator's
    /// engine when it cannot speak, else an engine with voice files but no engine, else Kokoro.
    fn lead_engine(&self) -> Option<Engine> {
        let needs = |e: Engine| matches!(self.engine_status(e), Status::NotInstalled | Status::NeedsEngine);
        Engine::of_voice(&self.audio.default_voice)
            .filter(|e| needs(*e))
            .or_else(|| Engine::ALL.into_iter().find(|e| self.engine_status(*e) == Status::NeedsEngine))
            .or_else(|| needs(Engine::Kokoro).then_some(Engine::Kokoro))
    }

    /// The job that makes `e` usable now: finish an installed pack, or set up a new one.
    fn fix_target(&self, e: Engine) -> Target {
        match e.packs().find(|p| self.pack_installed(p)) {
            Some(p) if self.engine_status(e) == Status::NeedsEngine => Target::Pack(p),
            _ => Target::Pack(self.setup_pack(e)),
        }
    }

    /// Runs one setup job (Python if needed, the engine, the voice files, a load check) and
    /// reports how it ended. Studio's Setup sheet shows the same job.
    fn run_setup(&mut self, target: Target, cx: &mut Context<Self>) {
        let key = target.key();
        if matches!(voice_setup::state(key), Some(SetupState::Running(_))) {
            return;
        }
        // Two writers on one Chatterbox `.part` file would corrupt it.
        if target.engine_name() == "chatterbox" && matches!(voices::download_state("chatterbox"), Some(DownloadState::Running(_))) {
            toast::show(cx, Toast::info("The Chatterbox voice files are downloading already. Setup continues when they finish."));
        }
        voice_setup::clear_error(key);
        self.audio.confirm_remove = None;
        if !self.audio.setups.contains(&key) {
            self.audio.setups.push(key);
        }
        let label = tts::engine_label(target.engine_name());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(voice_setup::run(target)).await;
            this.update(cx, |v, cx| {
                match res {
                    Ok(Ok(())) => toast::show(cx, Toast::success(format!("{label} is ready to narrate"))),
                    Ok(Err(StudioError::Cancelled)) => {}
                    // The engine card keeps the message with a Try again button.
                    Ok(Err(_)) => toast::show(cx, Toast::error(format!("{label} setup did not finish"))),
                    Err(e) => toast::show(cx, Toast::error(format!("{label} setup did not finish: {e:#}"))),
                }
                v.refresh_packs(cx);
                v.check_engines(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn stop_install(&mut self, stop: Stop, cx: &mut Context<Self>) {
        match stop {
            Stop::Setup(key) => voice_setup::cancel(key),
            Stop::Download(id) => voices::cancel_download(id),
        }
        self.refresh_packs(cx);
        cx.notify();
    }

    /// Bytes removing `pack` frees: files another installed pack shares stay (`voices::remove`).
    fn freed_by(&self, pack: &'static VoicePack) -> u64 {
        let others: Vec<&VoicePack> = PACKS.iter().filter(|p| p.id != pack.id && self.pack_installed(p)).collect();
        let mut with = others.clone();
        with.push(pack);
        packs_size(&with) - packs_size(&others)
    }

    /// Removes one pack (a single Piper voice, or Kokoro's unused Lite copy). The old pack cards
    /// had a Remove per pack; the engine card's Remove takes them all.
    fn remove_pack(&mut self, pack: &'static VoicePack, cx: &mut Context<Self>) {
        self.audio.confirm_voice = None;
        if pack.voice_ids().iter().any(|v| sample_playing(v)) {
            player::stop();
        }
        let freed = self.freed_by(pack);
        self.background(cx, move || voices::remove(pack).map_err(anyhow::Error::from), move |v, r, cx| {
            v.report(&format!("{} removed · {} freed", pack.name, format_bytes(freed)), r, cx);
            v.refresh_packs(cx);
            v.check_engines(cx);
        });
    }

    fn remove_engine(&mut self, e: Engine, cx: &mut Context<Self>) {
        self.audio.confirm_remove = None;
        player::stop();
        let packs: Vec<&'static VoicePack> = e.packs().filter(|p| self.pack_installed(p)).collect();
        let freed = packs_size(&packs);
        self.background(
            cx,
            move || {
                for p in &packs {
                    voices::remove(p).map_err(anyhow::Error::from)?;
                }
                Ok(())
            },
            move |v, r, cx| {
                v.report(&format!("{} voices removed · {} freed", e.name(), format_bytes(freed)), r, cx);
                v.refresh_packs(cx);
                v.check_engines(cx);
            },
        );
    }

    fn set_default_voice(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.audio.default_voice = id.to_string();
        self.studio.voice = id.to_string();
        cx.notify();
        self.background(cx, move || voices::set_default_voice(id).map_err(anyhow::Error::from), move |v, r, cx| {
            v.report(&format!("{} is now the default narrator", voice_label(id)), r, cx)
        });
    }

    fn play_sample(&mut self, voice: &'static str, cx: &mut Context<Self>) {
        self.play_sample_at(voice, 1.0, cx);
    }

    /// Plays (or stops) a sample of `voice` at `speed`. Samples are cached per voice and speed.
    fn play_sample_at(&mut self, voice: &'static str, speed: f64, cx: &mut Context<Self>) {
        // This voice's sample is playing (its cached clip, or a stand-in's clip): stop it.
        if sample_playing(voice) {
            player::stop();
            return cx.notify();
        }
        let Some(env) = self.audio.env.clone() else {
            toast::show(cx, Toast::info("Still checking the voice engines. Try again in a moment."));
            return;
        };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else {
            toast::show(cx, Toast::error("Samples need a voice engine. Set one up in Settings → Audio; it installs Python too."));
            return;
        };
        self.audio.previewing = Some(voice);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { tts::voice_preview(&py, &layout, &tts::default_samples_dir(), &std::env::temp_dir(), voice, speed, None).await }).await;
            this.update(cx, |v, cx| {
                // Only the newest request plays and clears the spinner: pressing a second voice
                // while the first rendered used to play the first and drop the second's spinner.
                let latest = v.audio.previewing == Some(voice);
                if latest {
                    v.audio.previewing = None;
                }
                match res {
                    Ok(Ok(p)) if latest => player::play(&p.path),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => toast::show(cx, Toast::error(format!("The sample failed: {e}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("The sample failed: {e:#}"))),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Studio → Speed: hear the default narrator at the chosen rate (the voice engines are
    /// checked when the Studio page opens).
    /// The default narrator's id: the live one once the Audio state has loaded, else the
    /// Studio prefs copy.
    pub(crate) fn narrator_id(&self) -> &str {
        if self.audio.default_voice.is_empty() {
            &self.studio.voice
        } else {
            &self.audio.default_voice
        }
    }

    pub(crate) fn speed_preview(&self, speed: f64, cx: &mut Context<Self>) -> AnyElement {
        let Some(voice) = tts::find_voice(self.narrator_id()).map(|v| v.id) else {
            return div().into_any_element();
        };
        if self.audio.previewing == Some(voice) {
            return div()
                .id("studio-speed-busy")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(radius::MD))
                .tooltip(mv_ui::tip("Making a sample…"))
                .child(spinner("studio-speed-spin", Tokens::accent(), icon_px::MD))
                .into_any_element();
        }
        let playing = sample_playing(voice);
        icon_button("studio-speed-play", if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop" } else { "Hear the default voice at this speed" })
            .selected(playing)
            .when_some(not_playable(self.readiness(voice), "Set up the default voice in Audio to hear it"), |b, why| b.disabled(why))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play_sample_at(voice, speed, cx)))
            .into_any_element()
    }

    // ------------------------------------------------------------ voice lab

    /// The take for `intensity` from the last render, when it matches the current voice and kind.
    fn lab_sample(&self, intensity: u8) -> Option<&LabSample> {
        let r = self.audio.lab.as_ref().filter(|r| r.voice == self.audio.lab_voice && r.kind == self.audio.lab_kind)?;
        r.samples.iter().find(|s| s.intensity == intensity)
    }

    /// Plays a take, rendering all three first (one model load) when needed.
    fn run_lab(&mut self, intensity: u8, cx: &mut Context<Self>) {
        if let Some(s) = self.lab_sample(intensity) {
            if player::now_playing().as_deref() == Some(s.path.as_path()) {
                player::stop();
            } else {
                player::play(&s.path);
            }
            return cx.notify();
        }
        let Some(env) = self.audio.env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else {
            toast::show(cx, Toast::error("The Voice Lab needs a voice engine. Set one up above; it installs Python too."));
            return;
        };
        let (voice, kind) = (self.audio.lab_voice, self.audio.lab_kind);
        player::stop();
        self.audio.lab_busy = Some(intensity);
        self.audio.lab_error = None;
        cx.spawn(async move |this, cx| {
            let req = tts::LabRequest::new(voice, kind);
            let res = mv_ui::bg::run(async move { tts::voice_lab(&py, &layout, &tts::default_lab_dir(), &std::env::temp_dir(), &req).await }).await;
            this.update(cx, |v, cx| {
                v.audio.lab_busy = None;
                match res {
                    Ok(Ok(r)) => {
                        let rtfs: Vec<f64> = r.samples.iter().filter_map(|s| s.rtf).collect();
                        if r.engine == "chatterbox" && !rtfs.is_empty() {
                            v.audio.cb_rtf = Some(rtfs.iter().sum::<f64>() / rtfs.len() as f64);
                        }
                        // Not when another voice or scene was picked while it rendered.
                        let current = v.audio.lab_voice == r.voice && v.audio.lab_kind == r.kind;
                        if let Some(s) = r.samples.iter().find(|s| s.intensity == intensity).filter(|_| current) {
                            player::play(&s.path);
                        }
                        v.audio.lab = Some(r);
                    }
                    Ok(Err(e)) => v.audio.lab_error = Some(e.to_string()),
                    Err(e) => v.audio.lab_error = Some(format!("{e:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ render

    pub(crate) fn render_audio(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(self.render_narrator(cx))
            .child(self.render_engines(cx))
            .child(self.render_voices(cx))
            .child(self.render_lab(cx))
            .child(self.render_advanced(cx))
    }

    fn readiness(&self, voice: &str) -> Option<VoiceReady> {
        let env = self.audio.env.as_ref()?;
        tts::find_voice(voice).map(|v| setup::voice_ready(&env.engines, v))
    }

    /// The default narrator: who it is, whether it can speak, and a preview.
    fn render_narrator(&self, cx: &mut Context<Self>) -> Div {
        let id = self.audio.default_voice.clone();
        let info = tts::find_voice(&id);
        let ready = self.readiness(&id);
        let engine = Engine::of_voice(&id);
        let name: AnyElement = if id.is_empty() {
            skeleton_line(px(180.)).h(px(18.)).into_any_element()
        } else {
            div().flex().items_center().gap(px(8.)).child(div().ramp(mv_ui::theme::ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(voice_label(&id))).child(default_tag()).into_any_element()
        };
        let meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.))
            .gap_y(px(4.))
            .when_some(info, |d, v| d.child(fact(ph::WAVEFORM, tts::engine_label(tts::engine_hint(v.id)))).child(fact(ph::TRANSLATE, language_name(v.lang))).child(fact(ph::USER, voice_style(v.id))))
            .child(match ready {
                None => skeleton_line(px(70.)).into_any_element(),
                Some(VoiceReady::Ready(_)) => status_tag(StatusKind::Done).label("Ready").into_any_element(),
                Some(VoiceReady::Fallback(e)) => state_pill(ph::INFO, Tokens::info(), Tokens::info_soft(), format!("{e} stands in")).into_any_element(),
                Some(VoiceReady::Missing) => state_pill(ph::WARNING, Tokens::warning(), Tokens::warning_soft(), "Can't speak yet").into_any_element(),
            });
        let voice: Option<&'static str> = info.map(|v| v.id);
        let preview = voice.map(|v| {
            let playing = sample_playing(v);
            btn("narrator-play", if playing { "Stop" } else { "Preview" }, ButtonKind::Secondary)
                .icon(if playing { ph::STOP } else { ph::PLAY })
                .selected(playing)
                .loading(self.audio.previewing == Some(v))
                .when_some(not_playable(ready, "Set up this voice's engine to hear it"), |b, why| b.disabled(why))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.play_sample(v, cx)))
        });
        let mut col = div().flex().flex_col().gap(px(12.)).child(
            surface().flex_row().items_center().gap(px(16.)).p(px(16.)).child(hero_tile(ph::MICROPHONE_STAGE, Hue::Cyan, 52.)).child(
                div().flex().flex_col().flex_1().min_w_0().gap(px(6.)).child(mv_ui::section_label("Default narrator")).child(name).child(meta),
            )
            .children(preview),
        );
        let fix = engine.map(|e| (e, self.fix_target(e)));
        // While that engine's setup runs the alert says so instead of offering the same setup
        // again (a second press did nothing).
        let installing = engine.is_some_and(|e| matches!(self.engine_status(e), Status::Installing { .. }));
        let action = |e: Engine, target: Target, alert: mv_ui::Alert, cx: &mut Context<Self>| {
            if installing {
                alert
            } else {
                alert.action(format!("Set up {}", e.name()), cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx)))
            }
        };
        match (ready, fix) {
            (Some(VoiceReady::Missing), Some((e, target))) => {
                let body = if installing {
                    format!("{} is being set up below. Studio narrates with it as soon as it finishes.", e.name())
                } else {
                    format!("Set up {} below. Until then Studio has no voice to narrate chapters with.", e.name())
                };
                col = col.child(action(e, target, inline_alert(AlertKind::Warning, format!("{} can't speak yet", voice_label(&id)), body), cx));
            }
            (Some(VoiceReady::Fallback(stand_in)), Some((e, target))) => {
                let body = if installing { format!("{} is being set up below.", e.name()) } else { format!("Set up {} to hear the real voice.", e.name()) };
                col = col.child(action(e, target, inline_alert(AlertKind::Info, format!("{stand_in} speaks for this voice for now"), body), cx));
            }
            // A studio-grade voice (F5-TTS, XTTS, Parler) with no engine and no stand-in: the
            // pill alone said "Can't speak yet" with no reason or way forward.
            (Some(VoiceReady::Missing), None) => {
                col = col.child(inline_alert(
                    AlertKind::Warning,
                    format!("{} can't speak on this PC", voice_label(&id)),
                    "Its engine is installed outside MangaVox and was not found. Pick a voice below to narrate with an engine MangaVox sets up.",
                ));
            }
            (Some(VoiceReady::Fallback(stand_in)), None) => {
                col = col.child(inline_alert(AlertKind::Info, format!("{stand_in} speaks for this voice for now"), "Studio-grade engines (F5-TTS, XTTS, Parler-TTS) are installed outside MangaVox. Pick a voice below to narrate with an engine MangaVox sets up."));
            }
            _ => {}
        }
        col
    }

    fn render_engines(&self, cx: &mut Context<Self>) -> Div {
        let mut section = setting_section("Voice engines", Some("The programs that turn text into speech. They run on this PC, offline. One button sets up everything an engine needs.".into()));
        let mut list = div().flex().flex_col().gap(px(12.)).pt(px(8.)).pb(px(6.));
        for e in Engine::ALL {
            list = list.child(self.engine_card(e, cx));
        }
        section = section.child(list);
        section
    }

    fn status_element(status: &Status) -> AnyElement {
        match status {
            Status::Checking => status_tag(StatusKind::Running).label("Checking").into_any_element(),
            Status::Ready => status_tag(StatusKind::Done).label("Ready").into_any_element(),
            Status::NeedsEngine => state_pill(ph::WRENCH, Tokens::warning(), Tokens::warning_soft(), "Needs engine").into_any_element(),
            Status::NotInstalled => state_pill(ph::DOWNLOAD_SIMPLE, Tokens::text_2(), Tokens::active(), "Not installed").into_any_element(),
            Status::Installing { .. } => status_tag(StatusKind::Running).label("Installing").into_any_element(),
            Status::Failed { .. } => status_tag(StatusKind::Failed).label("Setup failed").into_any_element(),
        }
    }

    fn engine_facts(&self, e: Engine) -> Vec<AnyElement> {
        let lite_only = e == Engine::Kokoro && self.audio.installed.get("kokoro-lite") == Some(&true) && self.audio.installed.get("kokoro") != Some(&true);
        let pack = if lite_only { voices::find_pack("kokoro-lite").unwrap_or(self.setup_pack(e)) } else { self.setup_pack(e) };
        let voices_n = e.voices().len();
        let size = match e {
            Engine::Piper => format!("about {} per voice", format_bytes(pack.size())),
            _ => format!("{} voices + {} engine", format_bytes(pack.size()), format_bytes(voices::engine_size(pack))),
        };
        let mut out = vec![
            meter_fact(ph::SPARKLE, "Quality", pack.quality).into_any_element(),
            meter_fact(ph::GAUGE, "Speed", pack.speed).into_any_element(),
            fact(ph::HARD_DRIVE, size).into_any_element(),
            fact(ph::TRANSLATE, e.languages()).into_any_element(),
            fact(ph::USERS, format!("{voices_n} voice{}", if voices_n == 1 { "" } else { "s" })).into_any_element(),
        ];
        if voices::needs_gpu(pack) {
            out.push(icon_tag(ph::WARNING, "Needs about 4 GB of graphics memory", Tone::Warning).into_any_element());
        } else {
            out.push(fact(ph::CPU, "Runs on any CPU").into_any_element());
        }
        out
    }

    /// Graphics card and measured speed, for Chatterbox.
    fn gpu_note(&self) -> Div {
        let gpu: AnyElement = match &self.audio.system {
            None => skeleton_line(px(160.)).into_any_element(),
            Some(s) => match (&s.gpu_name, s.vram_gb >= CHATTERBOX_VRAM_GB) {
                (Some(name), true) => icon_tag(ph::CHECK_CIRCLE, format!("{name} · {:.0} GB", s.vram_gb), Tone::Success).into_any_element(),
                (Some(name), false) => icon_tag(ph::WARNING, format!("{name} · {:.0} GB, not enough", s.vram_gb), Tone::Warning).into_any_element(),
                (None, _) => icon_tag(ph::WARNING, "No dedicated graphics card found", Tone::Warning).into_any_element(),
            },
        };
        let speed = match self.audio.cb_rtf {
            Some(r) => format!("Measured on this PC: {}.", speed_text(r)),
            None => "About 1.2 s of work per second of audio on an RTX 3080, so a little slower than real time. Measure it on this PC in the Voice Lab.".to_string(),
        };
        let weak = self.audio.system.as_ref().is_some_and(|s| s.gpu_name.is_none() || s.vram_gb < CHATTERBOX_VRAM_GB);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .child(div().flex().items_center().gap(px(8.)).child(fact(ph::MONITOR, "This PC")).child(gpu))
            .child(div().flex().items_start().gap(px(6.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(div().pt(px(2.)).child(ph_icon(ph::TIMER, Weight::Bold, Tokens::text_3(), icon_px::XS))).child(div().flex_1().min_w_0().child(speed)))
            .child(div().text_size(px(size::SM)).text_color(if weak { Tokens::warning() } else { Tokens::text_3() }).child(if weak {
                "Without an NVIDIA card with about 4 GB of memory, Chatterbox runs on the CPU and is many times slower than real time."
            } else {
                "It holds about 4 GB of graphics memory while it narrates; avoid running two Chatterbox jobs (or a Voice Lab test during one) at once."
            }))
    }

    fn progress_block(&self, e: Engine, stop: Stop, step: Step, text: &str, fraction: Option<f32>, cx: &mut Context<Self>) -> Div {
        let current = (step.number() - 1) as usize;
        let done: Vec<bool> = (0..Step::COUNT as usize).map(|i| i < current).collect();
        let steps = [(ph::TERMINAL_WINDOW, "Python"), (ph::CPU, "Engine"), (ph::DOWNLOAD_SIMPLE, "Voices"), (ph::CHECK_CIRCLE, "Check")];
        let (cancel_label, cancel_icon, cancel_tip) = match stop {
            Stop::Setup(_) => ("Cancel", ph::X, "Stops the setup. Files already downloaded stay."),
            Stop::Download(_) => ("Pause", ph::PAUSE, "Pauses the download. It resumes where it stopped."),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::accent_border())
            .child(stepper(("setup-steps", e as usize), steps, current, &done, Some(current), None))
            .child(progress_bar(("setup-bar", e as usize), fraction.unwrap_or(0.), true))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(text.to_string()))
                    .child(
                        btn(("setup-stop", e as usize), cancel_label, ButtonKind::Ghost)
                            .small()
                            .icon(cancel_icon)
                            .tooltip(cancel_tip)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.stop_install(stop, cx))),
                    ),
            )
    }

    /// The card's one action (and, when ready, the quieter ones).
    fn engine_actions(&self, e: Engine, status: &Status, cx: &mut Context<Self>) -> Option<Div> {
        let installed: Vec<&'static VoicePack> = e.packs().filter(|p| self.pack_installed(p)).collect();
        let on_disk = packs_size(&installed);
        let deps = self.env_engine_flags(e).is_some_and(|f| f.1);
        let kind = if self.lead_engine() == Some(e) { ButtonKind::Primary } else { ButtonKind::Secondary };
        let row = div().flex().flex_wrap().items_center().justify_end().gap(px(8.));
        let caption = |text: String| div().flex_1().min_w(px(180.)).text_size(px(size::SM)).text_color(Tokens::text_3()).child(text);
        match status {
            Status::NotInstalled => {
                let pack = self.setup_pack(e);
                let bytes = pack.size() + if deps { 0 } else { voices::engine_size(pack) };
                let target = Target::Pack(pack);
                let variant = (e == Engine::Kokoro).then(|| {
                    let sizes = |id: &str| voices::find_pack(id).map(|p| format_bytes(p.size())).unwrap_or_default();
                    segmented("kokoro-variant", [format!("Standard · {}", sizes("kokoro")).as_str(), format!("Lite · {}", sizes("kokoro-lite")).as_str()], self.audio.kokoro_lite as usize)
                        .small()
                        .on_change(cx.listener(|this, ix: &usize, _, cx| {
                            this.audio.kokoro_lite = *ix == 1;
                            cx.notify();
                        }))
                });
                // A paused or cancelled download keeps its files: say so and offer Resume
                // (the old pack card's "Resume (x of y)"; the Pause tooltip promises it).
                let resume = resume_label(self.audio.partial.get(pack.id).copied().unwrap_or(0), pack);
                let what = match (e, self.audio.kokoro_lite) {
                    _ if resume.is_some() => "Paused. Setup continues the download where it stopped, then installs the engine.".to_string(),
                    (Engine::Kokoro, true) => "Lite is a third of the size: faster on older PCs, slightly less crisp.".to_string(),
                    (Engine::Piper, _) => format!("Sets up the engine and {}. Add more Piper voices below.", short_name(pack.voices.first().copied().unwrap_or(""))),
                    _ => "Downloads the engine and its voices, then checks that they load.".to_string(),
                };
                let (label, glyph) = match resume {
                    Some(r) => (r, ph::PLAY),
                    None => (format!("Set up {} · {}", e.name(), format_bytes(bytes)), ph::DOWNLOAD_SIMPLE),
                };
                Some(
                    row.child(caption(what)).children(variant).child(
                        btn(SharedString::from(format!("setup-{}", e.id())), label, kind)
                            .icon(glyph)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx))),
                    ),
                )
            }
            Status::NeedsEngine => {
                let target = self.fix_target(e);
                let engine = voice_setup::engine_size_hint(e.id());
                Some(
                    row.child(caption(format!("The voice files are here. The {} engine ({engine}) still needs installing.", e.name()))).child(
                        btn(SharedString::from(format!("finish-{}", e.id())), "Finish setup", kind)
                            .icon(ph::WRENCH)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx))),
                    ),
                )
            }
            Status::Ready if self.audio.confirm_remove == Some(e) => Some(
                row.child(
                    confirm_inline(format!("Remove {} of {} voices?", format_bytes(on_disk), e.name()), "Remove", true)
                        .cancel_label("Keep")
                        .on_cancel(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.audio.confirm_remove = None;
                            cx.notify();
                        }))
                        .on_confirm(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_engine(e, cx))),
                ),
            ),
            Status::Ready => {
                let lite_only = e == Engine::Kokoro && installed.iter().all(|p| p.id == "kokoro-lite") && !installed.is_empty();
                let n = e.voices().len();
                let what = match e {
                    Engine::Piper => {
                        let ready = installed.len();
                        format!("{ready} of {n} voices downloaded · {}", format_bytes(on_disk))
                    }
                    _ => format!("{n} voices ready · {} on disk{}", format_bytes(on_disk), if lite_only { " (Lite)" } else { "" }),
                };
                let full = voices::find_pack("kokoro");
                // Both Kokoro packs on disk: the Lite copy is never used (kokoro_tts.py prefers
                // the full model), so offer to drop just that one.
                let spare_lite = (e == Engine::Kokoro && installed.len() > 1).then(|| installed.iter().copied().find(|p| p.id == "kokoro-lite")).flatten();
                Some(
                    row.child(caption(what))
                        .when(lite_only, |d| {
                            d.children(full.map(|p| {
                                btn("kokoro-upgrade", format!("Get full quality · {}", format_bytes(p.size())), ButtonKind::Ghost)
                                    .small()
                                    .icon(ph::SPARKLE)
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(Target::Pack(p), cx)))
                            }))
                        })
                        .children(spare_lite.map(|p| {
                            btn("kokoro-drop-lite", format!("Remove Lite copy · {}", format_bytes(self.freed_by(p))), ButtonKind::DangerGhost)
                                .small()
                                .icon(mv_ui::glyphs::DELETE)
                                .tooltip("Kokoro uses the full voices; the Lite copy is not needed")
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_pack(p, cx)))
                        }))
                        .when(!installed.is_empty(), |d| {
                            d.child(
                                btn(SharedString::from(format!("remove-{}", e.id())), "Remove voices", ButtonKind::DangerGhost)
                                    .small()
                                    .icon(mv_ui::glyphs::DELETE)
                                    .tooltip("Deletes the voice files. The engine stays; Storage → Delete models removes it too.")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.audio.confirm_remove = Some(e);
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            }
            Status::Checking | Status::Installing { .. } | Status::Failed { .. } => None,
        }
    }

    fn engine_card(&self, e: Engine, cx: &mut Context<Self>) -> Div {
        let status = self.engine_status(e);
        let lite_only = e == Engine::Kokoro && self.audio.installed.get("kokoro-lite") == Some(&true) && self.audio.installed.get("kokoro") != Some(&true);
        let head = div()
            .flex()
            .items_start()
            .gap(px(14.))
            .child(hero_tile(e.icon(), e.hue(), 44.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().ramp(mv_ui::theme::ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(e.name()))
                            .when(e == Engine::Kokoro, |d| d.child(icon_tag(ph::SEAL_CHECK, "Recommended", Tone::Accent)))
                            .when(e == Engine::Chatterbox, |d| d.child(icon_tag(ph::MASK_HAPPY, "Emotion", Tone::Info)))
                            .when(lite_only, |d| d.child(tag("Lite", Tone::Neutral)))
                            .child(div().flex_1())
                            .child(Self::status_element(&status)),
                    )
                    .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(e.tagline())),
            );
        let facts = div().flex().flex_wrap().items_center().gap_x(px(16.)).gap_y(px(8.)).pl(px(58.)).children(self.engine_facts(e));
        let mut card = surface().p(px(16.)).gap(px(14.)).child(head).child(facts);
        if e == Engine::Chatterbox {
            card = card.child(self.gpu_note());
        }
        card = match &status {
            Status::Installing { stop, step, text, fraction } => card.child(self.progress_block(e, *stop, *step, text, *fraction, cx)),
            Status::Failed { key, message } => {
                let key = *key;
                let retry = target_for_key(key);
                card.child(
                    inline_alert(AlertKind::Error, format!("{} setup did not finish", e.name()), message.clone())
                        .action(
                            "Try again",
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                voice_setup::clear_error(key);
                                match retry {
                                    Some(t) => this.run_setup(t, cx),
                                    None => cx.notify(),
                                }
                            }),
                        )
                        // A failed extra (Kokoro's full pack over a working Lite one) must not
                        // hide a working engine's status and actions for the rest of the session.
                        .dismissible(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            voice_setup::clear_error(key);
                            cx.notify();
                        })),
                )
            }
            _ => card,
        };
        card.children(self.engine_actions(e, &status, cx))
    }

    /// Every voice the engines speak, grouped by engine, filterable by language and readiness.
    fn render_voices(&self, cx: &mut Context<Self>) -> Div {
        let all: Vec<(Engine, &'static str)> = Engine::ALL.iter().flat_map(|e| e.voices().into_iter().map(move |v| (*e, v))).collect();
        let usable = |v: &str| self.readiness(v).is_some_and(VoiceReady::usable);
        let lang = |v: &str| tts::voice_lang(v);
        let keep = |v: &str| match self.audio.filter {
            VoiceFilter::All => true,
            VoiceFilter::English => lang(v) == "en",
            VoiceFilter::Hindi => lang(v) == "hi",
            VoiceFilter::Ready => usable(v),
        };
        let count = |f: VoiceFilter| {
            all.iter()
                .filter(|(_, v)| match f {
                    VoiceFilter::All => true,
                    VoiceFilter::English => lang(v) == "en",
                    VoiceFilter::Hindi => lang(v) == "hi",
                    VoiceFilter::Ready => usable(v),
                })
                .count()
        };
        let chip = |id: &'static str, label: &'static str, icon: Ph, f: VoiceFilter, cx: &mut Context<Self>| {
            filter_chip(id, label, self.audio.filter == f).icon(icon).count(count(f)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.audio.filter = f;
                cx.notify();
            }))
        };
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .pt(px(6.))
            .pb(px(4.))
            .child(chip("vf-all", "All", ph::USERS, VoiceFilter::All, cx))
            .child(chip("vf-en", "English", ph::TRANSLATE, VoiceFilter::English, cx))
            .child(chip("vf-hi", "Hindi", ph::TRANSLATE, VoiceFilter::Hindi, cx))
            .child(chip("vf-ready", "Ready to use", ph::CHECK_CIRCLE, VoiceFilter::Ready, cx));
        let mut groups = div().flex().flex_col().gap(px(18.)).pt(px(10.)).pb(px(8.));
        let mut shown = 0;
        for e in Engine::ALL {
            let list: Vec<&'static str> = all.iter().filter(|(g, v)| *g == e && keep(v)).map(|(_, v)| *v).collect();
            if list.is_empty() {
                continue;
            }
            shown += list.len();
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(mv_ui::section_label(format!("{} · {}", e.name(), list.len())))
                    .child(div().grid().grid_cols(2).gap(px(8.)).children(list.into_iter().map(|v| self.voice_tile(v, cx)))),
            );
        }
        if shown == 0 {
            // NN/g empty states: say why, and give the direct path to the task,
            // <https://www.nngroup.com/articles/empty-state-interface-design/>.
            let mut empty = mv_ui::empty_view(ph::WAVEFORM, "No voices ready yet", "Voices show up here as soon as their engine is set up and loads.").compact();
            if let Some(e) = self.lead_engine() {
                let target = self.fix_target(e);
                empty = empty.action(
                    btn("vf-empty-setup", format!("Set up {}", e.name()), ButtonKind::Secondary).icon(ph::DOWNLOAD_SIMPLE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx))),
                );
            }
            empty = empty.secondary(btn("vf-empty-all", "Show all voices", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.audio.filter = VoiceFilter::All;
                cx.notify();
            })));
            groups = groups.child(empty);
        }
        setting_section("Voices", Some("Press play to hear a voice. The gold star marks the default narrator Studio uses.".into())).child(chips).child(groups)
    }

    fn play_control(&self, voice: &'static str, ready: Option<VoiceReady>, cx: &mut Context<Self>) -> AnyElement {
        if self.audio.previewing == Some(voice) {
            return div()
                .id(SharedString::from(format!("sample-busy-{voice}")))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(32.))
                .rounded(px(radius::MD))
                .bg(Tokens::active())
                .tooltip(mv_ui::tip("Making a sample…"))
                .child(spinner(SharedString::from(format!("sample-spin-{voice}")), Tokens::accent(), icon_px::MD))
                .into_any_element();
        }
        let playing = sample_playing(voice);
        let engine = Engine::of_voice(voice).map(Engine::name).unwrap_or("its engine");
        icon_button(SharedString::from(format!("sample-{voice}")), if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop" } else { "Play a sample" })
            .selected(playing)
            .when_some(not_playable(ready, format!("Set up {engine} to hear this voice")), |b, why| b.disabled(why))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.play_sample(voice, cx);
            }))
            .into_any_element()
    }

    /// A setup or download that will make `voice` speak is running. Kokoro and Chatterbox set up
    /// all their voices at once; a Piper voice is its own download, so only that voice (or the
    /// Piper engine alone) counts, not another Piper voice's download.
    fn voice_installing(&self, voice: &str, e: Engine) -> bool {
        if e != Engine::Piper {
            return matches!(self.engine_status(e), Status::Installing { .. });
        }
        let running = |key: &str| matches!(voice_setup::state(key), Some(SetupState::Running(_)));
        running(Target::engine(e.id()).key()) || voices::pack_for_voice(voice).is_some_and(|p| running(p.id) || matches!(voices::download_state(p.id), Some(DownloadState::Running(_))))
    }

    /// The status line under a voice's name: Ready, a stand-in, or the button that fixes it.
    fn voice_status(&self, voice: &'static str, ready: Option<VoiceReady>, cx: &mut Context<Self>) -> AnyElement {
        let line = |icon: Ph, weight: Weight, color: Rgba, text: String| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .min_w_0()
                .text_size(px(size::XS))
                .text_color(Tokens::text_3())
                .child(ph_icon(icon, weight, color, icon_px::XS))
                .child(div().min_w_0().truncate().child(text))
        };
        let engine = Engine::of_voice(voice);
        let installing = engine.is_some_and(|e| self.voice_installing(voice, e));
        match ready {
            None => skeleton_line(px(90.)).mt(px(3.)).into_any_element(),
            Some(VoiceReady::Ready(_)) => line(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), format!("Ready · {}", voice_style(voice))).into_any_element(),
            _ if installing => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(size::XS))
                .text_color(Tokens::text_2())
                .child(spinner(SharedString::from(format!("voice-setup-{voice}")), Tokens::accent(), icon_px::XS))
                .child("Setting up…")
                .into_any_element(),
            // A stand-in speaks for it: offer the setup that gives it its own voice.
            Some(VoiceReady::Fallback(stand_in)) => match engine {
                Some(e) => btn(SharedString::from(format!("voice-fix-{voice}")), format!("Set up {}", e.name()), ButtonKind::Secondary)
                    .small()
                    .icon(ph::DOWNLOAD_SIMPLE)
                    .tooltip(format!("{stand_in} speaks for this voice until {} is set up", e.name()))
                    .mt(px(4.))
                    .on_click({
                        let target = self.fix_target(e);
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx))
                    })
                    .into_any_element(),
                None => line(ph::INFO, Weight::Bold, Tokens::info(), format!("{stand_in} speaks for now · {}", voice_style(voice))).into_any_element(),
            },
            Some(VoiceReady::Missing) => {
                let Some(e) = engine else {
                    return line(ph::PROHIBIT, Weight::Bold, Tokens::text_3(), "Not available on this PC".into()).into_any_element();
                };
                // A Piper voice is its own small download; the others need their engine set up.
                let (label, glyph, target) = match (e, voices::pack_for_voice(voice)) {
                    (Engine::Piper, Some(p)) if !self.pack_installed(p) => match resume_label(self.audio.partial.get(p.id).copied().unwrap_or(0), p) {
                        Some(r) => (r, ph::PLAY, Target::Pack(p)),
                        None => (format!("Download · {}", format_bytes(p.size())), ph::DOWNLOAD_SIMPLE, Target::Pack(p)),
                    },
                    _ => (format!("Set up {}", e.name()), ph::DOWNLOAD_SIMPLE, self.fix_target(e)),
                };
                btn(SharedString::from(format!("voice-fix-{voice}")), label, ButtonKind::Secondary)
                    .small()
                    .icon(glyph)
                    .mt(px(4.))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_setup(target, cx)))
                    .into_any_element()
            }
        }
    }

    fn voice_tile(&self, voice: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let is_default = self.audio.default_voice == voice;
        let ready = self.readiness(voice);
        let lang = tts::voice_lang(voice);
        // A downloaded Piper voice is its own small pack, removable on its own (the old pack
        // cards had a Remove each; the engine card's Remove takes every Piper voice).
        let own_pack = (Engine::of_voice(voice) == Some(Engine::Piper))
            .then(|| voices::pack_for_voice(voice))
            .flatten()
            .filter(|p| voices::engine_of(p) == "piper" && self.pack_installed(p) && !self.voice_installing(voice, Engine::Piper));
        let confirming = own_pack.filter(|_| self.audio.confirm_voice == Some(voice));
        let status: AnyElement = match confirming {
            // NN/g: name the consequence, label the button with the action.
            Some(p) => confirm_inline(format!("Remove {}?", format_bytes(self.freed_by(p))), "Remove", true)
                .cancel_label("Keep")
                .mt(px(4.))
                .on_cancel(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.audio.confirm_voice = None;
                    cx.notify();
                }))
                .on_confirm(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_pack(p, cx)))
                .into_any_element(),
            None => self.voice_status(voice, ready, cx),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .p(px(10.))
            .rounded(px(radius::LG))
            .border_1()
            .map(|d| if is_default { d.border_color(Tokens::favorite().alpha(0.45)).bg(Tokens::favorite().alpha(0.06)) } else { d.border_color(Tokens::border()).bg(Tokens::surface()) })
            .child(self.play_control(voice, ready, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(short_name(voice)))
                            .child(tag(if lang == "hi" { "HI" } else { "EN" }, Tone::Neutral)),
                    )
                    .child(status),
            )
            .when(confirming.is_none(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(2.))
                        .when(is_default, |d| d.child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::favorite()).child("Default")))
                        .children(own_pack.map(|p| {
                            icon_button(SharedString::from(format!("voice-rm-{voice}")), mv_ui::glyphs::DELETE, format!("Remove this voice · {}", format_bytes(self.freed_by(p))))
                                .small()
                                .danger()
                                .color(Tokens::text_3())
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.audio.confirm_voice = Some(voice);
                                    cx.notify();
                                }))
                        }))
                        // Filled gold when it is the default narrator, an outline that previews gold on
                        // hover otherwise (Fluent 2: filled marks selection).
                        .child(icon_toggle(SharedString::from(format!("default-{voice}")), ph::STAR, is_default, Tokens::favorite(), "Default narrator", "Make this the default narrator").on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                if this.audio.default_voice != voice {
                                    this.set_default_voice(voice, cx);
                                }
                            },
                        ))),
                )
            })
            .into_any_element()
    }

    /// One line at three intensities, so the effect of a scene on a voice can be heard.
    fn render_lab(&self, cx: &mut Context<Self>) -> Div {
        let voice = self.audio.lab_voice;
        let engine = Engine::of_voice(voice);
        let ready = self.readiness(voice);
        let expressive = tts::is_expressive(voice);
        let chips = div().flex().flex_wrap().gap(px(8.)).children(KINDS.iter().filter(|(k, _, _)| tts::SCENE_KINDS.contains(k)).map(|(k, label, icon)| {
            let k = *k;
            filter_chip(SharedString::from(format!("lab-kind-{k}")), *label, self.audio.lab_kind == k).icon(*icon).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.audio.lab_kind = k;
                this.audio.lab_error = None;
                cx.notify();
            }))
        }));
        let busy = self.audio.lab_busy.is_some();
        let not_ready = not_playable(ready, format!("Set up {} first", engine.map(Engine::name).unwrap_or("this voice's engine")));
        let buttons = div().flex().flex_wrap().gap(px(8.)).children(tts::LAB_INTENSITIES.iter().map(|&i| {
            let sample = self.lab_sample(i);
            let playing = sample.is_some_and(|s| player::now_playing().as_deref() == Some(s.path.as_path()));
            let label = match sample {
                Some(s) if s.duration > 0.0 => format!("{} · {i} · {:.1} s", intensity_word(i), s.duration),
                _ => format!("{} · {i}", intensity_word(i)),
            };
            btn(("lab-play", i as usize), label, ButtonKind::Secondary)
                .icon(if playing { ph::STOP } else { ph::PLAY })
                .selected(playing)
                .loading(self.audio.lab_busy == Some(i))
                .when(busy && self.audio.lab_busy != Some(i), |b| b.disabled("Rendering the takes…"))
                .when_some(not_ready.clone(), |b, why| b.disabled(why))
                .tooltip(format!("Intensity {i} of 10"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.run_lab(i, cx)))
        }));
        let note = if expressive {
            inline_alert(
                AlertKind::Warning,
                "Chatterbox uses about 4 GB of graphics memory",
                "The first take loads the model (about 25 seconds). Avoid testing while Studio narrates with Chatterbox: two copies can run out of memory.",
            )
        } else {
            inline_alert(AlertKind::Info, "Only Chatterbox acts out the scene", "Kokoro and Piper follow the scene's pace (speed), not its emotion. Pick an Expressive voice to hear the difference.")
        };
        let result = self.audio.lab.as_ref().filter(|r| r.voice == voice && r.kind == self.audio.lab_kind);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .py(px(12.))
            .child(div().flex().flex_col().gap(px(8.)).child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child("Scene")).child(chips))
            .child(note)
            .child(buttons);
        if busy {
            body = body.child(div().text_size(px(size::SM)).child(shimmer_text(if expressive {
                "Rendering three takes… the first one loads the model (about 25 s)."
            } else {
                "Rendering three takes…"
            })));
        }
        if let Some(err) = &self.audio.lab_error {
            body = body.child(inline_alert(AlertKind::Error, "The Voice Lab could not render", err.clone()));
        }
        if let Some(r) = result {
            let mut table = div().flex().flex_col().rounded(px(radius::MD)).border_1().border_color(Tokens::border()).bg(Tokens::well());
            if r.synth_voice != r.voice && !r.synth_voice.is_empty() {
                table = table.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(8.))
                        .border_b_1()
                        .border_color(Tokens::border())
                        .text_size(px(size::SM))
                        .text_color(Tokens::info())
                        .child(ph_icon(ph::INFO, Weight::Bold, Tokens::info(), icon_px::SM))
                        .child(format!("Spoken by {} (its stand-in) because {} is not set up yet.", voice_label(&r.synth_voice), engine.map(Engine::name).unwrap_or("its engine"))),
                );
            }
            for (n, s) in r.samples.iter().enumerate() {
                table = table.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(12.))
                        .py(px(8.))
                        .when(n > 0, |d| d.border_t_1().border_color(Tokens::border()))
                        .child(div().w(px(92.)).flex_none().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("{} · {}", intensity_word(s.intensity), s.intensity)))
                        .child(div().flex_1().min_w_0().truncate().font_family(mv_ui::MONO_FONT).text_size(px(size::XS)).text_color(Tokens::text_2()).child(params_text(&s.params)))
                        .child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(s.rtf.map(speed_text).unwrap_or_default())),
                );
            }
            body = body.child(table).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Line: “{}” · spoken by {}", r.text, tts::engine_label(&r.engine))));
        }
        setting_section("Voice Lab", Some("Hear how a voice delivers one line in a scene at three intensities, and the settings Studio sends the engine.".into()))
            .child(setting_row("Voice", "Expressive voices are listed first", Select::new(&self.lab_voice).w(px(280.)).menu_width(px(320.))))
            .child(divided(body))
    }

    fn render_advanced(&self, cx: &mut Context<Self>) -> Div {
        let python: Div = match self.audio.env.as_ref().map(|e| &e.python) {
            None => div().py(px(12.)).child(skeleton_line(px(220.))),
            Some(Ok(p)) => setting_row(format!("Python {}", p.version), "Runs the voice engines in their own environment (~/.mangavox/tts-venv).", status_tag(StatusKind::Done).label("Found")),
            Some(Err(_)) => div().flex().flex_col().gap(px(8.)).py(px(12.)).child(setting_row(
                "Python not found",
                "Setting up an engine installs a private copy automatically. To use your own, install Python 3.10 or newer:",
                mv_ui::external_link("python.org", "https://www.python.org/downloads/"),
            ))
            .child(command_chip("py-cmd-copy", setup::python_install_cmd())),
        };
        let studio_grade: Vec<&str> = self
            .audio
            .env
            .as_ref()
            .map(|env| [(env.engines.f5, "F5-TTS"), (env.engines.xtts, "XTTS v2"), (env.engines.parler, "Parler-TTS")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect())
            .unwrap_or_default();
        let note = if studio_grade.is_empty() {
            "F5-TTS, XTTS v2 and Parler-TTS voices need a strong GPU and are installed outside MangaVox. Without them those voices use a Kokoro or Piper stand-in.".to_string()
        } else {
            format!("Installed: {}.", studio_grade.join(", "))
        };
        let content = div()
            .flex()
            .flex_col()
            .child(python)
            .child(divided(setting_row("Studio-grade engines", note, div())))
            .child(divided(setting_row(
                "Check the engines again",
                "After installing something outside MangaVox",
                btn("engine-recheck", if self.audio.checking { "Checking…" } else { "Check again" }, ButtonKind::Secondary)
                    .small()
                    .icon(ph::ARROWS_CLOCKWISE)
                    .loading(self.audio.checking)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        voices::invalidate();
                        this.check_engines(cx);
                    })),
            )));
        div().flex().flex_col().border_t_1().border_color(Tokens::border()).pt(px(12.)).child(disclosure("audio-advanced", "Advanced: Python and other engines").content(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn every_pack_belongs_to_one_engine_card() {
        for p in PACKS {
            assert!(Engine::ALL.iter().any(|e| e.packs().any(|q| q.id == p.id)), "{}", p.id);
        }
        // Kokoro's full and Lite packs speak the same voices: each listed once.
        let k = Engine::Kokoro.voices();
        let mut dedup = k.clone();
        dedup.dedup();
        assert_eq!(k.len(), dedup.len());
        assert!(Engine::Chatterbox.voices().contains(&"cb_narrator_f"));
    }

    #[::core::prelude::v1::test]
    fn setup_keys_map_back_to_jobs() {
        for e in Engine::ALL {
            for k in e.setup_keys() {
                let t = target_for_key(k).expect(k);
                assert_eq!(t.key(), k);
                assert_eq!(t.engine_name(), e.id());
            }
        }
    }

    #[::core::prelude::v1::test]
    fn lab_text_reads_naturally() {
        assert_eq!(speed_text(0.5), "2.0× faster than real time");
        assert_eq!(speed_text(1.19), "1.2 s of work per second of audio");
        assert_eq!(speed_text(0.0), "");
        let p = serde_json::json!({"exaggeration": 0.7, "cfg_weight": 0.34});
        let t = params_text(&p);
        assert!(t.contains("exaggeration 0.70") && t.contains("cfg weight 0.34") && t.contains(" · "), "{t}");
        assert_eq!(intensity_word(2), "Gentle");
        assert_eq!(intensity_word(5), "Building");
        assert_eq!(intensity_word(8), "Intense");
        assert_eq!(lab_voice_ids()[0], "cb_narrator_f");
    }

    #[::core::prelude::v1::test]
    fn a_shared_file_is_counted_once() {
        let full = voices::find_pack("kokoro").unwrap();
        let lite = voices::find_pack("kokoro-lite").unwrap();
        assert_eq!(packs_size(&[full]), full.size());
        let shared: u64 = full.files.iter().filter(|f| lite.files.contains(f)).map(|f| f.size).sum();
        assert!(shared > 0, "the two Kokoro packs share voices-v1.0.bin");
        assert_eq!(packs_size(&[full, lite]), full.size() + lite.size() - shared);
    }

    #[::core::prelude::v1::test]
    fn disabled_reasons_and_resume_follow_the_real_state() {
        // While the check runs, a voice is not "missing an engine".
        assert_eq!(not_playable(None, "Set up Kokoro").as_deref(), Some("Checking the voice engines…"));
        assert_eq!(not_playable(Some(VoiceReady::Missing), "Set up Kokoro").as_deref(), Some("Set up Kokoro"));
        assert_eq!(not_playable(Some(VoiceReady::Fallback("Kokoro")), "x"), None);
        assert_eq!(not_playable(Some(VoiceReady::Ready("Kokoro")), "x"), None);
        // Resume only for a part-downloaded pack.
        let p = voices::find_pack("kokoro").unwrap();
        assert!(resume_label(0, p).is_none());
        assert!(resume_label(p.size(), p).is_none());
        assert!(resume_label(p.size() / 2, p).unwrap().starts_with("Resume · "));
    }
}
