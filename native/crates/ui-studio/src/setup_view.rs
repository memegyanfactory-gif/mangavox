//! Setup sheet: Python, the pipeline scripts, the voices (the packs on disk and the engine that
//! speaks them) and FFmpeg. Voices are set up in one click through `mv_studio::voice_setup`, the
//! same job Settings → Audio runs, with its live progress here too; everything else names the
//! exact command that fixes it, one click away under "Manual install".
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_studio::voice_setup::{self, SetupProgress, SetupState, Step, Target};
use mv_studio::voices::{self, VoicePack, PACKS};
use mv_studio::StudioError;
use mv_ui::theme::{icon as icon_px, radius, ramp, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, copy_button, disclosure, fmt_bytes, glyphs, icon_button, icon_ph, icon_tag, inline_alert, muted, path_chip, ph, ph_icon, progress_bar, size, spinner, AlertKind, ButtonKind, Ph, RampExt as _,
    Route, Tokens, Tone, Weight,
};

use crate::setup::{self, Env};
use crate::widgets::ago;

/// Last check, so reopening Studio shows it immediately.
static LAST_ENV: Mutex<Option<(Env, SystemTime)>> = Mutex::new(None);
/// How often the sheet looks for a newer check (Settings → Audio), a voice change, or a setup
/// job's progress.
const SYNC_EVERY: Duration = Duration::from_millis(500);
/// Registry keys of the engine-only setup jobs (packs use their own id).
const ENGINE_KEYS: [&str; 3] = ["engine:kokoro", "engine:piper", "engine:chatterbox"];

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

/// The one-click fix a row offers.
#[derive(Clone, Copy, PartialEq)]
enum Fix {
    SetUpVoices(Target),
}

pub struct SetupRow {
    id: &'static str,
    pub level: Level,
    icon: Ph,
    title: String,
    detail: Option<String>,
    /// A path shown with a copy button (the interpreter, the scripts folder).
    path: Option<String>,
    /// Voice packs on disk, shown as tags.
    packs: Vec<&'static VoicePack>,
    fix: Option<Fix>,
    commands: Vec<(&'static str, String)>,
}

pub struct SetupView {
    pub env: Option<Env>,
    pub checking: bool,
    pub error: Option<String>,
    checked_at: Option<SystemTime>,
    /// Voice packs on disk at the last check.
    packs: Vec<&'static VoicePack>,
    sync: setup::EnvSync,
}

impl SetupView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut sync = setup::EnvSync::default();
        let last = LAST_ENV.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (env, checked_at) = match sync.take_newer() {
            Some(env) => (Some(env), Some(SystemTime::now())),
            None => (last.as_ref().map(|(e, _)| e.clone()), last.map(|(_, t)| t)),
        };
        let mut v = Self { env, checking: false, error: None, checked_at, packs: installed_packs(), sync };
        v.recheck(cx);
        // Settings → Audio checks too (and voice packs change there): adopt its newer checks and
        // check again after a change, so Studio never offers a voice that Settings says is gone.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(SYNC_EVERY).await;
            if this.update(cx, |v, cx| v.follow_checks(cx)).is_err() {
                break;
            }
        })
        .detach();
        v
    }

    fn follow_checks(&mut self, cx: &mut Context<Self>) {
        // A setup job's progress moves on its own: keep its bar and step live.
        if voice_setup::any_running() {
            cx.notify();
        }
        if !self.sync.pending() {
            return;
        }
        if let Some(env) = self.sync.take_newer() {
            self.adopt(env);
            cx.notify();
        }
        if self.sync.should_check(self.checking) {
            self.recheck(cx);
        }
    }

    fn adopt(&mut self, env: Env) {
        let now = SystemTime::now();
        *LAST_ENV.lock().unwrap_or_else(|e| e.into_inner()) = Some((env.clone(), now));
        self.env = Some(env);
        self.checked_at = Some(now);
        self.packs = installed_packs();
        self.error = None;
    }

    pub fn recheck(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.sync.started();
        cx.spawn(async move |this, cx| {
            let env = mv_ui::bg::run(setup::check()).await;
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                match env {
                    // The newest finished check (this one, unless another view's is newer);
                    // nothing new means the sheet already shows the newest.
                    Ok(env) => match this.sync.take_newer() {
                        Some(newest) => this.adopt(newest),
                        None if this.env.is_none() => this.adopt(env),
                        None => {
                            this.error = None;
                            this.checked_at = Some(SystemTime::now());
                        }
                    },
                    Err(e) => this.error = Some(format!("Setup check failed: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Items needing attention (0 until the first check finishes).
    pub fn issues(&self) -> usize {
        self.env.as_ref().map(|e| setup_rows(e, &self.packs).iter().filter(|r| r.level != Level::Ok).count()).unwrap_or(0)
    }

    /// Runs one voice setup job (Python if needed, the engine, the voice files, a load check)
    /// and says how it ended. Settings → Audio shows the same job.
    fn set_up_voices(&mut self, target: Target, cx: &mut Context<Self>) {
        let key = target.key();
        if matches!(voice_setup::state(key), Some(SetupState::Running(_))) {
            return;
        }
        voice_setup::clear_error(key);
        let label = mv_studio::tts::engine_label(target.engine_name());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(voice_setup::run(target)).await;
            let _ = this.update(cx, |v, cx| {
                match res {
                    Ok(Ok(())) => toast::show(cx, Toast::success(format!("{label} is ready to narrate")).icon(glyphs::VOICE)),
                    Ok(Err(StudioError::Cancelled)) => toast::show(cx, Toast::info("Voice setup stopped. Files already in place stay.")),
                    // The row keeps the message with a Try again button.
                    Ok(Err(_)) => toast::show(cx, Toast::error(format!("{label} setup did not finish"))),
                    Err(e) => toast::show(cx, Toast::error(format!("{label} setup did not finish: {e:#}"))),
                }
                v.recheck(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn row(&self, row: SetupRow, cx: &mut Context<Self>) -> Div {
        let (status, color) = match row.level {
            Level::Ok => (ph::CHECK_CIRCLE, Tokens::success()),
            Level::Warn => (ph::WARNING, Tokens::warning()),
            Level::Error => (ph::X_CIRCLE, Tokens::danger()),
        };
        let SetupRow { id, icon, title, detail, path, packs, fix, commands, .. } = row;
        let n_cmds = commands.len();
        div()
            .flex()
            .items_start()
            .gap(px(space::MD))
            .py(px(space::MD))
            .border_t_1()
            .border_color(Tokens::border())
            .child(div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD)).bg(Tokens::hover()).child(icon_ph(icon, Tokens::text_2(), icon_px::MD)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .min_h(px(20.))
                            .child(ph_icon(status, Weight::Fill, color, icon_px::SM))
                            .child(div().flex_1().min_w_0().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title)),
                    )
                    .when_some(detail, |d, t| d.child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_3()).child(t)))
                    .when_some(path, |d, p| d.child(div().max_w(px(520.)).child(path_chip(p))))
                    .when(!packs.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().gap(px(space::XS)).children(packs.iter().map(|p| icon_tag(ph::CHECK_CIRCLE, format!("{} · {}", p.name, fmt_bytes(p.size())), Tone::Success))))
                    })
                    .when_some(fix, |d, f| d.child(self.fix_view(f, cx)))
                    .when(n_cmds > 0, |d| {
                        d.child(
                            disclosure(SharedString::from(format!("manual-{id}")), if fix.is_some() { "Manual install (terminal)" } else { "How to fix it (terminal)" })
                                .default_open(fix.is_none())
                                .content(div().flex().flex_col().gap(px(space::SM)).children(commands.into_iter().enumerate().map(|(i, (label, cmd))| command_box(format!("{id}-{i}"), label, cmd)))),
                        )
                    }),
            )
    }

    /// "Set up voices" and the live state of the job that runs it.
    fn fix_view(&self, fix: Fix, cx: &mut Context<Self>) -> AnyElement {
        let Fix::SetUpVoices(target) = fix;
        // Any voice setup that runs (started here or in Settings → Audio) shows here.
        let running = PACKS.iter().map(|p| p.id).chain(ENGINE_KEYS).find_map(|k| match voice_setup::state(k) {
            Some(SetupState::Running(p)) => Some((k, p)),
            _ => None,
        });
        if let Some((key, p)) = running {
            let name = voices::find_pack(key).map(|p| p.name).unwrap_or("Voice engine");
            return div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(space::MD))
                .rounded(px(radius::MD))
                .bg(Tokens::well())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(spinner("setup-voices-spin", Tokens::accent(), icon_px::SM))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("Setting up {name}")))
                        .child(btn("setup-voices-stop", "Stop", ButtonKind::Ghost).small().icon(ph::STOP_CIRCLE).on_click(move |_, _, _| voice_setup::cancel(key))),
                )
                .child(progress_bar("setup-voices-bar", setup_fraction(&p), true))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(p.summary()))
                .into_any_element();
        }
        if let Some(SetupState::Failed(msg)) = voice_setup::state(target.key()) {
            return inline_alert(AlertKind::Error, "Voice setup did not finish", msg)
                .action("Try again", cx.listener(move |this, _: &ClickEvent, _, cx| this.set_up_voices(target, cx)))
                .into_any_element();
        }
        let what = match target {
            Target::Pack(p) if voices::installed(p) => format!("Installs the {} engine ({}) for {}.", mv_studio::tts::engine_label(voices::engine_of(p)), voice_setup::engine_size_hint(voices::engine_of(p)), p.name),
            Target::Pack(p) => format!("Downloads {} ({}) and installs its engine ({}). Python comes too when this PC has none.", p.name, fmt_bytes(p.size()), voice_setup::engine_size_hint(voices::engine_of(p))),
            Target::Engine(e) => format!("Installs the {} engine ({}).", mv_studio::tts::engine_label(e), voice_setup::engine_size_hint(e)),
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(btn("setup-voices", "Set up voices", ButtonKind::Primary).icon(glyphs::DOWNLOAD).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_up_voices(target, cx))))
            .child(btn("setup-audio-settings", "Pick other voices", ButtonKind::Ghost).icon(glyphs::SETTINGS).tooltip("Settings → Audio: every voice pack and engine").on_click(|_, w, cx| {
                mv_ui::navigate(Route::Settings { tab: Some("audio".into()) }, w, cx);
            }))
            .child(div().w_full().child(muted(what)))
            .into_any_element()
    }
}

impl Render for SetupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.env.as_ref().map(|e| setup_rows(e, &self.packs)).unwrap_or_default();
        let issues = rows.iter().filter(|r| r.level != Level::Ok).count();
        let subtitle = match (&self.env, issues) {
            (None, _) => "Checking Python, voices and tools…".to_string(),
            (Some(_), 0) => "Everything Studio needs is installed.".to_string(),
            (Some(_), n) => format!("{n} item{} need{} attention.", if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" }),
        };
        let checked = match (self.checking, self.checked_at) {
            (true, _) => "Checking…".to_string(),
            (false, Some(t)) => format!("Checked {}", ago(t)),
            (false, None) => String::new(),
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .pb(px(space::MD))
                    .child(div().flex().flex_none().items_center().justify_center().size(px(36.)).rounded(px(radius::MD)).bg(Tokens::accent_soft()).child(ph_icon(ph::WRENCH, Weight::Duotone, Tokens::accent(), icon_px::LG)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child("Setup"))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(subtitle)),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(checked))
                    .child(if self.checking {
                        div().flex().items_center().justify_center().size(px(32.)).child(spinner("studio-recheck-spin", Tokens::text_2(), icon_px::MD)).into_any_element()
                    } else {
                        icon_button("studio-recheck", glyphs::REFRESH, "Check again").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.recheck(cx))).into_any_element()
                    }),
            )
            .when_some(self.error.clone(), |d, e| d.child(div().pb(px(space::SM)).child(inline_alert(AlertKind::Error, "The check failed", e))))
            .when(self.env.is_none(), |d| d.children((0..3).map(|i| div().flex().items_center().gap(px(space::MD)).py(px(space::MD)).border_t_1().border_color(Tokens::border()).child(mv_ui::skeleton(px(32.), px(32.), radius::MD)).child(mv_ui::skeleton_line(relative(if i == 1 { 0.5 } else { 0.35 }))))))
            .children(rows.into_iter().map(|r| self.row(r, cx)))
    }
}

/// A shell command with its label and a copy button.
fn command_box(id: String, label: &'static str, cmd: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(space::XS))
        .child(div().flex().items_center().gap(px(6.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(icon_ph(ph::TERMINAL_WINDOW, Tokens::text_3(), icon_px::XS)).child(label))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(space::SM))
                .pl(px(10.))
                .pr(px(space::XS))
                .py(px(space::XS))
                .rounded(px(radius::MD))
                .bg(Tokens::well())
                .border_1()
                .border_color(Tokens::border())
                .child(div().flex_1().min_w_0().py(px(3.)).font_family(crate::page::MONO).text_size(px(size::SM)).text_color(Tokens::text_2()).child(cmd.clone()))
                .child(copy_button(SharedString::from(format!("copy-{id}")), cmd)),
        )
}

/// Overall progress of a setup job: its step, and the download within the model step.
fn setup_fraction(p: &SetupProgress) -> f32 {
    let count = Step::COUNT as f32;
    let within = p.download.as_ref().map(|d| d.fraction()).unwrap_or(0.);
    ((p.step.number() as f32 - 1. + within) / count).clamp(0., 1.)
}

/// Voice packs whose files are on disk.
fn installed_packs() -> Vec<&'static VoicePack> {
    PACKS.iter().filter(|p| voices::installed(p)).collect()
}

fn setup_rows(env: &Env, packs: &[&'static VoicePack]) -> Vec<SetupRow> {
    let row = |id, level, icon, title: String| SetupRow { id, level, icon, title, detail: None, path: None, packs: vec![], fix: None, commands: vec![] };
    let mut rows = vec![];
    match &env.python {
        Ok(p) => rows.push(SetupRow { path: Some(p.command_line()), ..row("python", Level::Ok, ph::TERMINAL_WINDOW, format!("Python {}", p.version)) }),
        Err(e) => rows.push(SetupRow {
            detail: Some(format!("{e}. Install Python, reopen MangaVox, then check again.")),
            commands: vec![("Install Python", setup::python_install_cmd().into())],
            ..row("python", Level::Error, ph::TERMINAL_WINDOW, "Python 3.10 or newer not found".into())
        }),
    }
    match &env.scripts {
        Ok(l) => rows.push(SetupRow { path: Some(l.dir.display().to_string()), ..row("scripts", Level::Ok, glyphs::FOLDER, "Pipeline scripts".into()) }),
        Err(e) => rows.push(SetupRow {
            detail: Some(format!("{e}. Keep the python/ folder next to the app, or set MANGAVOX_PYTHON_DIR to its path.")),
            ..row("scripts", Level::Error, glyphs::FOLDER, "Pipeline scripts not found".into())
        }),
    }
    if env.runnable() {
        let packs = packs.to_vec();
        if env.engines.any() {
            rows.push(SetupRow { detail: Some(format!("Narrating with {}.", env.engines.names().join(", "))), packs, ..row("voices", Level::Ok, glyphs::VOICE, "Voices ready".into()) });
        } else {
            // One click sets up what is missing: the engine for packs already downloaded, else
            // the recommended pack and its engine.
            let mut commands = vec![];
            if let Some(c) = env.tts_packages_cmd() {
                commands.push(("Install the voice engines (Kokoro + Piper)", c));
            }
            if let Some(c) = env.voice_models_cmd() {
                commands.push(("Download the voice models (about 450 MB, once)", c));
            }
            let (title, detail) = match (&env.tts_error, packs.is_empty()) {
                (Some(e), _) => ("The voice check failed".to_string(), e.clone()),
                (None, false) => ("Voices downloaded, engine not installed".to_string(), "The voice files are here; the engine that speaks them (Python packages) is missing.".to_string()),
                (None, true) => ("No voices installed".to_string(), "Narration needs a local voice: a voice pack and the engine that speaks it.".to_string()),
            };
            rows.push(SetupRow { detail: Some(detail), packs, fix: Some(Fix::SetUpVoices(Target::Pack(voice_setup::suggested_pack()))), commands, ..row("voices", Level::Error, glyphs::VOICE, title) });
        }
    }
    rows.push(if env.ffmpeg {
        row("ffmpeg", Level::Ok, glyphs::VIDEO, "FFmpeg".into())
    } else {
        SetupRow {
            detail: Some("Needed to render the video. Audio still works.".into()),
            commands: vec![("Install FFmpeg", setup::ffmpeg_install_cmd().into())],
            ..row("ffmpeg", Level::Warn, glyphs::VIDEO, "FFmpeg not found".into())
        }
    });
    rows
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{setup_fraction, SetupProgress, Step};
    use mv_studio::voices::Progress;

    #[test]
    fn setup_progress_spans_the_steps() {
        let p = |step, download| SetupProgress { step, detail: String::new(), download };
        assert_eq!(setup_fraction(&p(Step::Runtime, None)), 0.);
        assert_eq!(setup_fraction(&p(Step::Model, Some(Progress { done: 50, total: 100, bytes_per_sec: 0. }))), 0.625);
        assert_eq!(setup_fraction(&p(Step::Verify, None)), 0.75);
    }
}
