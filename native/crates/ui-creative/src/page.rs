//! The Creative page: a project picker, then four tabs — Chat (the director), Characters (model
//! and expression sheets), Manga (pages, export) and Video (animate and stitch a chapter). The
//! paint queue (`jobs`) runs underneath all of them.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::{CardState, ChatLine, Project};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_ph, menu_button, muted, ph, segmented, size, ButtonKind, MenuItem, Tokens};
use ui_studio::images::{Keys, CATALOG};

use crate::jobs::{self, Task};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Chat,
    Characters,
    Manga,
    Video,
    /// Manga to Blender: assets, previz and chapter renders (`ui-3d`).
    ThreeD,
}

pub(crate) const TABS: [(Tab, &str); 5] = [(Tab::Chat, "Chat"), (Tab::Characters, "Characters"), (Tab::Manga, "Manga"), (Tab::Video, "Video"), (Tab::ThreeD, "3D")];

/// A downloaded manga: its title and chapter folders (style references, inspiration notes).
#[derive(Clone)]
pub(crate) struct LibManga {
    pub id: String,
    pub title: String,
    pub pages_dirs: Vec<PathBuf>,
}

pub(crate) struct Running {
    pub label: String,
    pub status: Arc<Mutex<String>>,
    pub started: Instant,
}

pub(crate) struct CreativePage {
    pub(crate) projects: Vec<Project>,
    pub(crate) p: Option<Project>,
    pub(crate) tab: Tab,
    pub(crate) library: Vec<LibManga>,
    pub(crate) composer: Option<Entity<TextareaState>>,
    pub(crate) picker: Option<AnyView>,
    /// The text AI is working ("Writing the series…").
    pub(crate) thinking: Option<Arc<Mutex<String>>>,
    pub(crate) queue: VecDeque<Task>,
    pub(crate) running: Option<Running>,
    /// Tasks finished in the current run, for the progress bar.
    pub(crate) run_done: usize,
    pub(crate) error: Option<String>,
    pub(crate) keys: Keys,
    pub(crate) chapter: u32,
    /// A page or sheet shown large.
    pub(crate) viewing: Option<PathBuf>,
    pub(crate) confirm_delete: bool,
    pub(crate) ffmpeg: Option<bool>,
    pub(crate) chat_scroll: ScrollHandle,
    pub(crate) scroll: ScrollHandle,
    /// The 3D pipeline's view, made the first time its tab opens.
    pub(crate) three_d: Option<AnyView>,
    /// A repaint loop is running (`tick`).
    pub(crate) ticking: bool,
    /// The run as Activity shows it (how far along, what it is doing) and its Cancel.
    pub(crate) activity: Option<(Arc<Mutex<(f32, String)>>, Arc<AtomicBool>)>,
    /// The clip picked on the Video tab (page, panel) and the box its notes are typed in.
    pub(crate) clip_sel: Option<(u32, usize)>,
    pub(crate) clip_notes: Option<Entity<TextareaState>>,
    /// Cancels the director AI's request in flight.
    pub(crate) ai_cancel: mv_ai::CancellationToken,
    pub(crate) _subs: Vec<Subscription>,
}

impl CreativePage {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<mv_ui::settings::ServiceKeysChanged>() {
            cx.set_global(mv_ui::settings::ServiceKeysChanged(0));
        }
        let subs = vec![cx.observe_global::<mv_ui::settings::ServiceKeysChanged>(|this: &mut Self, cx| {
            this.keys = Keys::load();
            cx.notify();
        })];
        let projects = Project::list();
        let p = projects.first().cloned();
        let chapter = p.as_ref().and_then(|p| p.chapters.first().map(|c| c.number)).unwrap_or(1);
        cx.spawn(async move |this, cx| {
            let lib = mv_ui::bg::blocking(|| ui_studio::data::load().unwrap_or_default()).await.unwrap_or_default();
            let ff = mv_ui::bg::blocking(mv_create::video::has_ffmpeg).await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.library = lib.into_iter().map(|g| LibManga { id: g.id, title: g.title, pages_dirs: g.chapters.into_iter().map(|c| c.pages_dir).collect() }).collect();
                this.ffmpeg = Some(ff);
                cx.notify();
            });
        })
        .detach();
        let mut page = CreativePage {
            projects,
            p,
            tab: Tab::Chat,
            library: vec![],
            composer: None,
            picker: None,
            thinking: None,
            queue: VecDeque::new(),
            running: None,
            run_done: 0,
            error: None,
            keys: Keys::load(),
            chapter,
            viewing: None,
            confirm_delete: false,
            ffmpeg: None,
            chat_scroll: ScrollHandle::new(),
            scroll: ScrollHandle::new(),
            three_d: None,
            ticking: false,
            activity: None,
            clip_sel: None,
            clip_notes: None,
            ai_cancel: mv_ai::CancellationToken::new(),
            _subs: subs,
        };
        page.ensure_inputs(window, cx);
        page
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.is_some() {
            return;
        }
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 10).submit_on_enter(true).placeholder("Describe your manga: \"Like Dr. Stone, but the hero is my character Arhan, and science is replaced by alchemy…\""));
        self._subs.push(cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = ev {
                this.send_from_composer(window, cx);
            }
        }));
        self.composer = Some(composer);
        self.clip_notes = Some(cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 4).placeholder("Notes for this clip: \"she should turn to the camera and smile\", \"slower, hold on his face\", \"it is raining\"")));
        self.picker = Some(ui_assistant::model_picker_above(window, cx));
    }

    /// Painting, or the AI is writing: the open project must not change under it.
    pub(crate) fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty() || self.thinking.is_some()
    }

    pub(crate) fn project_id(&self) -> Option<String> {
        self.p.as_ref().map(|p| p.id.clone())
    }

    /// Saves the open project (and refreshes its row in the picker).
    pub(crate) fn save(&mut self) {
        let Some(p) = self.p.as_mut() else { return };
        if let Err(e) = p.save() {
            self.error = Some(format!("Could not save the project: {e:#}"));
            return;
        }
        let p = p.clone();
        match self.projects.iter_mut().find(|x| x.id == p.id) {
            Some(row) => *row = p,
            None => self.projects.insert(0, p),
        }
    }

    /// Changes the open project and saves it.
    pub(crate) fn edit(&mut self, f: impl FnOnce(&mut Project)) {
        if let Some(p) = self.p.as_mut() {
            f(p);
            self.save();
        }
    }

    pub(crate) fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.edit(|p| p.chat.push(ChatLine::new("note", text)));
    }

    pub(crate) fn new_project(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            self.error = Some("Wait for the AI or the painting to finish, or stop it, before starting another manga.".into());
            cx.notify();
            return;
        }
        let mut p = Project::new("Untitled manga");
        p.settings.image_model = jobs::image_model(&p, &self.keys).id.to_string();
        if let Err(e) = p.save() {
            self.error = Some(format!("Could not create the project: {e:#}"));
            cx.notify();
            return;
        }
        self.projects.insert(0, p.clone());
        self.p = Some(p);
        self.chapter = 1;
        self.tab = Tab::Chat;
        self.error = None;
        cx.notify();
    }

    fn open_project(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.busy() {
            self.error = Some("Wait for the AI or the painting to finish, or stop it, before switching manga.".into());
            cx.notify();
            return;
        }
        match Project::load(id) {
            Ok(p) => {
                self.chapter = p.chapters.first().map(|c| c.number).unwrap_or(1);
                self.p = Some(p);
                self.error = None;
                self.confirm_delete = false;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    fn delete_project(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.p.as_ref().map(|p| p.id.clone()) else { return };
        if self.busy() {
            self.confirm_delete = false;
            self.error = Some("Wait for the AI or the painting to finish, or stop it, before deleting this manga.".into());
            cx.notify();
            return;
        }
        if let Err(e) = Project::delete(&id) {
            self.error = Some(format!("Could not delete it: {e:#}"));
        }
        self.projects.retain(|p| p.id != id);
        self.p = self.projects.first().cloned();
        self.confirm_delete = false;
        cx.notify();
    }

    // ------------------------------------------------------------------ queue

    pub(crate) fn enqueue(&mut self, tasks: Vec<Task>, cx: &mut Context<Self>) {
        if tasks.is_empty() {
            return;
        }
        if !self.busy() {
            self.run_done = 0;
        }
        // The run shows in Activity from every page, with Cancel, until the queue is empty.
        if self.activity.is_none() {
            if let Some(p) = self.p.as_ref() {
                let state = Arc::new(Mutex::new((0., "Starting".to_string())));
                let cancel = Arc::new(AtomicBool::new(false));
                ui_studio::pipeline::track_activity(&p.id, &format!("Creative · {}", if p.title.is_empty() { "Untitled manga" } else { &p.title }), &state, &cancel);
                self.activity = Some((state, cancel));
            }
        }
        for t in tasks {
            if !self.queue.contains(&t) {
                self.queue.push_back(t);
            }
        }
        self.error = None;
        self.pump(cx);
        cx.notify();
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        let left = self.queue.len();
        self.queue.clear();
        if left > 0 {
            self.note(format!("Stopped. {left} job{} left unpainted; the one running now finishes.", if left == 1 { "" } else { "s" }));
        }
        cx.notify();
    }

    /// Tells Activity where the run is, and stops it when Cancel was pressed there.
    fn report(&mut self, cx: &mut Context<Self>) {
        let Some((state, cancel)) = self.activity.clone() else { return };
        if cancel.swap(false, Ordering::Relaxed) && !self.queue.is_empty() {
            self.stop(cx);
        }
        let Some(r) = self.running.as_ref() else { return };
        let total = self.run_done + self.queue.len() + 1;
        let status = r.status.lock().map(|s| s.clone()).unwrap_or_default();
        if let Ok(mut s) = state.lock() {
            *s = (self.run_done as f32 / total as f32, format!("{} · {status} · {} of {total}", r.label, self.run_done + 1));
        };
    }

    /// Starts the next task when nothing is running.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(task) = self.queue.pop_front() else {
            // Nothing left: the Activity card goes.
            self.activity = None;
            return;
        };
        let Some(p) = self.p.clone() else {
            self.queue.clear();
            return;
        };
        let status = Arc::new(Mutex::new("Starting".to_string()));
        let job = match jobs::build(&p, &task, &self.keys, status.clone()) {
            Ok(j) => j,
            Err(e) => {
                // A missing key stops everything; anything else skips just this task.
                let msg = format!("{e:#}");
                if msg.contains("key") || msg.contains("workflow") || msg.contains("ComfyUI") || msg.contains("Magnific's website") {
                    self.queue.clear();
                    self.error = Some(msg);
                } else {
                    self.note(format!("Skipped: {msg}"));
                }
                cx.notify();
                return self.pump(cx);
            }
        };
        let (label, credits, model) = (job.label.clone(), job.credits, job.model);
        let pid = p.id.clone();
        self.running = Some(Running { label: label.clone(), status, started: Instant::now() });
        self.report(cx);
        let fut = job.fut;
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(fut).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.running = None;
                match res {
                    Ok(o) => {
                        if let Some(m) = model {
                            ui_studio::images::record(m, credits);
                        }
                        let stitched = matches!(o, jobs::Outcome::Stitch { .. });
                        // Switching is refused while painting; should the project change anyway,
                        // the result still goes to the project it was painted for.
                        if this.project_id().as_deref() == Some(pid.as_str()) {
                            this.edit(|p| jobs::apply(p, o));
                        } else if let Ok(mut other) = Project::load(&pid) {
                            jobs::apply(&mut other, o);
                            let _ = other.save();
                        }
                        this.run_done += 1;
                        if stitched {
                            this.note(format!("{label} is ready. Open the Video tab to play it."));
                            toast::show(cx, Toast::success(format!("{label} is ready")).icon(ph::FILM_STRIP));
                        }
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        let fatal = ["API key", "out of credits", "rate limiting", "refused the request", "Connect Magnific", "FFmpeg", "ComfyUI", "Studio's Python", "Magnific's website"].iter().any(|k| msg.contains(k));
                        this.note(format!("{label} failed: {msg}"));
                        if fatal {
                            this.queue.clear();
                            this.error = Some(msg);
                        }
                    }
                }
                if this.queue.is_empty() && this.run_done > 0 {
                    toast::show(cx, Toast::success("Painting finished").icon(ph::CHECK));
                }
                this.pump(cx);
                cx.notify();
            });
        })
        .detach();
        self.tick(cx);
    }

    /// Repaints while something runs, for the status and the timer. One loop at a time.
    pub(crate) fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
            let alive = this.update(cx, |this, cx| {
                this.report(cx);
                cx.notify();
                let alive = this.running.is_some() || this.thinking.is_some() || !this.queue.is_empty();
                if !alive {
                    this.ticking = false;
                }
                alive
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Stops the director AI (a stalled answer would otherwise keep the manga locked).
    pub(crate) fn stop_ai(&mut self, cx: &mut Context<Self>) {
        self.ai_cancel.cancel();
        self.ai_cancel = mv_ai::CancellationToken::new();
        self.thinking = None;
        self.note("Stopped the AI.");
        cx.notify();
    }

    pub(crate) fn start_card(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(p) = self.p.as_ref() else { return };
        let Some(line) = p.chat.get(i) else { return };
        let Some(proposal) = line.proposal.clone() else { return };
        let tasks = jobs::plan(p, &proposal);
        if tasks.is_empty() {
            self.note("Nothing left to paint there.");
        }
        self.edit(|p| p.chat[i].state = CardState::Started);
        self.enqueue(tasks, cx);
    }

    pub(crate) fn dismiss_card(&mut self, i: usize, cx: &mut Context<Self>) {
        self.edit(|p| {
            if let Some(l) = p.chat.get_mut(i) {
                l.state = CardState::Dismissed;
            }
        });
        cx.notify();
    }

    // ------------------------------------------------------------------ shared controls

    /// The image model menu: models with a key first, the rest say which key they need.
    pub(crate) fn model_menu(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let Some(p) = self.p.as_ref() else { return div().into_any_element() };
        let current = jobs::image_model(p, &self.keys);
        let weak = cx.entity().downgrade();
        let mut items = vec![MenuItem::heading("Ready to use")];
        let mut rest = vec![MenuItem::separator(), MenuItem::heading("Needs a key")];
        for m in CATALOG {
            let w = weak.clone();
            let has = self.keys.has(m);
            let label = format!("{}{}{}", m.name, if jobs::paints_pages(m) { " · whole pages" } else { "" }, if m.refs == 0 { " · no character refs" } else { "" });
            let item = MenuItem::new(label, move |_, cx| {
                let _ = w.update(cx, |this, cx| {
                    this.edit(|p| p.settings.image_model = m.id.to_string());
                    cx.notify();
                });
            })
            .icon(m.icon())
            .checked(m.id == current.id);
            if has {
                items.push(item);
            } else {
                rest.push(item);
            }
        }
        if rest.len() > 2 {
            items.extend(rest);
        }
        menu_button(id, current.icon(), "Image model", items).label(current.name).small().min_width(320.).into_any_element()
    }

    pub(crate) fn resolution_menu(&self, id: &'static str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.p.as_ref()?;
        let m = jobs::image_model(p, &self.keys);
        if m.resolutions.is_empty() {
            return None;
        }
        let current = jobs::resolution(p, m);
        let weak = cx.entity().downgrade();
        let items = m
            .resolutions
            .iter()
            .map(|r| {
                let w = weak.clone();
                let r = *r;
                MenuItem::new(format!("{r}{}", m.cost_label(r)), move |_, cx| {
                    let _ = w.update(cx, |this, cx| {
                        this.edit(|p| p.settings.resolution = r.to_string());
                        cx.notify();
                    });
                })
                .checked(r == current)
            })
            .collect();
        Some(menu_button(id, ph::SQUARES_FOUR, "Resolution", items).label(current).small().into_any_element())
    }

    /// Model, resolution, whole-page / panel mode, colour and reading order.
    pub(crate) fn look_bar(&self, cx: &mut Context<Self>) -> Div {
        let Some(p) = self.p.as_ref() else { return div() };
        let m = jobs::image_model(p, &self.keys);
        let mode_ix = mv_create::RenderMode::ALL.iter().position(|x| *x == p.settings.mode).unwrap_or(0);
        let web = p.settings.web_images;
        let note = if web {
            "Pictures are made on Magnific's website in the app's browser window, signed in as you: Google Nano Banana 2.1 at 1K with unlimited mode on, one at a time. Nothing is generated unless the page shows it as Unlimited."
        } else if p.settings.live_action {
            "Live action: characters become real-looking people (full-body casting sheets) and every scene is a film still, then a clip."
        } else if jobs::whole_pages(p, m) {
            "Paints each page whole: panels, balloons and lettering."
        } else if matches!(p.settings.mode, mv_create::RenderMode::Page) {
            "This model cannot paint a portrait page, so it paints panel by panel."
        } else {
            "Paints each panel at the model's nearest size, then lays out and letters the page here."
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(segmented("cr-via", ["API key", "Magnific website"], usize::from(web)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.edit(|p| p.settings.web_images = *ix == 1);
                cx.notify();
            })))
            .when(web, |d| {
                d.child(btn("cr-web-open", "Open Magnific window", ButtonKind::Secondary).small().icon(ph::GLOBE).tooltip("Sign in to Magnific here once; the app keeps the sign-in. If Magnific asks you to confirm you are a person, do it in this window.").on_click(cx.listener(|this, _, _, cx| {
                    cx.spawn(async move |this, cx| {
                        if let Err(e) = mv_ui::bg::run(crate::web::open_window()).await.and_then(|r| r) {
                            let _ = this.update(cx, |this, cx| {
                                this.error = Some(format!("{e:#}"));
                                cx.notify();
                            });
                        }
                    })
                    .detach();
                    let _ = this;
                })))
            })
            .when(!web, |d| d.child(self.model_menu("cr-model", cx)).children(self.resolution_menu("cr-res", cx)))
            .child(segmented("cr-mode", mv_create::RenderMode::ALL.map(|m| m.label()), mode_ix).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.edit(|p| p.settings.mode = mv_create::RenderMode::ALL[*ix]);
                cx.notify();
            })))
            .child(segmented("cr-live", ["Drawn manga", "Live action"], usize::from(p.settings.live_action)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.edit(|p| p.settings.live_action = *ix == 1);
                cx.notify();
            })))
            .child(segmented("cr-color", ["B&W", "Colour"], usize::from(p.settings.color)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.edit(|p| p.settings.color = *ix == 1);
                cx.notify();
            })))
            .child(segmented("cr-rtl", ["Right to left", "Left to right"], usize::from(!p.settings.rtl)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.edit(|p| p.settings.rtl = *ix == 0);
                cx.notify();
            })))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(note))
            .when(!web && m.refs == 0, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child(format!("{} takes no reference images, so characters can drift. Nano Banana 2.1 keeps them on model.", m.name))))
            .when(!web && !self.keys.has(m), |d| {
                d.child(btn("cr-keys", if m.via == ui_studio::images::Via::Magnific { "Add Magnific key" } else { "Add key" }, ButtonKind::Secondary).small().icon(ph::KEY).on_click(move |_, window, cx| {
                    let tab = if m.via == ui_studio::images::Via::Magnific { "magnific" } else { "services" };
                    mv_ui::navigate(mv_ui::Route::Settings { tab: Some(tab.into()) }, window, cx);
                }))
            })
    }

    /// What is painting now and what is queued.
    pub(crate) fn queue_strip(&self, cx: &mut Context<Self>) -> Option<Div> {
        let r = self.running.as_ref()?;
        let status = r.status.lock().map(|s| s.clone()).unwrap_or_default();
        let total = self.run_done + self.queue.len() + 1;
        let secs = r.started.elapsed().as_secs();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(space::MD))
                .rounded(px(radius::LG))
                .bg(Tokens::surface_2())
                .border_1()
                .border_color(Tokens::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(mv_ui::spinner("cr-spin", Tokens::accent(), 14.))
                        .child(div().flex_1().min_w_0().text_size(px(size::SM)).text_color(Tokens::text_1()).truncate().child(format!("{} · {status} · {}:{:02}", r.label, secs / 60, secs % 60)))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} of {total}", self.run_done + 1)))
                        .child(btn("cr-stop", "Stop", ButtonKind::Ghost).small().icon(ph::STOP).on_click(cx.listener(|this, _, _, cx| this.stop(cx)))),
                )
                .child(mv_ui::progress_bar("cr-progress", self.run_done as f32 / total as f32, true)),
        )
    }

    pub(crate) fn view_large(&mut self, f: PathBuf, cx: &mut Context<Self>) {
        self.viewing = Some(f);
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let weak = cx.entity().downgrade();
        let mut items: Vec<MenuItem> = vec![];
        {
            let w = weak.clone();
            items.push(MenuItem::new("New manga", move |_, cx| {
                let _ = w.update(cx, |this, cx| this.new_project(cx));
            }).icon(ph::PLUS));
        }
        if !self.projects.is_empty() {
            items.push(MenuItem::separator());
            items.push(MenuItem::heading("Your manga"));
        }
        let open = self.p.as_ref().map(|p| p.id.clone());
        for p in &self.projects {
            let w = weak.clone();
            let id = p.id.clone();
            items.push(MenuItem::new(if p.title.is_empty() { "Untitled manga".to_string() } else { p.title.clone() }, move |_, cx| {
                let _ = w.update(cx, |this, cx| this.open_project(&id, cx));
            }).checked(open.as_deref() == Some(p.id.as_str())));
        }
        if self.p.is_some() {
            let w = weak.clone();
            items.push(MenuItem::separator());
            items.push(MenuItem::new("Delete this manga…", move |_, cx| {
                let _ = w.update(cx, |this, cx| {
                    this.confirm_delete = true;
                    cx.notify();
                });
            }).icon(ph::TRASH).danger());
        }
        let title = self.p.as_ref().map(|p| if p.title.is_empty() { "Untitled manga".to_string() } else { p.title.clone() }).unwrap_or_else(|| "Creative".into());
        let tab_ix = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::MD))
                    .child(icon_ph(ph::PAINT_BRUSH, Tokens::accent(), icon_px::LG))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(title))
                            .child(muted(self.p.as_ref().map(summary).unwrap_or_else(|| "Write and paint your own manga with AI.".into()))),
                    )
                    .child(menu_button("cr-projects", ph::BOOKS, "Your manga", items).label("Manga").small().min_width(260.)),
            )
            .child(segmented("cr-tabs", TABS.map(|(_, l)| l), tab_ix).on_change(cx.listener(|this, ix: &usize, _, cx| {
                this.tab = TABS[*ix].0;
                cx.notify();
            })))
            .when(self.confirm_delete, |d| {
                d.child(
                    mv_ui::inline_alert(mv_ui::AlertKind::Warning, "Delete this manga?", "Its script, sheets, pages and videos are removed from this PC. Exported PDFs and images stay.")
                        .action("Delete", cx.listener(|this, _, _, cx| this.delete_project(cx)))
                        .dismissible(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = false;
                            cx.notify();
                        })),
                )
            })
            .when_some(self.error.clone(), |d, e| {
                d.child(mv_ui::inline_alert(mv_ui::AlertKind::Error, "Something went wrong", e).dismissible(cx.listener(|this, _, _, cx| {
                    this.error = None;
                    cx.notify();
                })))
            })
    }
}

/// "12 characters · 3 chapters · 40 pages painted".
fn summary(p: &Project) -> String {
    if p.cast.is_empty() {
        return "Tell the AI what you want to make.".into();
    }
    let painted: usize = p.chapters.iter().map(|c| c.painted()).sum();
    format!("{} characters · {} chapters · {painted} pages painted", p.cast.len(), p.chapters.len())
}

impl Render for CreativePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);
        let header = self.header(cx);
        if self.tab == Tab::ThreeD && self.three_d.is_none() {
            self.three_d = Some(ui_3d::view(window, cx));
        }
        let body: AnyElement = match (&self.p, self.tab) {
            // The 3D pipeline works from the downloaded manga, so it needs no Creative project.
            (_, Tab::ThreeD) => self.three_d.clone().map(|v| v.into_any_element()).unwrap_or_else(|| div().into_any_element()),
            (None, _) => self.welcome(cx).into_any_element(),
            (Some(_), Tab::Chat) => self.chat_tab(cx).into_any_element(),
            (Some(_), Tab::Characters) => self.cast_tab(cx).into_any_element(),
            (Some(_), Tab::Manga) => self.manga_tab(cx).into_any_element(),
            (Some(_), Tab::Video) => self.video_tab(cx).into_any_element(),
        };
        // The chat keeps its composer pinned and 3D scrolls itself; the other tabs scroll as one page.
        let chat = (self.p.is_some() && self.tab == Tab::Chat) || self.tab == Tab::ThreeD;
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("cr-scroll")
                    .size_full()
                    .when(!chat, |d| d.overflow_y_scroll().track_scroll(&self.scroll))
                    .child(div().size_full().flex().flex_col().gap(px(space::LG)).px(px(size::PAGE_X)).py(px(size::PAGE_Y)).child(header).children(self.queue_strip(cx)).child(div().flex_1().min_h_0().flex().flex_col().child(body))),
            )
            .when_some(self.viewing.clone(), |d, f| {
                d.child(
                    div()
                        .id("cr-viewer")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(32.))
                        .bg(gpui_kit::rgba(0x000000d9))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.viewing = None;
                            cx.notify();
                        }))
                        .child(img(f).size_full().object_fit(ObjectFit::Contain)),
                )
            })
    }
}

impl CreativePage {
    fn welcome(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(space::LG))
            .pt(px(60.))
            .child(icon_ph(ph::PAINT_BRUSH, Tokens::accent(), 48.))
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Make your own manga"))
            .child(div().max_w(px(560.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Chat with the AI about the story you want. It writes the series, designs every character with a model sheet and expression sheet, paints the pages with your image model, exports them as PDF or images, and animates them into a video."))
            .child(btn("cr-new", "Start a new manga", ButtonKind::Primary).icon(ph::PLUS).on_click(cx.listener(|this, _, _, cx| this.new_project(cx))))
            .children(self.adapt_menu("cr-adapt-welcome", cx))
    }
}
