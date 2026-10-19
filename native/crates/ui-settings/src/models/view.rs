//! Model Library view: four primary pages in a segmented control (In use, On this PC, Get models,
//! Engines), category filter chips on "On this PC", a folder and hardware strip, then the page.
//! Hick's law, fewer clearer choices: <https://lawsofux.com/hicks-law/>.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::HubSystemFit;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, empty_view, filter_chip, glyphs, icon, icon_button, icon_tag, ph, ph_icon, segmented, size, skeleton, skeleton_line, skeleton_row, tag, ButtonKind, Hue, Ph, Tokens, Tone, Weight};

use super::jobs::{self, Choice, Job, Selections, Status};
use super::scan::{self, Category, Engine, EngineId, EngineState, Inventory, LocalModel, Source};
use crate::ui::{format_bytes, hero_tile, page_head, state_pill, surface};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    InUse,
    Downloaded,
    Downloads,
    Engines,
    Category(Category),
}

impl Page {
    /// Sub-page ids accepted after `models:` in the settings tab id.
    pub fn from_id(id: &str) -> Option<Page> {
        Some(match id {
            "in-use" | "inuse" => Page::InUse,
            "downloaded" | "installed" | "on-this-pc" => Page::Downloaded,
            "downloads" | "hub" | "get" => Page::Downloads,
            "engines" => Page::Engines,
            "language" => Page::Category(Category::Language),
            "voice" => Page::Category(Category::Voice),
            "transcription" => Page::Category(Category::Transcription),
            "ocr" => Page::Category(Category::Ocr),
            "image" => Page::Category(Category::ImageGen),
            "audio" => Page::Category(Category::Audio),
            _ => return None,
        })
    }

    /// Index in the primary segmented control (categories live under "On this PC").
    fn primary(self) -> usize {
        match self {
            Page::InUse => 0,
            Page::Downloaded | Page::Category(_) => 1,
            Page::Downloads => 2,
            Page::Engines => 3,
        }
    }
}

const PRIMARY: [Page; 4] = [Page::InUse, Page::Downloaded, Page::Downloads, Page::Engines];

/// Asks the Settings page to switch tabs (e.g. "Use my own…" opens AI providers).
pub enum LibraryEvent {
    OpenTab(&'static str),
}

type Picker = Entity<SelectState<Vec<SharedString>>>;

pub struct ModelLibrary {
    page: Page,
    inv: Option<Inventory>,
    scanning: bool,
    system: Option<HubSystemFit>,
    sel: Selections,
    lists: HashMap<Job, Vec<Choice>>,
    pickers: HashMap<Job, Picker>,
    hub: Option<AnyView>,
    busy: HashSet<EngineId>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for ModelLibrary {}

impl ModelLibrary {
    pub fn new(page: Option<Page>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            page: page.unwrap_or(Page::InUse),
            inv: None,
            scanning: false,
            system: None,
            sel: Selections::default(),
            lists: HashMap::new(),
            pickers: HashMap::new(),
            hub: None,
            busy: HashSet::new(),
            _subs: vec![],
        };
        this.ensure_hub(window, cx);
        this.rescan(window, cx);
        cx.spawn(async move |this, cx| {
            let sys = mv_ui::bg::blocking(ui_assistant::detect_system).await.ok();
            this.update(cx, |t, cx| {
                t.system = sys;
                cx.notify();
            })
            .ok();
        })
        .detach();
        this
    }

    pub fn set_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.ensure_hub(window, cx);
        cx.notify();
    }

    fn ensure_hub(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Downloads && self.hub.is_none() {
            self.hub = Some(ui_assistant::model_hub_panel(window, cx));
        }
    }

    fn rescan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = mv_ui::bg::blocking(|| {
                let dir = ui_assistant::models_dir();
                (scan::scan(dir), jobs::load_selections())
            })
            .await;
            this.update_in(cx, |t, window, cx| {
                t.scanning = false;
                if let Ok((inv, sel)) = result {
                    t.apply(inv, sel, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rereads what each job uses without scanning the disk again: Settings → Audio's star and the
    /// assistant's model picker change the same settings while this page stays open, and the job
    /// cards showed the old narrator until a Rescan.
    pub fn refresh_selections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inv.is_none() || self.scanning {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Ok(sel) = mv_ui::bg::blocking(jobs::load_selections).await else { return };
            this.update_in(cx, |t, window, cx| {
                if let Some(inv) = t.inv.clone().filter(|_| !t.scanning) {
                    t.apply(inv, sel, window, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, inv: Inventory, mut sel: Selections, window: &mut Window, cx: &mut Context<Self>) {
        let (provider, model) = ui_assistant::assistant_model(cx);
        if !model.is_empty() {
            sel.assistant = Some((provider, model));
        }
        self.sel = sel;
        for job in Job::ALL {
            let list = jobs::choices(job, &inv);
            let st = jobs::status(job, &self.sel, &inv, &list);
            let selected = match &st {
                Status::Uses { model_id, .. } => list.iter().position(|c| c.model_id == *model_id && (job != Job::Voice || Some(&c.value) == self.sel.voice.as_ref())),
                _ => None,
            };
            let labels: Vec<SharedString> = list.iter().map(|c| SharedString::from(c.label.clone())).collect();
            match self.pickers.get(&job) {
                Some(p) => p.update(cx, |s, cx| {
                    s.set_items(labels, window, cx);
                    s.set_selected_index(selected.map(IndexPath::new), window, cx);
                }),
                None => {
                    let p = cx.new(|cx| SelectState::new(labels, selected.map(IndexPath::new), window, cx));
                    let sub = cx.subscribe_in(&p, window, move |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
                        if let SelectEvent::Confirm(Some(label)) = ev {
                            let value = this.lists.get(&job).and_then(|l| l.iter().find(|c| c.label == label.as_ref())).map(|c| c.value.clone());
                            if let Some(v) = value {
                                this.choose(job, v, cx);
                            }
                        }
                    });
                    self._subs.push(sub);
                    self.pickers.insert(job, p);
                }
            }
            self.lists.insert(job, list);
        }
        self.inv = Some(inv);
    }

    /// Makes `value` the model for `job` and stores it where the feature reads it.
    fn choose(&mut self, job: Job, value: String, cx: &mut Context<Self>) {
        if job == Job::Assistant {
            if let Some((p, m)) = value.split_once("::") {
                ui_assistant::set_assistant_model(p, m, cx);
            }
        }
        self.sel.set(job, value.clone());
        let label = self.lists.get(&job).and_then(|l| l.iter().find(|c| c.value == value)).map(|c| c.label.clone()).unwrap_or_else(|| value.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || jobs::save_choice(job, &value)).await.and_then(|r| r);
            this.update(cx, |_, cx| {
                toast::show(
                    cx,
                    match r {
                        Ok(()) if job.wired() => Toast::success(format!("{} now uses {label}", job.title())),
                        Ok(()) => Toast::success(format!("Saved {label} for {}", job.title().to_lowercase())),
                        Err(e) => Toast::error(format!("Could not save: {e:#}")),
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Picks a model from an "On this PC" row and updates that job's dropdown.
    fn use_for(&mut self, job: Job, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(p), Some(i)) = (self.pickers.get(&job), self.lists.get(&job).and_then(|l| l.iter().position(|c| c.value == value))) {
            p.update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(i)), window, cx));
        }
        self.choose(job, value, cx);
    }

    fn start_engine(&mut self, id: EngineId, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.inv.as_ref().and_then(|i| i.engine(id)).and_then(|e| e.path.clone());
        self.busy.insert(id);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let r = mv_ui::bg::blocking(move || start_engine(id, path)).await.and_then(|r| r);
            if r.is_ok() {
                // Give the server a moment to bind its port before re-checking.
                cx.background_executor().timer(Duration::from_secs(4)).await;
            }
            this.update_in(cx, |t, window, cx| {
                t.busy.remove(&id);
                if let Err(e) = r {
                    toast::show(cx, Toast::error(format!("Could not start {}: {e:#}", short_name(id))));
                }
                t.rescan(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn change_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let d = dir.to_string_lossy().into_owned();
            let r = mv_ui::bg::blocking(move || crate::data::save_models_dir(&d)).await.and_then(|r| r);
            this.update_in(cx, |t, window, cx| {
                toast::show(
                    cx,
                    match r {
                        Ok(()) => Toast::success(format!("Models folder set to {}", dir.display())),
                        Err(e) => Toast::error(format!("{e:#}")),
                    },
                );
                t.rescan(window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn open_audio(&mut self, cx: &mut Context<Self>) {
        cx.emit(LibraryEvent::OpenTab("audio"));
    }

    // ── Sub-navigation ──────────────────────────────────────────────────────

    fn render_subnav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.inv.as_ref().map(|i| i.models.len());
        let on_pc = match total {
            Some(n) => format!("On this PC · {n}"),
            None => "On this PC".into(),
        };
        let pages = segmented(
            "ml-pages",
            [(Some(ph::CHECK_CIRCLE), "In use".to_string()), (Some(glyphs::STORAGE), on_pc), (Some(glyphs::DOWNLOAD), "Get models".to_string()), (Some(ph::HARD_DRIVES), "Engines".to_string())],
            self.page.primary(),
        )
        .on_change(cx.listener(|this, ix: &usize, window, cx| this.set_page(PRIMARY[*ix], window, cx)));
        let top = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(pages)
            .child(div().flex_1())
            .child(
                btn("ml-rescan", if self.scanning { "Scanning…" } else { "Rescan" }, ButtonKind::Ghost)
                    .small()
                    .icon(glyphs::REFRESH)
                    .loading(self.scanning)
                    .tooltip("Look for models on this PC again")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.rescan(window, cx))),
            );

        // Categories filter "On this PC" (Material 3 filter chips).
        let categories = matches!(self.page, Page::Downloaded | Page::Category(_)).then(|| {
            let mut chips = div().flex().flex_wrap().gap(px(6.)).child(
                filter_chip("ml-cat-all", "All", self.page == Page::Downloaded).count(total.unwrap_or(0)).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloaded, window, cx))),
            );
            for (i, c) in Category::ALL.into_iter().enumerate() {
                let n = self.inv.as_ref().map(|inv| inv.count(c)).unwrap_or(0);
                if c == Category::Other && n == 0 {
                    continue;
                }
                chips = chips.child(
                    filter_chip(("ml-cat", i), c.label(), self.page == Page::Category(c))
                        .icon(c.icon())
                        .count(n)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_page(Page::Category(c), window, cx))),
                );
            }
            chips
        });

        let folder = self.inv.as_ref().map(|i| i.models_dir.display().to_string());
        let free = self.inv.as_ref().and_then(|i| i.free_bytes).map(|b| format!("{} free", format_bytes(b)));
        let hw_text = self.system.as_ref().map(|h| {
            let gpu = h.gpu_name.clone().unwrap_or_else(|| "No dedicated GPU".into());
            let vram = if h.vram_gb > 0.0 { format!(" · {:.0} GB VRAM", h.vram_gb) } else { String::new() };
            format!("{gpu}{vram} · {:.0} GB RAM", h.ram_gb)
        });
        let strip = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.))
            .gap_y(px(4.))
            .text_size(px(size::XS))
            .text_color(Tokens::text_3())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(ph_icon(glyphs::FOLDER, Weight::Bold, Tokens::text_3(), icon_px::XS))
                    .child(match folder {
                        Some(f) => div().max_w(px(380.)).overflow_hidden().whitespace_nowrap().text_ellipsis_middle().font_family(mv_ui::MONO_FONT).text_color(Tokens::text_2()).child(f).into_any_element(),
                        None => skeleton_line(px(200.)).into_any_element(),
                    }),
            )
            .when_some(free, |d, f| d.child(f))
            .child(
                btn("ml-folder", "Change folder", ButtonKind::Ghost)
                    .small()
                    .icon(ph::PENCIL_SIMPLE)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.change_folder(window, cx))),
            )
            .when_some(hw_text, |d, t| d.child(div().flex().items_center().gap(px(6.)).child(ph_icon(ph::CPU, Weight::Bold, Tokens::text_3(), icon_px::XS)).child(t)));

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(32.))
            .pt(px(24.))
            .pb(px(12.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(top)
            .children(categories)
            .child(strip)
    }

    // ── Pages ───────────────────────────────────────────────────────────────

    fn engine_line(&self, job: Job, e: &Engine, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.contains(&e.id);
        let (icon, color, _) = engine_state(e.state);
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(radius::MD))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .child(ph_icon(icon, Weight::Fill, color, icon_px::SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(format!("{}: {}", e.name, e.status_text())))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(e.detail.clone())),
            )
            .children(self.engine_actions(e, ("job", job.id()), busy, cx))
    }

    fn engine_actions(&self, e: &Engine, scope: (&'static str, &'static str), busy: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let id = e.id;
        let key = |what: &str| SharedString::from(format!("{}-{}-{}-{what}", scope.0, scope.1, id_str(id)));
        let mut out: Vec<AnyElement> = vec![];
        if busy {
            out.push(btn(key("starting"), "Starting…", ButtonKind::Secondary).small().loading(true).into_any_element());
            return out;
        }
        match (id, e.state) {
            (EngineId::Ollama | EngineId::LmStudio | EngineId::ComfyUi, EngineState::Stopped) => {
                out.push(btn(key("start"), "Start", ButtonKind::Secondary).small().icon(glyphs::PLAY).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.start_engine(id, window, cx))).into_any_element());
            }
            (EngineId::PythonTts, _) => {}
            (_, EngineState::NotInstalled) => {
                let url = install_url(id);
                out.push(btn(key("get"), format!("Get {}", short_name(id)), ButtonKind::Secondary).small().trailing_icon(ph::ARROW_UP_RIGHT).tooltip(url).on_click(move |_, _, cx| cx.open_url(url)).into_any_element());
            }
            _ => {}
        }
        match id {
            EngineId::Ollama | EngineId::LmStudio => out.push(
                btn(key("own"), "Use my own…", ButtonKind::Ghost)
                    .small()
                    .icon(ph::KEY)
                    .tooltip("Connect a server or key in AI providers")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(LibraryEvent::OpenTab("providers"))))
                    .into_any_element(),
            ),
            // Voices are set up in one place: Settings → Audio.
            EngineId::PythonTts => out.push(btn(key("voices"), "Manage voices", ButtonKind::Secondary).small().icon(ph::WAVEFORM).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_audio(cx))).into_any_element()),
            EngineId::Ffmpeg | EngineId::ComfyUi => {
                if let Some(p) = e.path.clone().filter(|_| e.installed()) {
                    out.push(icon_button(key("reveal"), ph::FOLDER_OPEN, "Show in Explorer").small().on_click(move |_, _, cx| cx.reveal_path(&p)).into_any_element());
                }
            }
        }
        out
    }

    fn model_line(m: &LocalModel) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(thumb(m, 40.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(m.name.clone()))
                            .when(m.vision && m.category == Category::Language, |d| d.child(icon_tag(ph::EYE, "Vision", Tone::Info)))
                            .when_some(m.quant.clone(), |d, q| d.child(tag(q, Tone::Neutral))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(format!("{} · {} · {}", m.family, format_bytes(m.size), m.source.label()))),
            )
    }

    fn job_card(&self, job: Job, cx: &mut Context<Self>) -> impl IntoElement {
        let inv = self.inv.as_ref();
        let list = self.lists.get(&job).map(Vec::as_slice).unwrap_or(&[]);
        let st = inv.map(|i| jobs::status(job, &self.sel, i, list)).unwrap_or(Status::None);
        let model = st.model_id().and_then(|id| inv.and_then(|i| i.model(id))).cloned();

        let picker = div().flex_none().w(px(260.)).when_some(self.pickers.get(&job), |d, p| {
            d.child(
                Select::new(p)
                    .w_full()
                    .menu_width(px(320.))
                    .placeholder(if list.is_empty() { "Nothing on this PC" } else { "Choose a model" })
                    .disabled(list.is_empty()),
            )
        });

        // Icon plus words, never a colour dot alone (WCAG 1.4.1).
        let status_row = {
            let (icon, color, fill, text) = match &st {
                Status::Uses { label, .. } if job.wired() => (ph::CHECK_CIRCLE, Tokens::success(), Tokens::success_soft(), format!("MangaVox uses: {label}")),
                Status::Uses { label, .. } => (ph::CHECK_CIRCLE, Tokens::success(), Tokens::success_soft(), format!("Saved: {label}")),
                Status::Auto { label, .. } => (ph::SPARKLE, Tokens::accent(), Tokens::accent_soft(), format!("MangaVox picks: {label}")),
                Status::Cloud { label } => (ph::CLOUD, Tokens::info(), Tokens::info_soft(), format!("Cloud: {label}")),
                Status::Missing { label } => (ph::WARNING, Tokens::warning(), Tokens::warning_soft(), format!("Not on this PC: {label}")),
                Status::None if list.is_empty() => (ph::CIRCLE_DASHED, Tokens::text_3(), Tokens::active(), "None: the AI won't do this on your PC".to_string()),
                Status::None => (ph::CIRCLE_DASHED, Tokens::text_2(), Tokens::active(), format!("Not chosen yet: {} model{} available", list.len(), if list.len() == 1 { "" } else { "s" })),
            };
            let missing = matches!(st, Status::None | Status::Missing { .. }) && list.is_empty();
            div().flex().items_center().gap(px(10.)).min_w_0().child(status_pill(icon, color, fill, text, !matches!(st, Status::None))).when(missing, |d| d.child(self.get_link(job, cx)))
        };

        // With nothing chosen, show the engine behind the first available model.
        let engine_model = model.clone().or_else(|| list.first().and_then(|c| c.model_id.as_deref()).and_then(|id| inv.and_then(|i| i.model(id))).cloned());
        let engine = job.engine(engine_model.as_ref()).and_then(|id| inv.and_then(|i| i.engine(id))).cloned();
        let show_engine = engine.is_some() && (model.is_some() || !list.is_empty() || job.wired());

        surface()
            .p(px(18.))
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(hero_tile(job.icon(), job.category().hue(), 36.))
                    .child(div().flex_1().min_w_0().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(job.title()))
                    .child(picker),
            )
            .child(status_row)
            .when_some(model, |d, m| d.child(Self::model_line(&m)))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(job.explain()))
            .when_some(engine.filter(|_| show_engine), |d, e| d.child(self.engine_line(job, &e, cx)))
    }

    /// The next step for a job with nothing available.
    fn get_link(&self, job: Job, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("get-{}", job.id()));
        match job {
            Job::Assistant => btn(id, "Get a chat model", ButtonKind::Ghost).small().icon(glyphs::DOWNLOAD).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloads, window, cx))).into_any_element(),
            Job::Vision => btn(id, "How to get one", ButtonKind::Ghost)
                .small()
                .icon(ph::INFO)
                .on_click(|_, _, cx| {
                    toast::show(
                        cx,
                        Toast::info("Install Ollama, then run ollama pull minicpm-v:8b (about 5 GB). Rescan when it finishes.")
                            .duration(Duration::from_secs(10))
                            .action("Copy command", |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(ui_studio::setup::OLLAMA_PULL_CMD.to_string()))),
                    )
                })
                .into_any_element(),
            Job::Voice => btn(id, "Manage voices", ButtonKind::Ghost).small().icon(ph::WAVEFORM).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_audio(cx))).into_any_element(),
            Job::Transcription => btn(id, "Get Whisper", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url("https://huggingface.co/Systran/faster-whisper-small")).into_any_element(),
            Job::Image => btn(id, "Get ComfyUI", ButtonKind::Ghost).small().trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url("https://www.comfy.org/download")).into_any_element(),
        }
    }

    fn render_in_use(&self, cx: &mut Context<Self>) -> Div {
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(page_head(glyphs::MODEL, Hue::Green, "Model Library", "Choose which model MangaVox uses for each job. Everything listed here runs on this PC."));
        if self.inv.is_none() {
            return col.children((0..3).map(|_| skeleton_job()));
        }
        for job in Job::ALL {
            col = col.child(self.job_card(job, cx));
        }
        col
    }

    /// Jobs a detected model can be picked for, with whether it is the current pick.
    fn uses_of(&self, m: &LocalModel) -> Vec<(Job, String, bool)> {
        let Some(inv) = self.inv.as_ref() else { return vec![] };
        let mut out = vec![];
        for job in Job::ALL {
            let list = self.lists.get(&job).map(Vec::as_slice).unwrap_or(&[]);
            // Voices: one model file serves many voices; offer it only once it is chosen.
            let Some(choice) = list.iter().find(|c| c.model_id.as_deref() == Some(m.id.as_str())) else { continue };
            let current = jobs::status(job, &self.sel, inv, list).model_id() == Some(m.id.as_str()) && !matches!(jobs::status(job, &self.sel, inv, list), Status::Auto { .. });
            out.push((job, choice.value.clone(), current));
        }
        out
    }

    fn model_row(&self, idx: usize, m: &LocalModel, first: bool, cx: &mut Context<Self>) -> Div {
        let uses = self.uses_of(m);
        let in_use: Vec<&str> = uses.iter().filter(|u| u.2).map(|u| u.0.title()).collect();
        let offer = uses.iter().find(|u| !u.2 && u.0 != Job::Voice).cloned();
        let path = m.path.clone();
        let used: Option<AnyElement> = if !in_use.is_empty() {
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(size::XS))
                    .text_color(Tokens::success())
                    .child(ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::XS))
                    .child(div().truncate().child(format!("In use: {}", in_use.join(", "))))
                    .into_any_element(),
            )
        } else if uses.is_empty() {
            Some(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(not_used_reason(m)).into_any_element())
        } else {
            None
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .py(px(12.))
            .when(!first, |d| d.border_t_1().border_color(Tokens::border()))
            .child(thumb(m, 36.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .min_w_0()
                            .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).truncate().child(m.name.clone()))
                            .when(m.vision && m.category == Category::Language, |d| d.child(icon_tag(ph::EYE, "Vision", Tone::Info)))
                            .when_some(m.quant.clone(), |d, q| d.child(tag(q, Tone::Neutral))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(format!("{} · {}", m.family, m.source.label())))
                    .when(!m.path.as_os_str().is_empty(), |d| {
                        d.child(div().overflow_hidden().whitespace_nowrap().text_ellipsis_middle().font_family(mv_ui::MONO_FONT).text_size(px(size::XS)).text_color(Tokens::text_3()).child(m.path.display().to_string()))
                    })
                    .children(used),
            )
            .child(div().flex_none().w(px(64.)).text_size(px(size::SM)).text_color(Tokens::text_2()).flex().justify_end().child(format_bytes(m.size)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .when_some(offer, |d, (job, value, _)| {
                        d.child(
                            btn(("ml-use", idx), use_label(job), ButtonKind::Secondary)
                                .small()
                                .icon(ph::CHECK_CIRCLE)
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.use_for(job, value.clone(), window, cx))),
                        )
                    })
                    .when(!path.as_os_str().is_empty(), |d| d.child(icon_button(("ml-reveal", idx), ph::FOLDER_OPEN, "Show in Explorer").small().on_click(move |_, _, cx| cx.reveal_path(&path)))),
            )
    }

    fn model_group(&self, start: usize, title: Option<&str>, models: &[&LocalModel], cx: &mut Context<Self>) -> Div {
        let mut card = surface().px(px(18.)).py(px(4.));
        for (i, m) in models.iter().enumerate() {
            card = card.child(self.model_row(start + i, m, i == 0, cx));
        }
        let size: u64 = models.iter().map(|m| m.size).sum();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when_some(title, |d, t| d.child(mv_ui::section_label(t.to_string()).child(div().text_color(Tokens::text_3()).font_weight(FontWeight::NORMAL).child(format!("· {} · {}", models.len(), format_bytes(size))))))
            .child(card)
    }

    /// What a category's empty state offers as the next step.
    fn empty_action(&self, c: Category, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(match c {
            Category::Language => btn("ml-empty-get", "Get a chat model", ButtonKind::Primary).icon(glyphs::DOWNLOAD).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloads, window, cx))).into_any_element(),
            Category::Voice => btn("ml-empty-voices", "Manage voices", ButtonKind::Primary).icon(ph::WAVEFORM).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_audio(cx))).into_any_element(),
            Category::Transcription => btn("ml-empty-whisper", "Get Whisper", ButtonKind::Secondary).trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url("https://huggingface.co/Systran/faster-whisper-small")).into_any_element(),
            Category::ImageGen => btn("ml-empty-comfy", "Get ComfyUI", ButtonKind::Secondary).trailing_icon(ph::ARROW_UP_RIGHT).on_click(|_, _, cx| cx.open_url("https://www.comfy.org/download")).into_any_element(),
            _ => return None,
        })
    }

    fn render_downloaded(&self, cx: &mut Context<Self>) -> Div {
        let head = |desc: String| page_head(glyphs::STORAGE, Hue::Blue, "On this PC", desc);
        let Some(inv) = self.inv.as_ref() else {
            return div().flex().flex_col().gap(px(16.)).child(head("Looking for models on this PC…".into())).child(skeleton_list());
        };
        let total: u64 = inv.models.iter().map(|m| m.size).sum();
        let mut col = div().flex().flex_col().gap(px(20.)).child(head(format!(
            "{} models, {} in total. Includes Ollama, LM Studio, Hugging Face, ComfyUI and MangaVox's own folder.",
            inv.models.len(),
            format_bytes(total)
        )));
        if inv.models.is_empty() {
            return col.child(
                surface().child(
                    empty_view(glyphs::MODEL, "No models found yet", "Download a chat model, or install Ollama or LM Studio and rescan.")
                        .action(btn("ml-empty-all", "Get a chat model", ButtonKind::Primary).icon(glyphs::DOWNLOAD).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloads, window, cx)))),
                ),
            );
        }
        let mut start = 0;
        for c in Category::ALL {
            let group: Vec<&LocalModel> = inv.models.iter().filter(|m| m.category == c).collect();
            if group.is_empty() {
                continue;
            }
            col = col.child(self.model_group(start, Some(c.label()), &group, cx));
            start += group.len();
        }
        col
    }

    fn render_category(&self, c: Category, cx: &mut Context<Self>) -> Div {
        let mut col = div().flex().flex_col().gap(px(20.)).child(page_head(c.icon(), c.hue(), c.label(), c.description()));
        let Some(inv) = self.inv.as_ref() else { return col.child(skeleton_list()) };
        let group: Vec<&LocalModel> = inv.models.iter().filter(|m| m.category == c).collect();
        if group.is_empty() {
            let mut e = empty_view(c.icon(), format!("No {} on this PC yet", c.label().to_lowercase()), empty_hint(c));
            if let Some(a) = self.empty_action(c, cx) {
                e = e.action(a);
            }
            return col.child(surface().child(e));
        }
        // Language models: split by where they run.
        if c == Category::Language {
            let mut start = 0;
            for src in [Source::Ollama, Source::LmStudio, Source::MangaVox, Source::HuggingFace] {
                let g: Vec<&LocalModel> = group.iter().copied().filter(|m| m.source == src).collect();
                if !g.is_empty() {
                    col = col.child(self.model_group(start, Some(src.label()), &g, cx));
                    start += g.len();
                }
            }
            return col;
        }
        col.child(self.model_group(0, None, &group, cx))
    }

    fn render_engines(&self, cx: &mut Context<Self>) -> Div {
        let mut col = div().flex().flex_col().gap(px(16.)).child(page_head(ph::HARD_DRIVES, Hue::Slate, "Engines", "The programs that run models. MangaVox finds them on this PC and can start the ones that are stopped."));
        let Some(inv) = self.inv.as_ref() else { return col.children((0..3).map(|_| skeleton_job())) };
        for e in &inv.engines {
            let busy = self.busy.contains(&e.id);
            let models = match e.id {
                EngineId::Ollama => Some(inv.models.iter().filter(|m| m.source == Source::Ollama).count()),
                EngineId::LmStudio => Some(inv.models.iter().filter(|m| m.source == Source::LmStudio).count()),
                EngineId::ComfyUi => Some(inv.models.iter().filter(|m| m.source == Source::ComfyUi).count()),
                _ => None,
            };
            let (icon, color, fill) = engine_state(e.state);
            col = col.child(
                surface()
                    .p(px(18.))
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(engine_tile(e.id))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(e.name))
                                    .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(e.detail.clone())),
                            )
                            .child(state_pill(icon, color, fill, capitalize(e.status_text()))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_middle()
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_3())
                                    .child(match (&e.path, models) {
                                        (Some(p), Some(n)) => format!("{} · {n} model{} found", p.display(), if n == 1 { "" } else { "s" }),
                                        (Some(p), None) => p.display().to_string(),
                                        (None, Some(n)) if n > 0 => format!("{n} model{} found from an earlier install", if n == 1 { "" } else { "s" }),
                                        _ => String::new(),
                                    }),
                            )
                            .children(self.engine_actions(e, ("eng", "page"), busy, cx)),
                    ),
            );
        }
        col
    }

    fn render_downloads(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(page_head(glyphs::DOWNLOAD, Hue::Green, "Get models", "Chat models from Hugging Face as GGUF files. Downloads resume if interrupted and land in the models folder."))
            .child(
                surface()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(12.))
                    .child(hero_tile(ph::WAVEFORM, Hue::Cyan, 32.))
                    .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Narration voices (Kokoro, Chatterbox, Piper) are set up in Settings → Audio."))
                    .child(btn("ml-dl-audio", "Manage voices", ButtonKind::Secondary).small().icon(ph::WAVEFORM).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_audio(cx)))),
            )
            .child(div().flex_1().min_h(px(420.)).when_some(self.hub.clone(), |d, h| d.child(h)))
    }
}

impl Render for ModelLibrary {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let downloads = self.page == Page::Downloads;
        let body = match self.page {
            Page::InUse => self.render_in_use(cx),
            Page::Downloaded => self.render_downloaded(cx),
            Page::Downloads => self.render_downloads(cx),
            Page::Engines => self.render_engines(cx),
            Page::Category(c) => self.render_category(c, cx),
        };
        let inner = div().w_full().max_w(px(780.)).when(downloads, |d| d.max_w(px(1200.)).h_full()).px(px(32.)).py(px(28.)).flex().flex_col().gap(px(14.)).child(body);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_subnav(cx))
            .child(div().id("ml-content").flex_1().min_w_0().h_full().when(!downloads, |d| d.overflow_y_scroll()).child(inner))
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Status pill whose text truncates (job status lines carry model names). The glyph is filled
/// when something is chosen and bold when nothing is (Fluent 2: filled marks a chosen state).
fn status_pill(icon: Ph, color: Rgba, fill: Rgba, text: String, chosen: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .h(px(24.))
        .px(px(9.))
        .rounded(px(radius::PILL))
        .bg(fill)
        .text_size(px(size::SM))
        .text_color(color)
        .child(ph_icon(icon, if chosen { Weight::Fill } else { Weight::Bold }, color, icon_px::SM))
        .child(div().min_w_0().truncate().child(text))
}

/// Shaped like a job card while the scan runs.
fn skeleton_job() -> Div {
    surface()
        .p(px(18.))
        .gap(px(14.))
        .child(div().flex().items_center().gap(px(12.)).child(skeleton(px(36.), px(36.), radius::LG)).child(skeleton_line(px(180.))).child(div().flex_1()).child(skeleton(px(260.), px(32.), radius::MD)))
        .child(skeleton(px(220.), px(24.), radius::PILL))
        .child(mv_ui::skeleton_text(2))
}

fn skeleton_list() -> Div {
    surface().px(px(10.)).py(px(6.)).children((0..4).map(|_| skeleton_row()))
}

fn thumb(m: &LocalModel, side: f32) -> AnyElement {
    match &m.thumbnail {
        Some(p) => div()
            .flex_none()
            .size(px(side))
            .rounded(px(side * 0.25))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .child(img(p.clone()).size_full().object_fit(ObjectFit::Cover))
            .into_any_element(),
        None => {
            let letter = m.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            let hue = Hue::ALL[m.source as usize % Hue::ALL.len()];
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(side))
                .rounded(px(side * 0.25))
                .bg(Tokens::hue_soft(hue))
                .text_size(px(side * 0.4))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::hue(hue))
                .child(letter)
                .into_any_element()
        }
    }
}

fn empty_hint(c: Category) -> &'static str {
    match c {
        Category::Language => "Download a GGUF model, or install one with Ollama or LM Studio.",
        Category::Voice => "Download narration voices in Settings → Audio. They show up here once they are on this PC.",
        Category::Transcription => "Whisper models appear here once downloaded (for example by faster-whisper).",
        Category::Ocr => "PaddleOCR or manga-ocr models appear here after Studio installs them.",
        Category::ImageGen => "Install ComfyUI and download a checkpoint to see it here.",
        Category::Audio => "Audio models such as Demucs appear here once downloaded.",
        Category::Other => "Nothing else was found.",
    }
}

fn not_used_reason(m: &LocalModel) -> &'static str {
    match (m.source, m.category) {
        (Source::MangaVox, Category::Language) => "Add it to Ollama (Get models) to chat with it",
        (Source::HuggingFace, Category::Language) => "Transformers format: MangaVox runs GGUF models",
        (_, Category::Audio) | (_, Category::Ocr) => "Used automatically by the tools that downloaded it",
        (Source::ComfyUi, _) => "Add-on used inside ComfyUI workflows",
        _ => "Not used by MangaVox",
    }
}

fn use_label(job: Job) -> &'static str {
    match job {
        Job::Assistant => "Use for chat",
        Job::Vision => "Use for pages",
        Job::Voice => "Use voice",
        Job::Transcription => "Use",
        Job::Image => "Use",
    }
}

/// (icon, colour, soft fill) for an engine state: status in words and shape, not colour alone.
fn engine_state(s: EngineState) -> (Ph, Rgba, Rgba) {
    match s {
        EngineState::Running => (ph::PLAY_CIRCLE, Tokens::success(), Tokens::success_soft()),
        EngineState::Ready => (ph::CHECK_CIRCLE, Tokens::success(), Tokens::success_soft()),
        EngineState::Stopped => (ph::PAUSE_CIRCLE, Tokens::text_2(), Tokens::active()),
        EngineState::NeedsSetup => (ph::WRENCH, Tokens::warning(), Tokens::warning_soft()),
        EngineState::NotInstalled => (ph::DOWNLOAD_SIMPLE, Tokens::text_3(), Tokens::active()),
    }
}

/// Engine tile: the vendor's mark where MangaVox ships one (Ollama, LM Studio), else a glyph.
fn engine_tile(id: EngineId) -> Div {
    let brand = match id {
        EngineId::Ollama => Some("brand/ollama.svg"),
        EngineId::LmStudio => Some("brand/lmstudio.svg"),
        _ => None,
    };
    match brand {
        Some(path) => div().flex().flex_none().items_center().justify_center().size(px(36.)).rounded(px(radius::LG)).bg(Tokens::surface_2()).border_1().border_color(Tokens::border()).child(icon(path, Tokens::text_1(), icon_px::LG)),
        None => {
            let (glyph, hue) = match id {
                EngineId::PythonTts => (ph::WAVEFORM, Hue::Cyan),
                EngineId::Ffmpeg => (glyphs::VIDEO, Hue::Violet),
                _ => (ph::PAINT_BRUSH, Hue::Rose),
            };
            hero_tile(glyph, hue, 36.)
        }
    }
}

fn id_str(id: EngineId) -> &'static str {
    match id {
        EngineId::Ollama => "ollama",
        EngineId::LmStudio => "lmstudio",
        EngineId::PythonTts => "python",
        EngineId::Ffmpeg => "ffmpeg",
        EngineId::ComfyUi => "comfy",
    }
}

fn short_name(id: EngineId) -> &'static str {
    match id {
        EngineId::Ollama => "Ollama",
        EngineId::LmStudio => "LM Studio",
        EngineId::PythonTts => "Python",
        EngineId::Ffmpeg => "FFmpeg",
        EngineId::ComfyUi => "ComfyUI",
    }
}

fn install_url(id: EngineId) -> &'static str {
    match id {
        EngineId::Ollama => "https://ollama.com/download",
        EngineId::LmStudio => "https://lmstudio.ai/download",
        EngineId::PythonTts => "https://www.python.org/downloads/",
        EngineId::Ffmpeg => "https://ffmpeg.org/download.html",
        EngineId::ComfyUi => "https://www.comfy.org/download",
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Launches an engine without a console window. `path` is what the scan found.
fn start_engine(id: EngineId, path: Option<PathBuf>) -> anyhow::Result<()> {
    let (program, args): (PathBuf, &[&str]) = match id {
        EngineId::Ollama => (path.or_else(scan::ollama_binary).ok_or_else(|| anyhow::anyhow!("Ollama is not installed"))?, &["serve"]),
        EngineId::LmStudio => (scan::lms_binary().ok_or_else(|| anyhow::anyhow!("LM Studio's `lms` tool was not found"))?, &["server", "start"]),
        EngineId::ComfyUi => (path.or_else(scan::comfy_app).ok_or_else(|| anyhow::anyhow!("ComfyUI is not installed"))?, &[]),
        _ => anyhow::bail!("This engine starts on its own when needed"),
    };
    let mut cmd = std::process::Command::new(&program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS: keeps running after MangaVox exits.
        if id != EngineId::ComfyUi {
            cmd.creation_flags(0x0800_0000 | 0x0000_0008);
        }
    }
    cmd.spawn().map(|_| ()).map_err(|e| anyhow::anyhow!("{}: {e}", program.display()))
}
