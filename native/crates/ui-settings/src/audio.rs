//! Settings → Audio: narration voice packs (download, resume, remove), a sample for every
//! voice, the default narrator Studio uses, and the voice engines the packs run on.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_studio::voices::{self, DownloadState, VoicePack, PACKS};
use mv_studio::{tts, CancelToken};
use mv_ui::{button, icon, size, ButtonKind, Tokens};
use ui_studio::audio as player;
use ui_studio::setup::{self, Env, VoiceReady};

use crate::ui::{card, divided, format_bytes, row, tag};
use crate::SettingsView;

/// What the Audio page knows. Pack state is cached and refreshed after every change.
#[derive(Default)]
pub struct AudioState {
    loaded: bool,
    /// `None` while the engine check runs.
    env: Option<Env>,
    checking: bool,
    default_voice: String,
    installed: HashMap<&'static str, bool>,
    partial: HashMap<&'static str, u64>,
    previewing: Option<&'static str>,
    /// Engine being installed and its latest step.
    installing: Option<(&'static str, Arc<Mutex<String>>)>,
    install_cancel: Option<CancelToken>,
    /// Downloads seen running on the last tick, to refresh when they end.
    running: Vec<&'static str>,
}

impl AudioState {
    /// True while something on the page changes on its own (download, install, sample).
    pub fn busy(&self) -> bool {
        !self.running.is_empty() || voices::any_running() || self.installing.is_some() || self.previewing.is_some() || player::now_playing().is_some()
    }
}

fn dots(n: u8) -> String {
    (1..=5).map(|i| if i <= n { '●' } else { '○' }).collect()
}

fn voice_label(id: &str) -> String {
    tts::find_voice(id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string())
}

impl SettingsView {
    /// First visit: read pack state and check the engines.
    pub(crate) fn audio_open(&mut self, cx: &mut Context<Self>) {
        if self.audio.loaded {
            return;
        }
        self.audio.loaded = true;
        self.refresh_packs(cx);
        self.check_engines(cx);
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
        cx.spawn(async move |this, cx| {
            let env = mv_ui::bg::run(setup::check()).await.ok();
            this.update(cx, |v, cx| {
                v.audio.checking = false;
                if env.is_some() {
                    v.audio.env = env;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Called by the page ticker: repaints progress and refreshes when downloads end.
    pub(crate) fn audio_tick(&mut self, cx: &mut Context<Self>) {
        let now: Vec<&'static str> = PACKS.iter().filter(|p| matches!(voices::download_state(p.id), Some(DownloadState::Running(_)))).map(|p| p.id).collect();
        let ended = self.audio.running.iter().any(|id| !now.contains(id));
        self.audio.running = now;
        if ended {
            self.refresh_packs(cx);
            self.check_engines(cx);
        }
        cx.notify();
    }

    fn download_pack(&mut self, id: &'static str, cx: &mut Context<Self>) {
        voices::clear_error(id);
        self.audio.running.push(id);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(voices::run_download(id)).await;
            this.update(cx, |v, cx| {
                let name = voices::find_pack(id).map(|p| p.name).unwrap_or(id);
                match res {
                    Ok(Ok(())) if voices::installed(voices::find_pack(id).expect("pack")) => v.report(&format!("{name} downloaded"), Ok(()), cx),
                    // Cancelled or failed: the pack card shows it.
                    _ => cx.notify(),
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn cancel_pack(&mut self, id: &'static str, cx: &mut Context<Self>) {
        voices::cancel_download(id);
        self.refresh_packs(cx);
    }

    fn remove_pack(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let Some(pack) = voices::find_pack(id) else { return };
        player::stop();
        self.background(cx, move || voices::remove(pack).map_err(anyhow::Error::from), move |v, r, cx| {
            v.report(&format!("{} removed", pack.name), r, cx);
            v.refresh_packs(cx);
            v.check_engines(cx);
        });
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
        let speed = 1.0;
        let cached = tts::preview_cache_path(&tts::default_samples_dir(), voice, tts::default_preview_engine(voice), speed);
        if player::now_playing().as_deref() == Some(cached.as_path()) {
            player::stop();
            return cx.notify();
        }
        let Some(env) = self.audio.env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else {
            self.report("", Err(anyhow::anyhow!("Samples need Python and the voice scripts; see Voice engines below")), cx);
            return;
        };
        self.audio.previewing = Some(voice);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { tts::voice_preview(&py, &layout, &tts::default_samples_dir(), &std::env::temp_dir(), voice, speed, None).await }).await;
            this.update(cx, |v, cx| {
                v.audio.previewing = None;
                match res {
                    Ok(Ok(p)) => {
                        v.notice = None;
                        player::play(&p.path);
                    }
                    Ok(Err(e)) => v.report("", Err(anyhow::anyhow!("Sample failed: {e}")), cx),
                    Err(e) => v.report("", Err(anyhow::anyhow!("Sample failed: {e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn install_engine(&mut self, engine: &'static str, cx: &mut Context<Self>) {
        let Some(env) = self.audio.env.clone() else { return };
        let (Ok(py), Ok(layout)) = (env.python.clone(), env.scripts.clone()) else {
            self.report("", Err(anyhow::anyhow!("Install Python 3.10+ first: {}", setup::python_install_cmd())), cx);
            return;
        };
        let step = Arc::new(Mutex::new("Starting…".to_string()));
        let cancel = CancelToken::new();
        self.audio.installing = Some((engine, step.clone()));
        self.audio.install_cancel = Some(cancel.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                tts::install_engine(&py, &layout, engine, &cancel, move |s| {
                    *step.lock().unwrap_or_else(|e| e.into_inner()) = s.to_string();
                })
                .await
            })
            .await;
            this.update(cx, |v, cx| {
                v.audio.installing = None;
                v.audio.install_cancel = None;
                let label = tts::engine_label(engine);
                match res {
                    Ok(Ok(())) => v.report(&format!("{label} engine installed"), Ok(()), cx),
                    Ok(Err(mv_studio::StudioError::Cancelled)) => cx.notify(),
                    Ok(Err(e)) => v.report("", Err(anyhow::anyhow!("{label} install failed: {e}")), cx),
                    Err(e) => v.report("", Err(e), cx),
                }
                v.check_engines(cx);
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
            .gap(px(16.))
            .child(self.render_default_voice(cx))
            .child(self.render_engines(cx))
            .child(self.render_packs(cx))
    }

    fn render_default_voice(&self, cx: &mut Context<Self>) -> Div {
        let id = self.audio.default_voice.clone();
        let label = if id.is_empty() { "…".to_string() } else { voice_label(&id) };
        card("Default narrator", Some("Studio narrates every chapter with this voice unless you pick another one for that chapter."))
            .child(row(&label, "Pick a voice below with Set as default. Press ▶ on any voice to hear it.", match tts::find_voice(&id) {
                Some(v) => self.play_button(v.id, cx).into_any_element(),
                None => div().into_any_element(),
            }))
    }

    fn engine_row(&self, engine: &'static str, label: &str, ready: bool, hint: &str, cx: &mut Context<Self>) -> Div {
        let installing = self.audio.installing.as_ref().filter(|(e, _)| *e == engine).map(|(_, s)| s.lock().unwrap_or_else(|e| e.into_inner()).clone());
        let control = if let Some(step) = installing {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().max_w(px(260.)).truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(step))
                .child(button(SharedString::from(format!("engine-cancel-{engine}")), "Cancel", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(c) = this.audio.install_cancel.take() {
                        c.cancel();
                    }
                    cx.notify();
                })))
                .into_any_element()
        } else if ready {
            tag("Installed", Tokens::accent()).into_any_element()
        } else {
            let can = self.audio.env.as_ref().is_some_and(|e| e.runnable()) && self.audio.installing.is_none();
            button(SharedString::from(format!("engine-install-{engine}")), "Install", ButtonKind::Secondary)
                .when(!can, |b| b.opacity(0.5))
                .when(can, |b| b.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.install_engine(engine, cx))))
                .into_any_element()
        };
        row(label, hint, control)
    }

    fn render_engines(&self, cx: &mut Context<Self>) -> Div {
        let mut c = card("Voice engines", Some("The programs that turn text into speech. They run on this PC, offline."));
        let Some(env) = &self.audio.env else {
            return c.child(row("Checking Python and voice engines…", "This takes a few seconds.", div()));
        };
        let python = match &env.python {
            Ok(p) => row(&format!("Python {}", p.version), "Runs the voice engines in their own environment (~/.mangavox/tts-venv).", tag("Found", Tokens::accent())),
            Err(e) => row("Python not found", e, div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(setup::python_install_cmd())),
        };
        c = c
            .child(python)
            .child(divided(self.engine_row("kokoro", "Kokoro", env.engines.kokoro_deps, "Recommended. Natural voices, fast on CPU. About 60 MB of Python packages.", cx)))
            .child(divided(self.engine_row("piper", "Piper", env.engines.piper_deps, "Lightest and fastest; sounds more synthetic. About 40 MB.", cx)))
            .child(divided(self.engine_row(
                "chatterbox",
                "Chatterbox (expressive)",
                env.engines.chatterbox_deps,
                "Acts out each scene from the story: soft for calm pages, intense for fights. Its own Python and PyTorch (about 5 GB, plus 3.2 GB of voice files); an NVIDIA graphics card makes it fast.",
                cx,
            )));
        let studio_grade: Vec<&str> = [(env.engines.f5, "F5-TTS"), (env.engines.xtts, "XTTS v2"), (env.engines.parler, "Parler-TTS")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
        let note = if studio_grade.is_empty() {
            "F5-TTS, XTTS v2 and Parler-TTS voices sound closest to a studio narrator but need a strong GPU and several GB. Without them those voices fall back to Kokoro or Piper.".to_string()
        } else {
            format!("Studio-grade engines installed: {}.", studio_grade.join(", "))
        };
        c.child(divided(
            row("Studio-grade engines", &note, button("engine-recheck", if self.audio.checking { "Checking…" } else { "Check again" }, ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.check_engines(cx)))),
        ))
    }

    fn render_packs(&self, cx: &mut Context<Self>) -> Div {
        let mut c = card("Voice packs", Some("Download the voices you want. Downloads resume if interrupted and are saved in ~/.mangavox/models."));
        for (i, pack) in PACKS.iter().enumerate() {
            let block = self.render_pack(pack, cx);
            c = c.child(if i == 0 { block } else { block.border_t_1().border_color(Tokens::border()) });
        }
        c
    }

    fn render_pack(&self, pack: &'static VoicePack, cx: &mut Context<Self>) -> Div {
        let id = pack.id;
        let installed = self.audio.installed.get(id).copied().unwrap_or(false);
        let partial = self.audio.partial.get(id).copied().unwrap_or(0);
        let state = voices::download_state(id);
        let action: AnyElement = match &state {
            Some(DownloadState::Running(p)) => {
                let pct = (p.fraction() * 100.0).round();
                let speed = if p.bytes_per_sec > 0.0 { format!(" · {}/s", format_bytes(p.bytes_per_sec as u64)) } else { String::new() };
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{pct}% · {} of {}{speed}", format_bytes(p.done), format_bytes(p.total))))
                            .child(button(SharedString::from(format!("pack-cancel-{id}")), "Pause", ButtonKind::Ghost).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cancel_pack(id, cx)))),
                    )
                    .child(
                        div().w(px(240.)).h(px(4.)).rounded(px(2.)).bg(Tokens::surface_2()).child(div().h_full().rounded(px(2.)).bg(Tokens::accent()).w(relative(p.fraction()))),
                    )
                    .into_any_element()
            }
            _ if installed => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(tag("Installed", Tokens::accent()))
                .child(button(SharedString::from(format!("pack-remove-{id}")), "Remove", ButtonKind::Ghost).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_pack(id, cx))))
                .into_any_element(),
            other => {
                let label = if partial > 0 {
                    format!("Resume ({} of {})", format_bytes(partial), format_bytes(pack.size()))
                } else {
                    format!("Download · {}", format_bytes(pack.size()))
                };
                let failed = match other {
                    Some(DownloadState::Failed(e)) => Some(e.clone()),
                    _ => None,
                };
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(4.))
                    .child(
                        button(SharedString::from(format!("pack-get-{id}")), label, if pack.recommended { ButtonKind::Primary } else { ButtonKind::Secondary })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.download_pack(id, cx))),
                    )
                    .when_some(failed, |d, e| d.child(div().max_w(px(320.)).text_size(px(size::XS)).text_color(Tokens::danger()).child(e)))
                    .into_any_element()
            }
        };
        let head = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(pack.name))
                            .when(pack.recommended, |d| d.child(tag("Recommended", Tokens::accent()))),
                    )
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(pack.summary))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!(
                        "Quality {}   Speed {}   {} · {} · {} voice{}",
                        dots(pack.quality),
                        dots(pack.speed),
                        pack.engine,
                        pack.languages,
                        pack.voice_ids().len(),
                        if pack.voice_ids().len() == 1 { "" } else { "s" }
                    ))),
            )
            .child(div().flex_none().child(action));
        let mut block = div().flex().flex_col().gap(px(10.)).py(px(14.)).child(head);
        if installed {
            block = block.child(div().flex().flex_wrap().gap(px(8.)).children(pack.voice_ids().into_iter().map(|v| self.voice_tile(v, cx))));
        }
        block
    }

    fn readiness(&self, voice: &'static str) -> Option<VoiceReady> {
        let env = self.audio.env.as_ref()?;
        tts::find_voice(voice).map(|v| setup::voice_ready(&env.engines, v))
    }

    fn play_button(&self, voice: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        let previewing = self.audio.previewing == Some(voice);
        let playing = player::now_playing().is_some_and(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{}_", tts::safe_voice_name(voice)))));
        let usable = self.readiness(voice).is_some_and(VoiceReady::usable);
        div()
            .id(SharedString::from(format!("sample-{voice}")))
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(Tokens::surface_2())
            .when(!usable, |d| d.opacity(0.4))
            .when(usable, |d| {
                d.cursor_pointer().hover(|s| s.bg(Tokens::active())).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.play_sample(voice, cx);
                }))
            })
            .child(icon(
                if previewing {
                    "icons/loader-circle.svg"
                } else if playing {
                    "icons/square.svg"
                } else {
                    "icons/play.svg"
                },
                Tokens::text_1(),
                13.,
            ))
    }

    fn voice_tile(&self, voice: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let is_default = self.audio.default_voice == voice;
        let (status, color) = match self.readiness(voice) {
            None => ("Checking…".to_string(), Tokens::text_3()),
            Some(VoiceReady::Ready(e)) => (format!("Ready · {e}"), Tokens::text_3()),
            Some(VoiceReady::Fallback(e)) => (format!("Uses {e} for now"), Tokens::warning()),
            Some(VoiceReady::Missing) => ("Install its engine above".to_string(), Tokens::warning()),
        };
        div()
            .id(SharedString::from(format!("voice-{voice}")))
            .flex()
            .items_center()
            .gap(px(10.))
            // Two per row in the 680 px column.
            .w(px(336.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .border_1()
            .map(|d| if is_default { d.border_color(Tokens::accent()).bg(Tokens::accent_soft()) } else { d.border_color(Tokens::border()) })
            .child(self.play_button(voice, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(voice_label(voice)))
                    .child(div().truncate().text_size(px(size::XS)).text_color(color).child(status)),
            )
            .child(if is_default {
                icon("icons/star.svg", Tokens::accent(), 13.).into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("default-{voice}")))
                    .flex_none()
                    .text_size(px(size::XS))
                    .text_color(Tokens::accent())
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.8))
                    .child("Set as default")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_default_voice(voice, cx)))
                    .into_any_element()
            })
            .into_any_element()
    }
}
