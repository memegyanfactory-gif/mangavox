//! Characters: every cast member with their model sheet and expression sheet, the chapter they
//! arrive in, the user's own pictures of them, and the locations. Sheets are what every page is
//! painted from, so this is where a character gets fixed before their first scene.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::{existing, CastMember, Project};
use mv_ui::theme::{radius, space};
use mv_ui::{btn, icon_ph, ph, size, tag, ButtonKind, Tokens, Tone};

use crate::jobs::Task;
use crate::page::CreativePage;

const CARD_W: f32 = 340.;

impl CreativePage {
    pub(crate) fn cast_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.p.clone().unwrap_or_default();
        if p.cast.is_empty() {
            return div().flex().flex_col().gap(px(space::MD)).child(mv_ui::empty_state("No characters yet", "Describe your manga in the Chat tab; the AI designs the whole cast, and you can add pictures of your own characters here.")).child(btn("cr-to-chat", "Go to Chat", ButtonKind::Secondary).small().icon(ph::CHAT_CIRCLE).on_click(cx.listener(|this, _, _, cx| {
                this.tab = crate::page::Tab::Chat;
                cx.notify();
            })));
        }
        let missing: Vec<Task> = p.cast.iter().filter(|c| c.intro_chapter <= self.chapter.max(1)).flat_map(|c| {
            let mut t = vec![];
            if c.sheet.is_none() {
                t.push(Task::Sheet(c.id.clone()));
            }
            if c.expressions.is_none() {
                t.push(Task::Expressions(c.id.clone()));
            }
            t
        }).collect();
        let painted = p.cast.iter().filter(|c| c.has_sheets()).count();
        // Leads and the main cast first: their sheets matter most (and are the most detailed).
        let key: Vec<Task> = {
            let mut cast: Vec<_> = p.cast.iter().filter(|c| c.tier() <= 1).collect();
            cast.sort_by_key(|c| c.tier());
            cast.iter().flat_map(|c| {
                let mut t = vec![];
                if c.sheet.is_none() {
                    t.push(Task::Sheet(c.id.clone()));
                }
                if c.expressions.is_none() {
                    t.push(Task::Expressions(c.id.clone()));
                }
                t
            }).collect()
        };
        let n_key = key.len();
        let tiers = [0u8, 1, 2, 3].map(|t| p.cast.iter().filter(|c| c.tier() == t).count());
        let mut chapters: Vec<u32> = p.cast.iter().map(|c| c.intro_chapter).collect();
        chapters.sort_unstable();
        chapters.dedup();
        let n_missing = missing.len();
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(self.look_bar(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("{} characters ({} lead, {} main cast, {} recurring, {} minor) · {painted} with sheets. {} are made before a character's first scene, so they look the same everywhere.", p.cast.len(), tiers[0], tiers[1], tiers[2], tiers[3], if p.settings.live_action { "Realistic full-body casting sheets" } else { "Sheets" })))
                    .when(n_key > 0, |d| {
                        d.child(btn("cr-paint-key", format!("Make the leads' and main cast's sheets ({n_key})"), ButtonKind::Secondary).small().icon(ph::STAR).tooltip("The one or two leads first, then the main cast").on_click(cx.listener(move |this, _, _, cx| this.enqueue(key.clone(), cx))))
                    })
                    .when(n_missing > 0, |d| {
                        d.child(btn("cr-paint-missing", format!("Paint {n_missing} missing sheets up to chapter {}", self.chapter.max(1)), ButtonKind::Primary).small().icon(ph::PAINT_BRUSH).on_click(cx.listener(move |this, _, _, cx| this.enqueue(missing.clone(), cx))))
                    }),
            )
            .children(chapters.into_iter().map(|ch| {
                let members: Vec<(usize, CastMember)> = p.cast.iter().cloned().enumerate().filter(|(_, c)| c.intro_chapter == ch).collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .child(mv_ui::section_label(format!("Arrives in chapter {ch}")))
                    .child(div().flex().flex_wrap().gap(px(space::MD)).children(members.into_iter().map(|(i, c)| self.cast_card(&p, i, c, cx))))
            }))
            .when(!p.locations.is_empty(), |d| {
                d.child(mv_ui::section_label("Locations")).child(div().flex().flex_wrap().gap(px(space::MD)).children(p.locations.iter().enumerate().map(|(i, l)| {
                    let sheet = existing(&p, l.sheet.as_ref());
                    let id = l.id.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .w(px(260.))
                        .p(px(space::SM))
                        .rounded(px(radius::LG))
                        .bg(Tokens::surface_2())
                        .border_1()
                        .border_color(Tokens::border())
                        .child(self.thumb(("cr-loc", i), sheet, 244., 137., cx))
                        .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(l.name.clone()))
                        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(3).child(l.look.clone()))
                        .child(btn(("cr-loc-paint", i), if l.sheet.is_some() { "Repaint" } else { "Paint" }, ButtonKind::Secondary).small().icon(ph::PAINT_BRUSH).on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Location(id.clone())], cx))))
                })))
            })
    }

    /// An image box: the picture (click for large) or a placeholder.
    pub(crate) fn thumb(&self, id: impl Into<ElementId>, file: Option<std::path::PathBuf>, w: f32, h: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let d = div().id(id).flex_none().w(px(w)).h(px(h)).rounded(px(radius::MD)).overflow_hidden().bg(Tokens::surface_3()).flex().items_center().justify_center();
        match file {
            Some(f) => d.cursor_pointer().child(img(f.clone()).size_full().object_fit(ObjectFit::Contain)).on_click(cx.listener(move |this, _, _, cx| this.view_large(f.clone(), cx))),
            None => d.child(icon_ph(ph::IMAGE, Tokens::text_3(), 22.)),
        }
    }

    fn cast_card(&self, p: &Project, i: usize, c: CastMember, cx: &mut Context<Self>) -> Div {
        let sheet = existing(p, c.sheet.as_ref());
        let expr = existing(p, c.expressions.as_ref());
        let mine: Vec<std::path::PathBuf> = c.user_refs.iter().map(|r| p.file(r)).filter(|f| f.is_file()).collect();
        let id = c.id.clone();
        let id2 = c.id.clone();
        let id3 = c.id.clone();
        let busy = self.queue.iter().any(|t| matches!(t, Task::Sheet(x) | Task::Expressions(x) if *x == c.id)) || self.running.as_ref().is_some_and(|r| r.label.starts_with(&format!("{} ·", c.name)));
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .w(px(CARD_W))
            .p(px(space::SM))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .border_1()
            .border_color(Tokens::border())
            .child(div().flex().gap(px(6.)).child(self.thumb(("cr-sheet", i), sheet, 208., 117., cx)).child(self.thumb(("cr-expr", i), expr, 117., 117., cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(c.name.clone()))
                    .child(tag(c.tier_label(), if c.tier() == 0 { Tone::Accent } else { Tone::Neutral }))
                    .when(!c.role.is_empty(), |d| d.child(tag(c.role.clone(), if c.role.contains("protagonist") { Tone::Accent } else if c.role.contains("antagonist") { Tone::Danger } else { Tone::Neutral })))
                    .when(!c.age.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(c.age.clone()))),
            )
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).line_clamp(3).child(c.appearance.clone()))
            .when_some(c.outfits.first(), |d, o| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(2).child(format!("Wears: {}", o.look))))
            .when(!c.personality.is_empty(), |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(2).child(c.personality.clone())))
            .when(!mine.is_empty(), |d| {
                d.child(div().flex().items_center().gap(px(4.)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("Your pictures:")).children(mine.iter().enumerate().map(|(k, f)| self.thumb(SharedString::from(format!("cr-mine-{i}-{k}")), Some(f.clone()), 36., 36., cx))))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(btn(("cr-c-paint", i), if c.has_sheets() { "Repaint sheets" } else { "Paint sheets" }, ButtonKind::Secondary).small().icon(ph::PAINT_BRUSH).loading(busy).on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Sheet(id.clone()), Task::Expressions(id.clone())], cx))))
                    .when(c.sheet.is_some(), |d| d.child(btn(("cr-c-expr", i), "New expressions", ButtonKind::Ghost).small().on_click(cx.listener(move |this, _, _, cx| this.enqueue(vec![Task::Expressions(id2.clone())], cx)))))
                    .child(btn(("cr-c-mine", i), "Add my picture", ButtonKind::Ghost).small().icon(ph::UPLOAD_SIMPLE).tooltip("Your own drawing or photo of this character; the sheets are painted from it").on_click(cx.listener(move |this, _, _, cx| this.add_pictures(id3.clone(), cx)))),
            )
    }

    fn add_pictures(&mut self, id: String, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Use these pictures".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(files))) = rx.await else { return };
            let Ok(Some(p)) = this.update(cx, |this, _| this.p.clone()) else { return };
            // Decoding, resizing and saving the pictures happens off the UI thread.
            let pid = p.id.clone();
            let key = id.clone();
            let res = mv_ui::bg::blocking(move || mv_create::import_refs(&p, &files, &key)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                let Some(p) = this.p.as_ref().filter(|p| p.id == pid) else { return };
                let name = p.cast.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
                match res {
                    Ok(rels) => {
                        this.edit(|p| {
                            if let Some(c) = p.cast.iter_mut().find(|c| c.id == id) {
                                c.user_refs.extend(rels);
                                // The old sheets were not drawn from these pictures.
                                c.sheet = None;
                                c.expressions = None;
                            }
                        });
                        this.note(format!("Added your pictures of {name}; paint the sheets again to use them."));
                    }
                    Err(e) => this.error = Some(format!("Could not use those pictures: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
