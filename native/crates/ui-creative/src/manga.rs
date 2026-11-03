//! Manga: a chapter's pages as painted (or as scripted, until they are), paint / repaint, and
//! export to PDF or page images wherever the user picks.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_create::{existing, Page, Project, Proposal};
use mv_ui::theme::{radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, menu_button, ph, size, ButtonKind, MenuItem, Tokens};

use crate::jobs::{self, Task};
use crate::page::CreativePage;

const PAGE_W: f32 = 200.;

impl CreativePage {
    /// Chapter tabs (shared by Manga and Video).
    pub(crate) fn chapter_picker(&self, p: &Project, cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().gap(px(6.)).children(p.chapters.iter().map(|c| {
            let n = c.number;
            let label = format!("{n}{}", if c.pages.is_empty() { "" } else if c.painted() == c.pages.len() { " ✓" } else { "" });
            mv_ui::filter_chip(("cr-ch", n as usize), format!("Ch. {label}"), self.chapter == n).on_click(cx.listener(move |this, _, _, cx| {
                this.chapter = n;
                cx.notify();
            }))
        }))
    }

    pub(crate) fn manga_tab(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.p.clone().unwrap_or_default();
        if p.chapters.is_empty() {
            return div().child(mv_ui::empty_state("No chapters yet", "Tell the AI about your manga in the Chat tab; it outlines the chapters and writes each one page by page."));
        }
        let n = self.chapter;
        let ch = p.chapter(n).cloned().unwrap_or_default();
        let todo = jobs::plan(&p, &Proposal::Pages { chapter: n, pages: vec![] });
        let (images, credits) = jobs::estimate(&p, &self.keys, &todo);
        let painted = ch.painted();
        let weak = cx.entity().downgrade();
        let export_items = vec![
            {
                let w = weak.clone();
                MenuItem::new(format!("Chapter {n} as PDF…"), move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.export_pdf(Some(n), cx));
                })
                .icon(ph::BOOK_OPEN)
            },
            {
                let w = weak.clone();
                MenuItem::new(format!("Chapter {n} as images…"), move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.export_images(Some(n), cx));
                })
                .icon(ph::IMAGES)
            },
            MenuItem::separator(),
            {
                let w = weak.clone();
                MenuItem::new("Every chapter as one PDF…", move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.export_pdf(None, cx));
                })
                .icon(ph::BOOKS)
            },
            {
                let w = weak.clone();
                MenuItem::new("Every chapter as images…", move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.export_images(None, cx));
                })
                .icon(ph::FOLDER_OPEN)
            },
        ];
        let write_label = if ch.pages.is_empty() { "Write this chapter" } else { "Rewrite script" };
        div()
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(self.chapter_picker(&p, cx))
            .child(self.look_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(240.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Chapter {n}{}", if ch.title.is_empty() { String::new() } else { format!(" · {}", ch.title) })))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).line_clamp(3).child(if ch.summary.is_empty() { "Not outlined yet.".to_string() } else { ch.summary.clone() }))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{} pages scripted · {painted} painted", ch.pages.len()))),
                    )
                    .child(btn("cr-write", write_label, ButtonKind::Secondary).small().icon(ph::PENCIL_SIMPLE).loading(self.thinking.is_some()).on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = crate::page::Tab::Chat;
                        this.send(format!("Write chapter {n}."), cx);
                    })))
                    .when(!todo.is_empty(), |d| {
                        d.child(btn("cr-paint-ch", format!("Paint {} ({images} images{})", if painted == 0 { "chapter".to_string() } else { "the rest".to_string() }, if credits > 0. { format!(", ~{} credits", credits.round()) } else { String::new() }), ButtonKind::Primary).small().icon(ph::PAINT_BRUSH).on_click(cx.listener(move |this, _, _, cx| this.enqueue(todo.clone(), cx))))
                    })
                    .when(painted > 0, |d| d.child(menu_button("cr-export", ph::EXPORT, "Export", export_items).label("Export").small().min_width(240.))),
            )
            .child(div().flex().flex_wrap().gap(px(space::MD)).children(ch.pages.iter().map(|pg| self.page_tile(&p, n, pg, cx))))
    }

    fn page_tile(&self, p: &Project, chapter: u32, pg: &Page, cx: &mut Context<Self>) -> Div {
        let file = existing(p, pg.image.as_ref());
        let number = pg.number;
        let painting = self.running.as_ref().is_some_and(|r| r.label == format!("Chapter {chapter} · page {number}")) || self.queue.contains(&Task::Page { chapter, page: number });
        let script = pg.panels.iter().enumerate().map(|(i, x)| {
            let words: Vec<String> = x.lines.iter().map(|l| format!("{}: {}", l.speaker, l.text)).collect();
            format!("{}. {}{}", i + 1, x.description, if words.is_empty() { String::new() } else { format!(" — {}", words.join(" / ")) })
        }).collect::<Vec<_>>().join("\n");
        let id = ("cr-page", number as usize);
        let body = match file {
            Some(f) => self.thumb(id, Some(f), PAGE_W, PAGE_W * 1.5, cx).into_any_element(),
            None => div()
                .id(id)
                .w(px(PAGE_W))
                .h(px(PAGE_W * 1.5))
                .flex_none()
                .rounded(px(radius::MD))
                .bg(Tokens::surface_3())
                .p(px(8.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(format!("{} · {} panels", pg.layout, pg.panels.len())))
                .child(div().text_size(px(10.)).text_color(Tokens::text_3()).whitespace_normal().child(script))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(px(PAGE_W))
            .child(body)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_2()).truncate().child(format!("Page {number}{}", if pg.beat.is_empty() { String::new() } else { format!(" · {}", pg.beat) })))
                    .child(btn(("cr-repaint", number as usize), if pg.image.is_some() { "Repaint" } else { "Paint" }, ButtonKind::Ghost).small().loading(painting).on_click(cx.listener(move |this, _, _, cx| {
                        let tasks = this.p.as_ref().map(|p| jobs::plan(p, &Proposal::Pages { chapter, pages: vec![number] })).unwrap_or_default();
                        this.enqueue(tasks, cx);
                    }))),
            )
    }

    /// The painted pages of chapter `n` (or every chapter), in order.
    fn painted_pages(&self, n: Option<u32>) -> Vec<(u32, u32, std::path::PathBuf)> {
        let Some(p) = self.p.as_ref() else { return vec![] };
        p.chapters.iter().filter(|c| n.is_none_or(|n| c.number == n)).flat_map(|c| c.pages.iter().filter_map(move |pg| existing(p, pg.image.as_ref()).map(|f| (c.number, pg.number, f)))).collect()
    }

    fn export_pdf(&mut self, n: Option<u32>, cx: &mut Context<Self>) {
        let pages: Vec<_> = self.painted_pages(n).into_iter().map(|(_, _, f)| f).collect();
        let Some(p) = self.p.as_ref() else { return };
        let title = match n {
            Some(n) => format!("{} - Chapter {n}", p.title),
            None => p.title.clone(),
        };
        let dir = dirs_documents();
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{}.pdf", file_safe(&title))));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let path = if path.extension().is_none() { path.with_extension("pdf") } else { path };
            let (t, out) = (title.clone(), path.clone());
            let res = mv_ui::bg::blocking(move || mv_create::pdf::write(&pages, &t, &out)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        toast::show(cx, Toast::success(format!("Saved {}", path.display())).icon(ph::BOOK_OPEN));
                        this.note(format!("Exported “{title}” as PDF to {}", path.display()));
                    }
                    Err(e) => this.error = Some(format!("Could not export the PDF: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_images(&mut self, n: Option<u32>, cx: &mut Context<Self>) {
        let pages = self.painted_pages(n);
        let Some(p) = self.p.as_ref() else { return };
        let stem = file_safe(&p.title);
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Export here".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(dirs))) = rx.await else { return };
            let Some(dir) = dirs.into_iter().next() else { return };
            let target = dir.clone();
            let res = mv_ui::bg::blocking(move || -> anyhow::Result<usize> {
                for (c, pg, f) in &pages {
                    std::fs::copy(f, target.join(format!("{stem}_ch{c:02}_p{pg:03}.png")))?;
                }
                Ok(pages.len())
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(k) => {
                        toast::show(cx, Toast::success(format!("Saved {k} pages")).icon(ph::IMAGES));
                        this.note(format!("Exported {k} page images to {}", dir.display()));
                    }
                    Err(e) => this.error = Some(format!("Could not export the images: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn dirs_documents() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join("Documents")).filter(|d| d.is_dir()).unwrap_or_else(mv_core::paths::root)
}

/// A title as a file name.
fn file_safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() {
        "manga".into()
    } else {
        t
    }
}
