//! Make an anime: pick a downloaded manga and a chapter; the page shows the studio's steps
//! (reading the pages, the cast, scenes and storyboard, the animatic, designs, backgrounds, cuts,
//! the episode) with one button that runs whatever is left and stops at the animatic for the user
//! to watch; then the director's storyboard scene by scene: each cut's manga panel, its shot, what
//! was drawn, the finished cut, and Redo.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::anime::Cut;
use mv_create::{existing, Project};
use mv_ui::theme::{radius, space};
use mv_ui::{btn, icon_ph, menu_button, ph, size, tag, ButtonKind, MenuItem, Tokens, Tone};

use crate::anime_jobs::{self, AStage};
use crate::jobs::Task;
use crate::page::CreativePage;

/// What each step does, for its tooltip. The first four need only the AI that reads and writes;
/// nothing is painted until the animatic has been watched and approved.
const STEPS: [(&str, &str); 8] = [
    ("Reading the pages", "The chapter's real pages are cut into panels, and the AI looks at every panel: who is in it, wearing what, where, and what is said."),
    ("Cast from the pages", "The characters and places are gathered from what the panels show, with their looks and outfits, so the anime keeps the manga's own people."),
    ("Scenes and storyboard", "The AI directs: it divides the chapter into scenes, then writes each scene's cuts from a library of shot recipes (size, angle, lens, composition, camera move, length, how much moves) at the pacing measured on real episodes. An episode director's check then sends back what breaks film grammar."),
    ("Animatic", "Every cut is voiced and shown from its manga panel with its camera move, and the cuts are joined: the episode before anything is drawn. Watch it, then press Draw it."),
    ("Character designs", "An anime design sheet and an expression sheet for everyone the storyboard shows, drawn from their own manga face."),
    ("Backgrounds and effects", "One painted background plate per place, view and light, reused by every cut set there; effects painted once and placed where needed."),
    ("Cuts", "Each cut's characters are drawn on a chroma screen over the plate's light, animated as much as the cut needs, then keyed and composited with the camera move, light and grain."),
    ("Episode", "The finished cuts are joined with their transitions, voices and music."),
];

impl CreativePage {
    /// Picks a chapter of the anime (one not met yet is added, with what Studio read of it).
    fn anime_open_chapter(&mut self, n: u32, cx: &mut Context<Self>) {
        if self.busy() && self.auto.is_some() {
            return;
        }
        self.chapter = n;
        let known = self.p.as_ref().is_some_and(|p| p.chapter(n).is_some());
        if !known {
            let title = self.p.as_ref().and_then(|p| p.adapts.clone());
            self.edit(|p| {
                p.chapter_mut(n);
            });
            if let Some(title) = title {
                let pid = self.project_id();
                cx.spawn(async move |this, cx| {
                    let found = mv_ui::bg::blocking(move || crate::motion::chapter_recap(&title, n)).await.ok().flatten();
                    let _ = this.update(cx, |this, cx| {
                        if let (Some(summary), true) = (found, this.project_id() == pid) {
                            this.edit(|p| {
                                let ch = p.chapter_mut(n);
                                if ch.summary.trim().is_empty() {
                                    ch.summary = summary;
                                }
                            });
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Opens another project (an anime one from the list).
    fn anime_open_project(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy() {
            self.error = Some("Wait for what is being made to finish, or stop it, before switching.".into());
            cx.notify();
            return;
        }
        match Project::load(&id) {
            Ok(p) => {
                self.chapter = p.chapters.first().map(|c| c.number).unwrap_or(1);
                self.p = Some(p);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    /// No anime open: what this makes, the manga to adapt, and the anime made before.
    fn anime_start(&mut self, cx: &mut Context<Self>) -> Div {
        let weak = cx.entity().downgrade();
        let items: Vec<MenuItem> = self
            .library
            .iter()
            .take(200)
            .map(|m| {
                let (w, m2) = (weak.clone(), m.clone());
                MenuItem::new(m.title.clone(), move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.adapt_as(m2.clone(), true, cx));
                })
                .icon(ph::FILM_SLATE)
            })
            .collect();
        let earlier: Vec<(String, String)> = self.projects.iter().filter(|p| p.settings.anime).map(|p| (p.id.clone(), p.title.clone())).collect();
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(space::LG))
            .pt(px(40.))
            .child(icon_ph(ph::FILM_SLATE, Tokens::accent(), 44.))
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Make an anime from a manga"))
            .child(div().max_w(px(640.)).text_center().text_size(px(size::SM)).text_color(Tokens::text_2()).child("Pick a manga you downloaded, then a chapter. The AI reads its real pages panel by panel and directs it like a TV anime: a storyboard with timing, shots and camera moves. Every character gets an anime design from their manga face; backgrounds are painted once per place and reused; each cut's characters are drawn on a green screen, animated, keyed and composited over the background with light, grain and effects; then the cuts are voiced and edited into the episode."))
            .when(!items.is_empty(), |d| d.child(menu_button("cr-adapt-anime", ph::FILM_SLATE, "Adapt a downloaded manga as an anime", items).label("Pick a downloaded manga").min_width(360.)))
            .when(self.library.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::warning()).child("No downloaded manga found yet: download chapters of one first.")))
            .when(!earlier.is_empty(), |d| {
                d.child(mv_ui::section_label("Your anime")).child(div().flex().flex_wrap().justify_center().gap(px(space::SM)).children(earlier.into_iter().enumerate().map(|(i, (id, title))| btn(("cr-anime-open", i), title, ButtonKind::Secondary).small().icon(ph::FILM_SLATE).on_click(cx.listener(move |this, _, _, cx| this.anime_open_project(id.clone(), cx))))))
            })
    }

    pub(crate) fn anime_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(p) = self.p.clone().filter(|p| p.settings.anime) else { return self.anime_start(cx) };
        let n = if p.chapter(self.chapter).is_some() { self.chapter } else { p.first_chapter() };
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(self.anime_chapters(&p, n, cx))
            .child(self.anime_panel(&p, n, cx))
            .child(mv_ui::disclosure("cr-anime-setup", format!("How it is made · {}", self.anime_setup_line(&p))).content(div().flex().flex_col().gap(px(space::MD)).pb(px(space::SM)).child(self.look_bar(cx)).child(self.video_bar(cx)).child(self.music_bar(&p, cx))))
            .children(self.anime_assets(&p, n, cx))
            .children(self.anime_board(&p, n, cx))
    }

    fn anime_setup_line(&self, p: &Project) -> String {
        let vm = mv_create::video::video_model(&p.settings.video_model);
        [
            if p.settings.web_images { "Magnific website".to_string() } else { crate::jobs::image_model(p, &self.keys).name.to_string() },
            if anime_jobs::animates(p) { vm.name.to_string() } else { "Held drawings with camera moves (no video model)".to_string() },
            p.settings.video_aspect.clone(),
            if p.settings.music.is_empty() { "no music".into() } else { "music".into() },
        ]
        .join(" · ")
    }

    fn music_bar(&self, p: &Project, cx: &mut Context<Self>) -> Div {
        let music = p.settings.music.clone();
        div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(btn("cr-music", if music.is_empty() { "Pick music for the episode…" } else { "Change music…" }, ButtonKind::Secondary).small().icon(ph::MUSIC_NOTES).on_click(cx.listener(|_, _, _, cx| {
                let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this music".into()) });
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(files))) = rx.await else { return };
                    let Some(f) = files.into_iter().next() else { return };
                    let _ = this.update(cx, |this, cx| {
                        this.edit(|p| p.settings.music = f.to_string_lossy().to_string());
                        cx.notify();
                    });
                })
                .detach();
            })))
            .when(!music.is_empty(), |d| {
                d.child(btn("cr-music-off", "No music", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| {
                    this.edit(|p| p.settings.music.clear());
                    cx.notify();
                })))
            })
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if music.is_empty() { "The episode plays with its voices only; music goes under them, lowered while anyone speaks.".to_string() } else { music }))
    }

    /// The downloaded chapters as small buttons, the open one marked.
    fn anime_chapters(&self, p: &Project, open: u32, cx: &mut Context<Self>) -> Div {
        let norm = |s: &str| s.trim().to_lowercase();
        let downloaded: Vec<u32> = p.adapts.as_ref().and_then(|t| self.library.iter().find(|m| norm(&m.title) == norm(t))).map(|m| m.chapters.clone()).unwrap_or_default();
        let mut all: Vec<u32> = downloaded.iter().copied().chain(p.chapters.iter().map(|c| c.number)).collect();
        all.sort_unstable();
        all.dedup();
        div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .child(mv_ui::section_label(format!("Chapters · {} downloaded", downloaded.len())))
            .child(div().id("cr-anime-chs").flex().flex_wrap().gap(px(6.)).max_h(px(110.)).overflow_y_scroll().children(all.into_iter().map(|n| {
                let done = p.chapter(n).is_some_and(|c| c.anime.video.is_some());
                let label = format!("Ch {n}{}", if done { " ✓" } else { "" });
                btn(("cr-anime-ch", n as usize), label, if n == open { ButtonKind::Primary } else { ButtonKind::Secondary }).small().on_click(cx.listener(move |this, _, _, cx| this.anime_open_chapter(n, cx)))
            })))
    }

    /// The open chapter's steps, the button that runs what is left, the animatic and the episode.
    fn anime_panel(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Div {
        let stage = anime_jobs::stage(p, n);
        let at = stage.index();
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let a = &ch.anime;
        let busy = self.busy();
        let auto = self.auto.is_some();
        let read = a.panels.iter().filter(|x| x.read).count();
        let story = a.panels.iter().filter(|x| x.story()).count();
        let in_board: Vec<String> = {
            let mut v: Vec<String> = vec![];
            for name in a.cuts.iter().flat_map(|c| c.characters.iter().map(|x| x.name.clone())) {
                if !v.contains(&name) {
                    v.push(name);
                }
            }
            v
        };
        let designed = in_board.iter().filter(|x| p.character(x).is_some_and(|c| c.has_sheets())).count();
        let plates_used: Vec<&str> = a.cuts.iter().map(|c| c.plate.as_str()).collect();
        let plates_done = p.plates.iter().filter(|x| plates_used.contains(&x.id.as_str()) && x.image.is_some()).count();
        let plates_all = p.plates.iter().filter(|x| plates_used.contains(&x.id.as_str())).count();
        let roughs = a.cuts.iter().filter(|c| c.rough.is_some() || c.comp.is_some()).count();
        let comps = a.cuts.iter().filter(|c| c.comp.is_some()).count();
        let secs = a.seconds();
        let mmss = |s: f32| format!("{}:{:02}", (s / 60.) as u32, (s % 60.) as u32);
        let details: [String; 8] = [
            if a.panels.is_empty() { "Not cut yet".into() } else { format!("{read} of {} panels read · {story} story panels", a.panels.len()) },
            if a.cast_done { format!("{} characters · {} places", p.cast.len(), p.locations.len()) } else { "From the pages, once read".into() },
            if a.scenes.is_empty() {
                "Not divided yet".into()
            } else if a.cuts.is_empty() {
                format!("{} scenes · about {}", a.scenes.len(), mmss(a.scenes.iter().map(|s| s.seconds).sum()))
            } else {
                format!("{} scenes · {} cuts · {}{}", a.scenes.len(), a.cuts.len(), mmss(secs), if a.issues.is_empty() { String::new() } else { format!(" · {} notes", a.issues.len()) })
            },
            if a.cuts.is_empty() {
                "After the storyboard".into()
            } else if a.approved {
                "Approved".into()
            } else if a.animatic.is_some() {
                "Ready to watch".into()
            } else {
                format!("{roughs} of {} cuts", a.cuts.len())
            },
            if in_board.is_empty() { "After the animatic".into() } else { format!("{designed} of {} characters", in_board.len()) },
            if a.cuts.is_empty() { "After the animatic".into() } else { format!("{plates_done} of {plates_all} plates · {} effects", p.fx.len()) },
            if a.cuts.is_empty() { "After the animatic".into() } else { format!("{comps} of {} cuts finished", a.cuts.len()) },
            if a.video.is_some() { "Ready to play".into() } else { "After the cuts".into() },
        ];
        let now = self.running.as_ref().map(|r| format!("{} · {}", r.label, r.status.lock().map(|s| s.clone()).unwrap_or_default())).or_else(|| self.thinking.as_ref().map(|s| s.lock().map(|s| if s.is_empty() { "The AI is working".to_string() } else { s.clone() }).unwrap_or_default()));
        let mut steps = div().flex().flex_wrap().gap(px(space::SM));
        for (i, ((title, tip), detail)) in STEPS.iter().zip(details).enumerate() {
            let (done, current) = (at > i, at == i);
            let lead: AnyElement = if done {
                icon_ph(ph::CHECK_CIRCLE, Tokens::success(), 18.).into_any_element()
            } else if current && busy {
                mv_ui::spinner(("cr-astep-spin", i), Tokens::accent(), 16.).into_any_element()
            } else {
                div().flex_none().w(px(18.)).h(px(18.)).rounded_full().border_1().border_color(if current { Tokens::accent() } else { Tokens::border_strong() }).flex().items_center().justify_center().text_size(px(10.)).text_color(if current { Tokens::text_1() } else { Tokens::text_3() }).child(format!("{}", i + 1)).into_any_element()
            };
            steps = steps.child(
                div()
                    .id(("cr-astep", i))
                    .flex_1()
                    .min_w(px(150.))
                    .flex()
                    .flex_col()
                    .gap(px(space::XXS))
                    .px(px(space::MD))
                    .py(px(10.))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(if current { Tokens::accent_border() } else { Tokens::border() })
                    .bg(if current { Tokens::accent_soft() } else { Tokens::surface_3() })
                    .tooltip(mv_ui::tip(*tip))
                    .child(div().flex().items_center().gap(px(6.)).child(lead).child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(if done || current { Tokens::text_1() } else { Tokens::text_3() }).child(*title)))
                    .child(div().text_size(px(size::XS)).text_color(if done || current { Tokens::text_2() } else { Tokens::text_3() }).whitespace_normal().child(detail))
                    .when_some(now.clone().filter(|_| current && busy), |d, t| d.child(div().text_size(px(size::XS)).text_color(Tokens::accent()).whitespace_normal().child(t))),
            );
        }
        let play = |id: &'static str, label: &'static str, kind: ButtonKind, f: std::path::PathBuf, title: String| {
            btn(id, label, kind).icon(ph::PLAY).on_click(move |_, window, cx| {
                let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
            })
        };
        let video = existing(p, a.video.as_ref());
        let animatic = existing(p, a.animatic.as_ref());
        let waiting = stage == AStage::Approve;
        let label = if auto && busy {
            "Making it automatically…"
        } else if at == 0 && a.panels.is_empty() {
            "Make the animatic"
        } else if at < 3 {
            "Continue to the animatic"
        } else {
            "Continue automatically from here"
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .when(stage != AStage::Done && !waiting, |d| {
                d.child(btn("cr-anime-auto", label, ButtonKind::Primary).icon(ph::PLAY).loading(auto && busy).on_click(cx.listener(|this, _, _, cx| {
                    if !this.busy() {
                        this.auto_start(cx);
                    }
                })))
            })
            .when_some(animatic.clone().filter(|_| waiting), |d, f| d.child(play("cr-anime-play-rough", "Watch the animatic", ButtonKind::Primary, f, format!("{} · Chapter {n} · animatic", p.title))))
            .when(waiting && !busy, |d| {
                d.child(btn("cr-anime-approve", "Draw it", ButtonKind::Secondary).icon(ph::PAINT_BRUSH).tooltip("The storyboard is right: paint the designs, backgrounds and cuts. This is the step that spends image and video credits.").on_click(cx.listener(move |this, _, _, cx| {
                    this.edit(|p| p.chapter_mut(n).anime.approved = true);
                    this.auto_start(cx);
                })))
            })
            .when_some(animatic.filter(|_| !waiting), |d, f| d.child(play("cr-anime-play-rough2", "Watch the animatic", ButtonKind::Ghost, f, format!("{} · Chapter {n} · animatic", p.title))))
            .when_some(video, |d, f| {
                let f2 = f.clone();
                d.child(play("cr-anime-play", "Play the episode", if stage == AStage::Done { ButtonKind::Primary } else { ButtonKind::Secondary }, f, format!("{} · Chapter {n}", p.title)))
                    .child(btn("cr-anime-reveal", "Show in folder", ButtonKind::Ghost).icon(ph::FOLDER_OPEN).on_click(move |_, _, _| crate::motion::reveal(&f2)))
            })
            .when(a.read() && a.cast_done && !busy, |d| {
                d.child(btn("cr-anime-rescene", if a.scenes.is_empty() { "Divide into scenes" } else { "Direct it again" }, ButtonKind::Ghost).icon(ph::FILM_STRIP).tooltip("The AI divides the chapter into scenes and storyboards it again from its panels; everything made from the old board is made again").on_click(cx.listener(move |this, _, _, cx| {
                    this.thinking = Some(std::sync::Arc::new(std::sync::Mutex::new(format!("Dividing chapter {n} into scenes"))));
                    this.tick(cx);
                    this.run_ops_pub(vec![mv_create::prompts::Op::ScenesAnime { chapter: n }, mv_create::prompts::Op::BoardAnime { chapter: n, note: String::new() }], cx);
                })))
            })
            .when(comps > 0 && !busy, |d| d.child(btn("cr-anime-edit", "Edit the episode again", ButtonKind::Ghost).icon(ph::FILM_STRIP).on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Episode { chapter: n }], cx)))))
            .when(busy, |d| {
                d.child(btn("cr-anime-stop", "Stop", ButtonKind::Ghost).icon(ph::STOP).on_click(cx.listener(|this, _, _, cx| {
                    this.auto = None;
                    if this.thinking.is_some() {
                        this.stop_ai(cx);
                    }
                    this.stop(cx);
                })))
            })
            .child(div().flex_1().min_w(px(200.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(if auto && busy {
                "Runs by itself; it also shows in Activity."
            } else if waiting {
                "Watch the animatic first: every cut from its manga panel, with its camera move and voice. Nothing is painted until you press Draw it."
            } else {
                "Does every step that is left, one after another, and stops at the animatic for you to watch. Stop any time: what is made is kept."
            }));
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .p(px(space::LG))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Chapter {n}")))
                    .when(!ch.title.trim().is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_2()).child(ch.title.trim().to_string()))),
            )
            .when(!a.treatment.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(format!("Director's treatment: {}", a.treatment))))
            .child(steps)
            .child(actions)
    }

    /// The plates and effects the board uses.
    fn anime_assets(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Option<Div> {
        let a = &p.chapter(n)?.anime;
        if a.cuts.is_empty() || !a.approved {
            return None;
        }
        let used: Vec<&str> = a.cuts.iter().map(|c| c.plate.as_str()).collect();
        let plates: Vec<_> = p.plates.iter().filter(|x| used.contains(&x.id.as_str())).cloned().collect();
        let fx: Vec<_> = p.fx.iter().filter(|f| a.cuts.iter().any(|c| c.fx.iter().any(|u| u.id == f.id))).cloned().collect();
        let tile = |this: &Self, key: String, file: Option<std::path::PathBuf>, name: String, sub: String, redo: Task, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .w(px(176.))
                .child(this.thumb(SharedString::from(format!("cr-at-{key}")), file, 176., 99., cx))
                .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(name))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(2).child(sub))
                .child(btn(SharedString::from(format!("cr-at-redo-{key}")), "Paint again", ButtonKind::Ghost).small().icon(ph::PAINT_BRUSH).on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![redo.clone()], cx))))
        };
        let mut row = div().flex().flex_wrap().gap(px(space::MD));
        for pl in &plates {
            let uses = used.iter().filter(|u| **u == pl.id).count();
            row = row.child(tile(self, format!("pl-{}", pl.id), existing(p, pl.image.as_ref()), pl.location.clone(), format!("{} · {} · used by {uses} cut{}", pl.view, pl.light, if uses == 1 { "" } else { "s" }), Task::Plate(pl.id.clone()), cx));
        }
        for f in &fx {
            row = row.child(tile(self, format!("fx-{}", f.id), existing(p, f.image.as_ref()), f.what.clone(), format!("effect · {}", if f.blend == "over" { "keyed" } else { "screened (light)" }), Task::FxEl(f.id.clone()), cx));
        }
        for x in p.props.iter().filter(|x| used.contains(&x.plate.as_str())) {
            row = row.child(tile(self, format!("prop-{}", x.id), existing(p, x.image.as_ref()), x.what.clone(), "prop · left out of its background, drawn with the characters".into(), Task::PropEl(x.id.clone()), cx));
        }
        Some(div().flex().flex_col().gap(px(space::SM)).child(mv_ui::section_label(format!("Backgrounds and effects · {} plates for {} cuts", plates.len(), a.cuts.len()))).child(row))
    }

    /// The storyboard, scene by scene: each scene's kind, colour key and length against what its
    /// panels call for, the check's notes, and its cuts.
    fn anime_board(&self, p: &Project, n: u32, cx: &mut Context<Self>) -> Option<Div> {
        let a = &p.chapter(n)?.anime;
        if a.scenes.is_empty() {
            return None;
        }
        let busy = self.busy();
        let mut out = div().flex().flex_col().gap(px(space::MD)).child(mv_ui::section_label(format!("Storyboard · {} scenes · {} cuts", a.scenes.len(), a.cuts.len())));
        let general: Vec<&mv_create::anime::Issue> = a.issues.iter().filter(|i| i.scene == 0).collect();
        if !general.is_empty() {
            out = out.child(mv_ui::inline_alert(mv_ui::AlertKind::Warning, "The episode director's check", general.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("\n")));
        }
        for s in &a.scenes {
            let cuts: Vec<&Cut> = a.cuts.iter().filter(|c| c.scene == s.number).collect();
            let boarded: f32 = cuts.iter().map(|c| c.seconds).sum();
            let notes: Vec<&mv_create::anime::Issue> = a.issues.iter().filter(|i| i.scene == s.number).collect();
            let scene = s.number;
            let head = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Scene {} · {}", s.number, s.title)))
                .child(tag(s.kind.clone(), if s.kind == "action" { Tone::Accent } else { Tone::Neutral }))
                .child(tag(s.grade.replace('_', " "), Tone::Neutral))
                .when(!s.location.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(s.location.clone())))
                .child(div().flex_1())
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(if cuts.is_empty() { format!("about {:.0} s", s.seconds) } else { format!("{} cuts · {boarded:.0} s of about {:.0} s", cuts.len(), s.seconds) }))
                .when(!busy && !a.cuts.is_empty(), |d| {
                    d.child(btn(SharedString::from(format!("cr-scene-again-{scene}")), "Storyboard again", ButtonKind::Ghost).small().icon(ph::ARROWS_CLOCKWISE).tooltip("The AI directs this scene again; its cuts are made again").on_click(cx.listener(move |this, _, _, cx| {
                        this.thinking = Some(std::sync::Arc::new(std::sync::Mutex::new(format!("Storyboarding scene {scene} again"))));
                        this.tick(cx);
                        this.run_ops_pub(vec![mv_create::prompts::Op::BoardScene { chapter: n, scene, note: String::new() }], cx);
                    })))
                });
            let mut block = div().flex().flex_col().gap(px(space::SM)).p(px(space::MD)).rounded(px(radius::LG)).bg(Tokens::surface()).border_1().border_color(Tokens::border()).child(head);
            if !s.summary.is_empty() {
                block = block.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).whitespace_normal().child(s.summary.clone()));
            }
            if !notes.is_empty() {
                block = block.child(div().flex().flex_col().gap(px(2.)).children(notes.iter().map(|i| {
                    let at = if i.cut > 0 { format!("Cut {}: ", i.cut) } else { String::new() };
                    div().text_size(px(size::XS)).text_color(if i.severity == "error" { Tokens::danger() } else { Tokens::warning() }).whitespace_normal().child(format!("{at}{}", i.message))
                })));
            }
            block = block.children(cuts.iter().map(|c| self.cut_row(p, n, c, cx)));
            out = out.child(block);
        }
        Some(out)
    }

    fn cut_row(&self, p: &Project, n: u32, c: &Cut, cx: &mut Context<Self>) -> Div {
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let source = c.from.first().and_then(|i| ch.anime.panels.get(*i)).map(|sp| p.file(&sp.file)).filter(|f| f.is_file());
        let drawn = existing(p, c.layer.as_ref()).or_else(|| existing(p, c.key.as_ref())).or_else(|| p.plates.iter().find(|x| x.id == c.plate).and_then(|x| existing(p, x.image.as_ref())));
        let comp = existing(p, c.comp.as_ref());
        let rough = existing(p, c.rough.as_ref());
        let approved = ch.anime.approved;
        let num = c.number;
        let working = self.running.as_ref().is_some_and(|r| r.label.starts_with(&format!("Chapter {n} · cut {num} ")));
        let queued = self.queue.iter().any(|t| matches!(t, Task::Rough { chapter, cut } | Task::Layer { chapter, cut } | Task::Anim { chapter, cut } | Task::Comp { chapter, cut } if *chapter == n && *cut == num));
        let lines: Vec<String> = c.lines.iter().map(|l| format!("{}{}: {}", if l.speaker.is_empty() { "?" } else { &l.speaker }, if matches!(l.kind.as_str(), "thought" | "narration") { format!(" ({})", l.kind) } else { String::new() }, l.text.trim())).collect();
        let title = format!("{} · chapter {n} · cut {num}", p.title);
        let play_file = comp.clone().or(rough.clone());
        div()
            .flex()
            .gap(px(space::MD))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(if working { Tokens::accent() } else { Tokens::border() })
            .child(self.thumb(SharedString::from(format!("cr-cut-src-{num}")), source, 90., 120., cx))
            .when(approved, |d| d.child(self.thumb(SharedString::from(format!("cr-cut-drawn-{num}")), drawn, 192., 108., cx)))
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
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Cut {num} · {:.1} s", c.seconds)))
                            .child(tag(format!("{} · {}", c.size, c.angle), Tone::Neutral))
                            .when(!c.lens.is_empty() && c.lens != "normal", |d| d.child(tag(format!("{} lens", c.lens), Tone::Neutral)))
                            .child(tag(c.camera.replace('_', " "), Tone::Neutral))
                            .child(tag(c.motion.clone(), if c.motion == "full" { Tone::Accent } else { Tone::Neutral }))
                            .when(!c.recipe.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(c.recipe.clone())))
                            .when(c.transition != "cut", |d| d.child(tag(format!("{} in", c.transition), Tone::Neutral)))
                            .when(comp.is_some(), |d| d.child(icon_ph(ph::CHECK_CIRCLE, Tokens::success(), 14.)))
                            .when(working, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::accent()).child(self.running.as_ref().map(|r| r.status.lock().map(|s| s.clone()).unwrap_or_default()).unwrap_or_default()))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(2).child(c.picture.clone()))
                    .when(!c.composition.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(2).child(format!("Frame: {}", c.composition))))
                    .when(!c.action.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(1).child(format!("Moves: {}", c.action))))
                    .when(!lines.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(3).child(lines.join(" / "))))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .when(approved, |d| {
                                d.child(btn(SharedString::from(format!("cr-cut-redo-{num}")), if c.comp.is_some() { "Make again" } else { "Make this cut" }, ButtonKind::Ghost).small().icon(ph::ARROWS_CLOCKWISE).loading(queued || working).tooltip("Draws, animates and composites this cut again").on_click(cx.listener(move |this, _, _, cx| {
                                    this.edit(|p| {
                                        if let Some(c) = p.chapter_mut(n).anime.cuts.iter_mut().find(|c| c.number == num) {
                                            c.layer = None;
                                            c.key = None;
                                            c.anim = None;
                                            c.comp = None;
                                        }
                                    });
                                    let tasks = this.p.as_ref().and_then(|p| p.chapter(n)?.anime.cuts.iter().find(|c| c.number == num).map(|c| anime_jobs::cut_tasks(p, n, c))).unwrap_or_default();
                                    this.enqueue(tasks, cx);
                                })))
                            })
                            .when(c.comp.is_some(), |d| d.child(btn(SharedString::from(format!("cr-cut-recomp-{num}")), "Composite again", ButtonKind::Ghost).small().icon(ph::STACK).on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Comp { chapter: n, cut: num }], cx)))))
                            .when_some(play_file, |d, f| {
                                d.child(btn(SharedString::from(format!("cr-cut-play-{num}")), if comp.is_some() { "Play" } else { "Play the animatic cut" }, ButtonKind::Ghost).small().icon(ph::PLAY).on_click(move |_, window, cx| {
                                    let request = mv_ui::nav::PlayRequest { title: title.clone(), source: f.to_string_lossy().to_string(), file_idx: None, imdb_id: None, mal_id: None, season: None, episode: None };
                                    mv_ui::navigate(mv_ui::Route::Player { request }, window, cx);
                                }))
                            }),
                    ),
            )
    }
}
