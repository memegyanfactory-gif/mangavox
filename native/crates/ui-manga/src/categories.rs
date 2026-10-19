//! Library categories (Mihon's shelves): put a title on shelves from its page, and create,
//! rename and delete shelves from the Library. Both open in a dialog over the page.
//! Mihon categories: <https://mihon.app/docs/guides/categories>
use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::WindowExt as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::category::{self, LibraryCategory};
use mv_ui::theme::{icon as icon_px, radius, ramp};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, checkbox, confirm_inline, icon_button, inline_alert, muted, ph, ph_icon, size, AlertKind, ButtonKind, RampExt, Tokens, Weight};

use crate::backend;

enum Mode {
    /// Choose the shelves of one title.
    Assign { manga_id: String, title: String },
    /// Create, rename and delete shelves.
    Manage,
}

pub struct CategoryEditor {
    mode: Mode,
    cats: Option<Vec<LibraryCategory>>,
    /// Assign mode: shelves the title is on.
    checked: HashSet<String>,
    input: Entity<InputState>,
    /// Manage mode: the shelf being renamed and its text field.
    renaming: Option<(String, Entity<InputState>)>,
    confirm_delete: Option<String>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

fn load(manga_id: Option<String>) -> anyhow::Result<(Vec<LibraryCategory>, Vec<String>)> {
    backend::with_db(|c| {
        let cats = category::list(c)?;
        let member = match manga_id {
            Some(id) => category::manga_category_map(c)?.remove(&id).unwrap_or_default(),
            None => vec![],
        };
        Ok((cats, member))
    })
}

impl CategoryEditor {
    fn new(mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("New category, e.g. Reading"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.create(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let manga_id = match &mode {
            Mode::Assign { manga_id, .. } => Some(manga_id.clone()),
            Mode::Manage => None,
        };
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(manga_id)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((cats, member)) => {
                        this.cats = Some(cats);
                        this.checked = member.into_iter().collect();
                    }
                    Err(e) => {
                        this.cats = Some(vec![]);
                        this.error = Some(format!("{e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        Self { mode, cats: None, checked: HashSet::new(), input, renaming: None, confirm_delete: None, error: None, _subs: vec![sub] }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        if self.cats.as_ref().is_some_and(|cs| cs.iter().any(|c| c.name.eq_ignore_ascii_case(&name))) {
            self.error = Some(format!("There is already a category called \u{201c}{name}\u{201d}."));
            cx.notify();
            return;
        }
        match backend::with_db(|c| category::create(c, &name)) {
            Ok(cat) => {
                self.error = None;
                if matches!(self.mode, Mode::Assign { .. }) {
                    self.checked.insert(cat.id.clone());
                }
                self.cats.get_or_insert_with(Vec::new).push(cat);
                self.input.update(cx, |s, cx| s.set_value("", window, cx));
                backend::bump_data();
            }
            Err(e) => self.error = Some(format!("Could not create the category: {e:#}")),
        }
        cx.notify();
    }

    fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.checked.remove(id) {
            self.checked.insert(id.to_string());
        }
        cx.notify();
    }

    fn start_rename(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let rid = id.clone();
        let sub = cx.subscribe_in(&input, window, move |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.finish_rename(&rid, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self._subs.push(sub);
        self.renaming = Some((id, input));
        self.confirm_delete = None;
        cx.notify();
    }

    fn finish_rename(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((rid, input)) = self.renaming.take() else { return };
        if rid != id {
            return;
        }
        let name = input.read(cx).value().trim().to_string();
        if !name.is_empty() {
            match backend::with_db(|c| category::rename(c, id, &name)) {
                Ok(()) => {
                    if let Some(c) = self.cats.as_mut().and_then(|cs| cs.iter_mut().find(|c| c.id == id)) {
                        c.name = name;
                    }
                    backend::bump_data();
                }
                Err(e) => self.error = Some(format!("Could not rename: {e:#}")),
            }
        }
        cx.notify();
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        match backend::with_db(|c| category::delete(c, id)) {
            Ok(()) => {
                let name = self.cats.as_ref().and_then(|cs| cs.iter().find(|c| c.id == id)).map(|c| c.name.clone()).unwrap_or_default();
                if let Some(cs) = self.cats.as_mut() {
                    cs.retain(|c| c.id != id);
                }
                self.checked.remove(id);
                backend::bump_data();
                toast::show(cx, Toast::success(format!("Deleted the category \u{201c}{name}\u{201d}. Its titles stay in your library.")));
            }
            Err(e) => self.error = Some(format!("Could not delete: {e:#}")),
        }
        cx.notify();
    }

    /// Assign mode: write the chosen shelves.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Mode::Assign { manga_id, title } = &self.mode else { return };
        let ids: Vec<String> = self.cats.iter().flatten().filter(|c| self.checked.contains(&c.id)).map(|c| c.id.clone()).collect();
        let names: Vec<String> = self.cats.iter().flatten().filter(|c| self.checked.contains(&c.id)).map(|c| c.name.clone()).collect();
        let (mid, title) = (manga_id.clone(), title.clone());
        match backend::with_db(|c| category::set_manga_categories(c, &mid, &ids)) {
            Ok(()) => {
                backend::bump_data();
                let msg = match names.len() {
                    0 => format!("Removed {title} from every category"),
                    1 => format!("Moved {title} to {}", names[0]),
                    n => format!("Added {title} to {n} categories"),
                };
                toast::show(cx, Toast::success(msg));
            }
            Err(e) => toast::show(cx, Toast::error(format!("Could not save categories: {e:#}"))),
        }
    }

    fn row_assign(&self, i: usize, c: &LibraryCategory, cx: &mut Context<Self>) -> AnyElement {
        let id = c.id.clone();
        checkbox(("cat-check", i), self.checked.contains(&c.id))
            .w_full()
            .px(px(8.))
            .py(px(4.))
            .hover(|s| s.bg(Tokens::hover()))
            .child(c.name.clone())
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
            .into_any_element()
    }

    fn row_manage(&self, i: usize, c: &LibraryCategory, cx: &mut Context<Self>) -> AnyElement {
        let (id, id2, id3, name) = (c.id.clone(), c.id.clone(), c.id.clone(), c.name.clone());
        let row = div().id(("cat-row", i)).flex().items_center().gap(px(8.)).min_h(px(40.)).px(px(8.)).rounded(px(radius::MD));
        if self.confirm_delete.as_deref() == Some(c.id.as_str()) {
            return row
                .bg(Tokens::danger_soft())
                .child(
                    confirm_inline(format!("Delete \u{201c}{}\u{201d}? Titles stay in your library.", c.name), "Delete category", true)
                        .flex_1()
                        .on_confirm(cx.listener(move |this, _, _, cx| this.delete(&id, cx)))
                        .on_cancel(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        if let Some((_, input)) = self.renaming.as_ref().filter(|(rid, _)| rid == &c.id) {
            return row
                .child(div().flex_1().child(Input::new(input)))
                .child(btn(("cat-rename-save", i), "Save", ButtonKind::Primary).small().on_click(cx.listener(move |this, _, _, cx| this.finish_rename(&id, cx))))
                .child(btn(("cat-rename-cancel", i), "Cancel", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| {
                    this.renaming = None;
                    cx.notify();
                })))
                .into_any_element();
        }
        row.hover(|s| s.bg(Tokens::hover()))
            .child(ph_icon(ph::FOLDER_SIMPLE, Weight::Regular, Tokens::text_3(), icon_px::MD))
            .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).text_color(Tokens::text_1()).child(c.name.clone()))
            .child(icon_button(("cat-rename", i), ph::PENCIL_SIMPLE, "Rename").small().on_click(cx.listener(move |this, _, w, cx| this.start_rename(id2.clone(), name.clone(), w, cx))))
            .child(icon_button(("cat-delete", i), ph::TRASH, "Delete category").small().danger().on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_delete = Some(id3.clone());
                this.renaming = None;
                cx.notify();
            })))
            .into_any_element()
    }
}

impl Render for CategoryEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let assign = matches!(self.mode, Mode::Assign { .. });
        let list: AnyElement = match &self.cats {
            None => div().flex().flex_col().gap(px(8.)).children((0..3).map(|_| mv_ui::skeleton_line(relative(0.6)).h(px(20.)))).into_any_element(),
            Some(cs) if cs.is_empty() => muted(if assign { "No categories yet. Create one below and this title goes straight into it." } else { "No categories yet. Create shelves such as Reading, Plan to read or Finished." })
                .into_any_element(),
            Some(cs) => {
                let cs = cs.clone();
                div()
                    .id("cat-list")
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .children(cs.iter().enumerate().map(|(i, c)| if assign { self.row_assign(i, c, cx) } else { self.row_manage(i, c, cx) }))
                    .into_any_element()
            }
        };
        let empty_name = self.input.read(cx).value().trim().is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .child(div().flex_1().child(Input::new(&self.input).prefix(ph_icon(ph::FOLDER_SIMPLE_PLUS, Weight::Bold, Tokens::text_3(), icon_px::SM))))
                    .child(
                        btn("cat-add", "Add", ButtonKind::Secondary)
                            .icon(ph::PLUS)
                            .map(|b| if empty_name { b.disabled("Type a name first") } else { b })
                            .on_click(cx.listener(|this, _, w, cx| this.create(w, cx))),
                    ),
            )
            .when_some(self.error.clone(), |d, e| d.child(inline_alert(AlertKind::Error, e, "")))
    }
}

fn heading(icon: mv_ui::Ph, title: String) -> Div {
    div().flex().items_center().gap(px(10.)).child(ph_icon(icon, Weight::Fill, Tokens::accent(), icon_px::LG)).child(div().ramp(ramp::SUBTITLE_2).text_color(Tokens::text_1()).child(title))
}

/// "Set categories" for one title: a checkbox per shelf, a field to add a new one, Save.
pub fn open_assign(manga_id: String, title: String, window: &mut Window, cx: &mut App) {
    let head = format!("Categories for {title}");
    let editor = cx.new(|cx| CategoryEditor::new(Mode::Assign { manga_id, title }, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let save = editor.clone();
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .w_full()
            .child(btn("cat-cancel", "Cancel", ButtonKind::Secondary).on_click(|_, w, cx| w.close_dialog(cx)))
            .child(btn("cat-save", "Save", ButtonKind::Primary).icon(ph::CHECK).on_click(move |_, w, cx| {
                save.update(cx, |e, cx| e.save(cx));
                w.close_dialog(cx);
            }));
        dialog.w(px(460.)).title(heading(ph::FOLDER_SIMPLE, head.clone())).child(editor.clone()).footer(footer)
    });
}

/// "Manage categories" from the Library: create, rename and delete shelves.
pub fn open_manage(window: &mut Window, cx: &mut App) {
    let editor = cx.new(|cx| CategoryEditor::new(Mode::Manage, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let footer = div().flex().justify_end().w_full().child(btn("cat-done", "Done", ButtonKind::Primary).on_click(|_, w, cx| w.close_dialog(cx)));
        dialog.w(px(460.)).title(heading(ph::FOLDER_SIMPLE, "Library categories".into())).child(editor.clone()).footer(footer)
    });
}
