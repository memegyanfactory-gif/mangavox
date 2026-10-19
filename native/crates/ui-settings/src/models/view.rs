//! Model Library view: a sub-navigation column (In use, Downloaded, Downloads, Engines,
//! categories, folder + hardware footer) and the selected page.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::IndexPath;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::HubSystemFit;
use mv_ui::{button, icon, size, ButtonKind, Tokens};

use super::jobs::{self, Choice, Job, Selections, Status};
use super::scan::{self, Category, Engine, EngineId, EngineState, Inventory, LocalModel, Source};
use crate::ui::{format_bytes, link, page_title, pill, section_label, surface, tag, tile};

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
            "downloaded" | "installed" => Page::Downloaded,
            "downloads" | "hub" => Page::Downloads,
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
}

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
    notice: Option<(String, bool)>,
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
            notice: None,
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
        self.notice = None;
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
            this.update(cx, |t, cx| {
                t.notice = Some(match r {
                    Ok(()) if job.wired() => (format!("{} now uses {label}.", job.title()), false),
                    Ok(()) => (format!("Saved {label} for {}.", job.title().to_lowercase()), false),
                    Err(e) => (format!("Could not save: {e:#}"), true),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Picks a model from a Downloaded row and updates that job's dropdown.
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
                    t.notice = Some((format!("Could not start: {e:#}"), true));
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
                t.notice = Some(match r {
                    Ok(()) => (format!("Models folder set to {}.", dir.display()), false),
                    Err(e) => (format!("{e:#}"), true),
                });
                t.rescan(window, cx);
            })
            .ok();
        })
        .detach();
    }

    // ── Sub-navigation ──────────────────────────────────────────────────────

    fn nav_item(&self, id: &'static str, label: String, count: Option<usize>, page: Page, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.page == page;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(30.))
            .px(px(10.))
            .rounded(px(8.))
            .border_1()
            .text_size(px(size::SM))
            .cursor_pointer()
            .map(|d| {
                if active {
                    d.bg(Tokens::active()).border_color(rgba(0x2bd49a55)).text_color(Tokens::text_1()).font_weight(FontWeight::MEDIUM)
                } else {
                    d.border_color(transparent_black()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
                }
            })
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when_some(count, |d, n| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(n.to_string())))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.set_page(page, window, cx)))
    }

    fn render_subnav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.inv.as_ref().map(|i| i.models.len());
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(self.nav_item("ml-inuse", "In use".into(), None, Page::InUse, cx))
            .child(self.nav_item("ml-downloaded", match total {
                Some(n) => format!("Downloaded ({n})"),
                None => "Downloaded".into(),
            }, None, Page::Downloaded, cx))
            .child(self.nav_item("ml-downloads", "Downloads".into(), None, Page::Downloads, cx))
            .child(self.nav_item("ml-engines", "Engines".into(), None, Page::Engines, cx))
            .child(div().my(px(10.)).h(px(1.)).bg(Tokens::border()))
            .child(div().px(px(10.)).pb(px(4.)).child(section_label("Categories")));
        for (i, c) in Category::ALL.into_iter().enumerate() {
            let n = self.inv.as_ref().map(|inv| inv.count(c)).unwrap_or(0);
            if c == Category::Other && n == 0 {
                continue;
            }
            let ids = ["ml-c0", "ml-c1", "ml-c2", "ml-c3", "ml-c4", "ml-c5", "ml-c6"];
            list = list.child(self.nav_item(ids[i], c.label().into(), self.inv.as_ref().map(|_| n), Page::Category(c), cx));
        }

        let folder = self.inv.as_ref().map(|i| i.models_dir.display().to_string()).unwrap_or_else(|| "…".into());
        let free = self.inv.as_ref().and_then(|i| i.free_bytes).map(|b| format!("{} free", format_bytes(b)));
        let hw = self.system.as_ref();
        let footer = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(14.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(section_label("Models folder"))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(folder))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_3())
                            .when_some(free, |d, f| d.child(f))
                            .child(link("ml-folder", "Change folder").text_size(px(size::XS)).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.change_folder(window, cx)))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(icon("icons/cpu.svg", Tokens::text_3(), 14.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .min_w_0()
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(hw.and_then(|h| h.gpu_name.clone()).unwrap_or_else(|| if hw.is_some() { "No dedicated GPU".into() } else { "Detecting hardware…".into() })))
                            .when_some(hw, |d, h| {
                                let vram = if h.vram_gb > 0.0 { format!("{:.0} GB VRAM · ", h.vram_gb) } else { String::new() };
                                d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{vram}{:.0} GB RAM", h.ram_gb)))
                            }),
                    ),
            );

        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .py(px(20.))
            .border_r_1()
            .border_color(Tokens::border())
            .child(div().id("ml-subnav").flex_1().min_h_0().overflow_y_scroll().child(list))
            .child(footer)
    }

    // ── Pages ───────────────────────────────────────────────────────────────

    fn engine_line(&self, job: Job, e: &Engine, cx: &mut Context<Self>) -> Div {
        let id = e.id;
        let busy = self.busy.contains(&id);
        let dot = engine_dot(e.state);
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(8.))
            .bg(Tokens::surface_2())
            .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
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
        let small = |b: Stateful<Div>| b.h(px(28.)).px(px(10.));
        if busy {
            out.push(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Starting…").into_any_element());
            return out;
        }
        match (id, e.state) {
            (EngineId::Ollama | EngineId::LmStudio | EngineId::ComfyUi, EngineState::Stopped) => {
                out.push(small(button(key("start"), "Start", ButtonKind::Secondary)).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.start_engine(id, window, cx))).into_any_element());
            }
            (_, EngineState::NotInstalled) => {
                let url = install_url(id);
                out.push(small(button(key("get"), format!("Get {}", short_name(id)), ButtonKind::Secondary)).on_click(move |_, _, cx| cx.open_url(url)).into_any_element());
            }
            _ => {}
        }
        match id {
            EngineId::Ollama | EngineId::LmStudio => out.push(small(button(key("own"), "Use my own…", ButtonKind::Ghost)).on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(LibraryEvent::OpenTab("providers")))).into_any_element()),
            EngineId::PythonTts => out.push(small(button(key("studio"), "Open Studio", ButtonKind::Ghost)).on_click(|_, window, cx| mv_ui::navigate(mv_ui::Route::Studio, window, cx)).into_any_element()),
            EngineId::Ffmpeg | EngineId::ComfyUi => {
                if let Some(p) = e.path.clone().filter(|_| e.installed()) {
                    out.push(small(button(key("reveal"), "Show in folder", ButtonKind::Ghost)).on_click(move |_, _, cx| cx.reveal_path(&p)).into_any_element());
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
                            .when(m.vision && m.category == Category::Language, |d| d.child(tag("Vision", Tokens::text_2())))
                            .when_some(m.quant.clone(), |d, q| d.child(tag(q, Tokens::text_3()))),
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

        let status_row = {
            let (dot, text) = match &st {
                Status::Uses { label, .. } if job.wired() => (Tokens::accent(), format!("MangaVox uses: {label}")),
                Status::Uses { label, .. } => (Tokens::accent(), format!("Saved: {label}")),
                Status::Auto { label, .. } => (Tokens::accent(), format!("MangaVox picks: {label}")),
                Status::Cloud { label } => (rgb(0x60a5fa), format!("Cloud: {label}")),
                Status::Missing { label } => (Tokens::warning(), format!("Not on this PC: {label}")),
                Status::None if list.is_empty() => (Tokens::text_3(), "None: the AI won't do this on your PC".to_string()),
                Status::None => (Tokens::text_3(), format!("Not chosen yet: {} model{} available", list.len(), if list.len() == 1 { "" } else { "s" })),
            };
            let missing = matches!(st, Status::None | Status::Missing { .. }) && list.is_empty();
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(pill(dot, text))
                .when(missing, |d| d.child(self.get_link(job, cx)))
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
                    .child(tile(job.icon(), 32.))
                    .child(div().flex_1().min_w_0().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(job.title()))
                    .child(picker),
            )
            .child(status_row)
            .when_some(model, |d, m| d.child(Self::model_line(&m)))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(job.explain()))
            .when(show_engine, |d| d.child(self.engine_line(job, engine.as_ref().unwrap(), cx)))
    }

    /// "Download one" style link for a job with nothing available.
    fn get_link(&self, job: Job, cx: &mut Context<Self>) -> AnyElement {
        let id = SharedString::from(format!("get-{}", job.id()));
        match job {
            Job::Assistant => link(id, "Download one").on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloads, window, cx))).into_any_element(),
            Job::Vision => link(id, "How to get one")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.notice = Some(("Install Ollama, then run: ollama pull minicpm-v:8b (about 5 GB). Rescan when it finishes.".into(), false));
                    cx.notify();
                }))
                .into_any_element(),
            Job::Voice => link(id, "Set up in Studio").on_click(|_, window, cx| mv_ui::navigate(mv_ui::Route::Studio, window, cx)).into_any_element(),
            Job::Transcription => link(id, "Get Whisper").on_click(|_, _, cx| cx.open_url("https://huggingface.co/Systran/faster-whisper-small")).into_any_element(),
            Job::Image => link(id, "Get ComfyUI").on_click(|_, _, cx| cx.open_url("https://www.comfy.org/download")).into_any_element(),
        }
    }

    fn render_in_use(&self, cx: &mut Context<Self>) -> Div {
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.header("Model Library", "Choose which model MangaVox uses for each job. Everything listed here runs on this PC.", cx));
        if self.inv.is_none() {
            return col.child(loading());
        }
        for job in Job::ALL {
            col = col.child(self.job_card(job, cx));
        }
        col
    }

    fn header(&self, title: &str, desc: &str, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_start()
            .gap(px(16.))
            .child(div().flex_1().min_w_0().child(page_title(title.to_string(), desc.to_string())))
            .child(
                button("ml-rescan", if self.scanning { "Scanning…" } else { "Rescan" }, ButtonKind::Ghost)
                    .h(px(30.))
                    .child(icon("icons/refresh-cw.svg", Tokens::text_2(), 13.))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.rescan(window, cx))),
            )
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
        let used_text = if !in_use.is_empty() {
            Some((format!("In use: {}", in_use.join(", ")), Tokens::accent()))
        } else if uses.is_empty() {
            Some((not_used_reason(m).to_string(), Tokens::text_3()))
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
                            .when(m.vision && m.category == Category::Language, |d| d.child(tag("Vision", Tokens::text_2())))
                            .when_some(m.quant.clone(), |d, q| d.child(tag(q, Tokens::text_3()))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(format!("{} · {}", m.family, m.source.label())))
                    .when(!m.path.as_os_str().is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).truncate().child(m.path.display().to_string())))
                    .when_some(used_text, |d, (t, c)| d.child(div().text_size(px(size::XS)).text_color(c).truncate().child(t))),
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
                            button(("ml-use", idx), use_label(job), ButtonKind::Secondary)
                                .h(px(28.))
                                .px(px(10.))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.use_for(job, value.clone(), window, cx))),
                        )
                    })
                    .when(!path.as_os_str().is_empty(), |d| {
                        d.child(
                            div()
                                .id(("ml-reveal", idx))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(28.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .hover(|s| s.bg(Tokens::hover()))
                                .child(icon("icons/folder-open.svg", Tokens::text_2(), 14.))
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        )
                    }),
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
            .when_some(title, |d, t| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(2.))
                        .child(section_label(t))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} · {}", models.len(), format_bytes(size)))),
                )
            })
            .child(card)
    }

    fn render_downloaded(&self, cx: &mut Context<Self>) -> Div {
        let Some(inv) = self.inv.as_ref() else {
            return div().flex().flex_col().gap(px(16.)).child(self.header("Downloaded", "Models found on this PC.", cx)).child(loading());
        };
        let total: u64 = inv.models.iter().map(|m| m.size).sum();
        let mut col = div().flex().flex_col().gap(px(20.)).child(self.header(
            "Downloaded",
            &format!("{} models found on this PC, {} in total. Includes Ollama, LM Studio, Hugging Face, ComfyUI and MangaVox's own folder.", inv.models.len(), format_bytes(total)),
            cx,
        ));
        let mut start = 0;
        for c in Category::ALL {
            let group: Vec<&LocalModel> = inv.models.iter().filter(|m| m.category == c).collect();
            if group.is_empty() {
                continue;
            }
            col = col.child(self.model_group(start, Some(c.label()), &group, cx));
            start += group.len();
        }
        if inv.models.is_empty() {
            col = col.child(empty("No models found yet", "Download a chat model from Downloads, or install Ollama or LM Studio and rescan."));
        }
        col
    }

    fn render_category(&self, c: Category, cx: &mut Context<Self>) -> Div {
        let mut col = div().flex().flex_col().gap(px(20.)).child(self.header(c.label(), c.description(), cx));
        let Some(inv) = self.inv.as_ref() else { return col.child(loading()) };
        let group: Vec<&LocalModel> = inv.models.iter().filter(|m| m.category == c).collect();
        if group.is_empty() {
            let mut e = empty("Nothing here yet", empty_hint(c));
            if c == Category::Language {
                e = e.child(button("ml-browse", "Browse downloads", ButtonKind::Primary).mt(px(8.)).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.set_page(Page::Downloads, window, cx))));
            }
            return col.child(e);
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
        let mut col = div().flex().flex_col().gap(px(16.)).child(self.header("Engines", "The programs that run models. MangaVox finds them on this PC and can start the ones that are stopped.", cx));
        let Some(inv) = self.inv.as_ref() else { return col.child(loading()) };
        for e in &inv.engines {
            let busy = self.busy.contains(&e.id);
            let models = match e.id {
                EngineId::Ollama => Some(inv.models.iter().filter(|m| m.source == Source::Ollama).count()),
                EngineId::LmStudio => Some(inv.models.iter().filter(|m| m.source == Source::LmStudio).count()),
                EngineId::ComfyUi => Some(inv.models.iter().filter(|m| m.source == Source::ComfyUi).count()),
                _ => None,
            };
            col = col.child(
                surface()
                    .p(px(18.))
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(tile(engine_icon(e.id), 32.))
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
                            .child(pill(engine_dot(e.state), capitalize(e.status_text()))),
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
                                    .text_size(px(size::XS))
                                    .text_color(Tokens::text_3())
                                    .truncate()
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
            .child(self.header("Downloads", "Get chat models from Hugging Face as GGUF files. Downloads resume if interrupted and land in the models folder.", cx))
            .child(
                surface()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(12.))
                    .child(tile("icons/audio-lines.svg", 28.))
                    .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Voices (Kokoro, Piper) are installed by Studio the first time you narrate a chapter."))
                    .child(button("ml-dl-studio", "Open Studio", ButtonKind::Ghost).h(px(28.)).on_click(|_, window, cx| mv_ui::navigate(mv_ui::Route::Studio, window, cx))),
            )
            .child(div().flex_1().min_h(px(420.)).when_some(self.hub.clone(), |d, h| d.child(h)))
    }
}

impl Render for ModelLibrary {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notice = self.notice.clone().map(|(text, error)| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(14.))
                .py(px(9.))
                .rounded(px(10.))
                .bg(Tokens::surface())
                .border_1()
                .border_color(if error { rgba(0xf8717155) } else { rgba(0x2bd49a44) })
                .child(div().flex_1().text_size(px(size::SM)).text_color(if error { Tokens::danger() } else { Tokens::text_2() }).child(text))
                .child(
                    div()
                        .id("ml-notice-x")
                        .cursor_pointer()
                        .child(icon("icons/x.svg", Tokens::text_3(), 13.))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
        });
        let downloads = self.page == Page::Downloads;
        let body = match self.page {
            Page::InUse => self.render_in_use(cx),
            Page::Downloaded => self.render_downloaded(cx),
            Page::Downloads => self.render_downloads(cx),
            Page::Engines => self.render_engines(cx),
            Page::Category(c) => self.render_category(c, cx),
        };
        let inner = div()
            .w_full()
            .max_w(px(860.))
            .when(downloads, |d| d.max_w(px(1200.)).h_full())
            .px(px(32.))
            .py(px(28.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .when_some(notice, |d, n| d.child(n))
            .child(body);
        div()
            .size_full()
            .flex()
            .child(self.render_subnav(cx))
            .child(
                div()
                    .id("ml-content")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(!downloads, |d| d.overflow_y_scroll())
                    .child(inner),
            )
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

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
            let hue = m.source as u8 as f32 * 47.0 + 190.0;
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(side))
                .rounded(px(side * 0.25))
                .bg(hsla((hue % 360.0) / 360.0, 0.28, 0.22, 1.0))
                .border_1()
                .border_color(Tokens::border())
                .text_size(px(side * 0.4))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(Tokens::text_1())
                .child(letter)
                .into_any_element()
        }
    }
}

fn loading() -> Div {
    div().py(px(40.)).flex().justify_center().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Looking for models on this PC…")
}

fn empty(title: &str, body: &str) -> Div {
    surface()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .py(px(40.))
        .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title.to_string()))
        .child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(body.to_string()))
}

fn empty_hint(c: Category) -> &'static str {
    match c {
        Category::Language => "Download a GGUF model, or install one with Ollama or LM Studio.",
        Category::Voice => "Studio downloads Kokoro voices the first time you narrate.",
        Category::Transcription => "Whisper models appear here once downloaded (for example by faster-whisper).",
        Category::Ocr => "PaddleOCR or manga-ocr models appear here after Studio installs them.",
        Category::ImageGen => "Install ComfyUI and download a checkpoint to see it here.",
        Category::Audio => "Audio models such as Demucs appear here once downloaded.",
        Category::Other => "Nothing else was found.",
    }
}

fn not_used_reason(m: &LocalModel) -> &'static str {
    match (m.source, m.category) {
        (Source::MangaVox, Category::Language) => "Add it to Ollama (Downloads) to chat with it",
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

fn engine_dot(s: EngineState) -> Rgba {
    match s {
        EngineState::Running | EngineState::Ready => Tokens::accent(),
        EngineState::Stopped | EngineState::NeedsSetup => Tokens::text_3(),
        EngineState::NotInstalled => Tokens::warning(),
    }
}

fn engine_icon(id: EngineId) -> &'static str {
    match id {
        EngineId::Ollama => "icons/server.svg",
        EngineId::LmStudio => "icons/boxes.svg",
        EngineId::PythonTts => "icons/terminal.svg",
        EngineId::Ffmpeg => "icons/film.svg",
        EngineId::ComfyUi => "icons/wand-sparkles.svg",
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
