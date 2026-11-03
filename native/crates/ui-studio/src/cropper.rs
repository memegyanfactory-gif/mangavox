//! The Read step's panel cropper: the user draws a page's panels by hand (add, move, resize,
//! delete, reorder) and saves them as the page's panels (`mv_recap::set_manual_page`). Pages
//! cropped by hand skip the automatic frame finding and crops on every read; "Reset to
//! automatic" gives a page back to them. The box maths are pure functions at the bottom.
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::Bbox;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, icon_button, icon_ph, muted, ph, size, ButtonKind, Tokens};

use crate::page::{plural, Phase, StudioPage};
use crate::run::RunKind;

/// Grip reach around a box edge, in screen pixels.
const GRIP_PX: f32 = 7.;
/// A drawn box smaller than this (share of the page side) is a stray click.
const MIN_SIDE: f32 = 0.01;
const ZOOMS: [f32; 6] = [0.5, 0.75, 1., 1.5, 2., 3.];
const LIST_W: f32 = 220.;

/// The open cropper: one page of the focused chapter and the boxes being drawn on it.
pub(crate) struct Crop {
    pub chapter: String,
    pub page: usize,
    pub boxes: Vec<Bbox>,
    pub sel: Option<usize>,
    drag: Option<Drag>,
    zoom: usize,
    pub dirty: bool,
    busy: bool,
    area: Rc<Cell<Bounds<Pixels>>>,
    scroll: ScrollHandle,
}

/// Which sides of a box a drag moves; all false = the whole box.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) struct Sides {
    pub l: bool,
    pub t: bool,
    pub r: bool,
    pub b: bool,
}

impl Sides {
    fn any(self) -> bool {
        self.l || self.t || self.r || self.b
    }
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Drawing box `n` from the anchor.
    New { n: usize, from: (f32, f32) },
    /// Moving or resizing box `n` as it was at the press, from the press point.
    Edit { n: usize, sides: Sides, start: Bbox, from: (f32, f32) },
}

impl StudioPage {
    /// Opens the cropper on a page of the focused chapter: the page's own boxes when it was
    /// cropped by hand, else the AI's panels to start from.
    pub(crate) fn open_crop(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let file = f.pages.get(page).and_then(|(p, _)| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let boxes = match f.manual.boxes(page, &file) {
            Some(b) => b.to_vec(),
            None => f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == page)).map(|p| p.panels.iter().filter_map(|pn| pn.bbox).collect()).unwrap_or_default(),
        };
        let zoom = self.crop.as_ref().map(|c| c.zoom).unwrap_or(2);
        self.crop = Some(Crop { chapter: f.item.id.clone(), page, boxes, sel: None, drag: None, zoom, dirty: false, busy: false, area: Rc::new(Cell::new(Bounds::default())), scroll: ScrollHandle::new() });
        self.set_phase(Phase::Read, cx);
        cx.notify();
    }

    /// The cropper belongs to the open chapter and the Read step.
    pub(crate) fn crop_open(&self) -> bool {
        self.phase == Phase::Read && self.crop.as_ref().is_some_and(|c| self.focus.as_ref().is_some_and(|f| f.item.id == c.chapter))
    }

    fn crop_close(&mut self, cx: &mut Context<Self>) {
        self.crop = None;
        cx.notify();
    }

    /// Why the boxes cannot be saved now: a read of this chapter writes the same files.
    fn crop_blocker(&self) -> Option<&'static str> {
        let id = self.crop.as_ref()?.chapter.as_str();
        self.run.as_ref().is_some_and(|r| r.running() && r.kind == RunKind::Read && r.chapter_id.as_deref() == Some(id)).then_some("Wait for the read of this chapter to finish")
    }

    /// Saves the boxes as the page's panels; then shows `next` (another page) or closes.
    fn crop_save(&mut self, next: Option<usize>, cx: &mut Context<Self>) {
        let (Some(f), Some(c)) = (&self.focus, self.crop.as_mut()) else { return };
        if c.busy {
            return;
        }
        c.busy = true;
        let (ch, page, boxes) = (f.chref.clone(), c.page, c.boxes.clone());
        let n = boxes.len();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || mv_recap::set_manual_page(&ch, page, boxes)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Some(c) = this.crop.as_mut() {
                    c.busy = false;
                }
                match res {
                    Ok(()) => {
                        let what = if n == 0 { format!("Page {} is back to automatic panels", page + 1) } else { format!("Page {}: {n} panel{} saved", page + 1, plural(n)) };
                        toast::show(cx, Toast::success(what).icon(ph::CROP));
                        match next {
                            Some(p) => this.open_crop(p, cx),
                            None => this.crop = None,
                        }
                    }
                    Err(e) => this.fail(format!("Could not save the panels: {e:#}")),
                }
                this.refresh_manual(cx);
                this.refresh_focus(true, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Gives a page back to the automatic panels (from the Read row or the cropper).
    pub(crate) fn reset_crop(&mut self, page: usize, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let ch = f.chref.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || mv_recap::reset_manual_page(&ch, page)).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(again) => {
                        if this.crop.as_ref().is_some_and(|c| c.page == page) {
                            this.crop = None;
                        }
                        toast::show(cx, Toast::info(format!("Page {} is back to automatic panels", page + 1)).icon(ph::ARROW_COUNTER_CLOCKWISE));
                        // Cropped by hand before it was read: the AI has no panels of its own yet.
                        if again {
                            this.reread_page(page, cx);
                        }
                    }
                    Err(e) => this.fail(format!("Could not reset the panels: {e:#}")),
                }
                this.refresh_manual(cx);
                this.refresh_focus(true, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-reads the focused chapter's hand-cropped pages.
    pub(crate) fn refresh_manual(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.focus else { return };
        let (ch, id) = (f.chref.clone(), f.item.id.clone());
        cx.spawn(async move |this, cx| {
            let Ok(m) = mv_ui::bg::blocking(move || mv_recap::load_manual(&ch)).await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(f) = this.focus.as_mut().filter(|f| f.item.id == id) {
                    f.manual = m;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Delete removes the picked panel, Escape lets go of it (then closes when nothing changed).
    pub(crate) fn crop_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.crop_open() {
            return false;
        }
        let k = &ev.keystroke;
        if k.modifiers.modified() {
            return false;
        }
        let Some(c) = self.crop.as_mut() else { return false };
        match k.key.as_str() {
            "delete" | "backspace" if c.sel.is_some() => {
                let n = c.sel.take().unwrap_or(0);
                if n < c.boxes.len() {
                    c.boxes.remove(n);
                    c.dirty = true;
                }
            }
            "escape" if c.sel.is_some() => c.sel = None,
            "escape" if !c.dirty => self.crop = None,
            _ => return false,
        }
        cx.notify();
        true
    }

    fn crop_at(&self, pos: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.crop.as_ref()?.area.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        (w > 0. && h > 0.).then(|| (((f32::from(pos.x) - f32::from(b.origin.x)) / w).clamp(0., 1.), ((f32::from(pos.y) - f32::from(b.origin.y)) / h).clamp(0., 1.)))
    }

    fn crop_down(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(p) = self.crop_at(pos) else { return };
        let Some(c) = self.crop.as_mut() else { return };
        let b = c.area.get();
        let tol = (GRIP_PX / f32::from(b.size.width).max(1.), GRIP_PX / f32::from(b.size.height).max(1.));
        match hit(&c.boxes, c.sel, p, tol) {
            Some((n, sides)) => {
                c.sel = Some(n);
                c.drag = Some(Drag::Edit { n, sides, start: c.boxes[n], from: p });
            }
            None => {
                c.boxes.push(Bbox([p.0, p.1, p.0, p.1]));
                let n = c.boxes.len() - 1;
                c.sel = Some(n);
                c.drag = Some(Drag::New { n, from: p });
            }
        }
        cx.notify();
    }

    fn crop_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(p) = self.crop_at(pos) else { return };
        let Some(c) = self.crop.as_mut() else { return };
        let Some(drag) = c.drag else { return };
        match drag {
            Drag::New { n, from } => c.boxes[n] = span(from, p),
            Drag::Edit { n, sides, start, from } => c.boxes[n] = edit(start, sides, (p.0 - from.0, p.1 - from.1)),
        }
        c.dirty = true;
        // A box taller than the view (a long strip panel): the view follows the pointer.
        let view = c.scroll.bounds();
        let (y, top, bottom) = (f32::from(pos.y), f32::from(view.origin.y), f32::from(view.origin.y + view.size.height));
        let step = if y > bottom - 24. { -24. } else if y < top + 24. { 24. } else { 0. };
        if step != 0. {
            let o = c.scroll.offset();
            let max = f32::from(c.scroll.max_offset().y);
            c.scroll.set_offset(point(o.x, px((f32::from(o.y) + step).clamp(-max, 0.))));
        }
        cx.notify();
    }

    fn crop_up(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.crop.as_mut() else { return };
        if let Some(Drag::New { n, .. }) = c.drag {
            let b = c.boxes[n];
            if b.0[2] - b.0[0] < MIN_SIDE || b.0[3] - b.0[1] < MIN_SIDE {
                c.boxes.remove(n);
                c.sel = None;
            }
        }
        c.drag = None;
        cx.notify();
    }

    fn crop_reorder(&mut self, n: usize, up: bool, cx: &mut Context<Self>) {
        let Some(c) = self.crop.as_mut() else { return };
        let m = if up { n.checked_sub(1) } else { Some(n + 1).filter(|m| *m < c.boxes.len()) };
        if let Some(m) = m {
            c.boxes.swap(n, m);
            c.sel = Some(m);
            c.dirty = true;
        }
        cx.notify();
    }

    fn crop_remove(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(c) = self.crop.as_mut() else { return };
        if n < c.boxes.len() {
            c.boxes.remove(n);
            c.sel = None;
            c.dirty = true;
        }
        cx.notify();
    }

    /// The cropper, in place of the Read step's page list.
    pub(crate) fn crop_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(f), Some(c)) = (&self.focus, &self.crop) else { return div().into_any_element() };
        let (page, total) = (c.page, f.pages.len());
        let Some((path, ratio)) = f.pages.get(page).cloned() else { return div().into_any_element() };
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let manual = f.manual.boxes(page, &file).is_some();
        let auto: Vec<Bbox> = f.read.as_ref().and_then(|r| r.pages.iter().find(|p| p.index == page)).map(|p| p.panels.iter().filter_map(|pn| pn.bbox).collect()).unwrap_or_default();
        let blocker = self.crop_blocker();
        let (boxes, sel, zoom, busy, dirty) = (c.boxes.clone(), c.sel, c.zoom, c.busy, c.dirty);
        let ratio = ratio.unwrap_or(0.7).max(0.02);
        // Fit the page to the room, then the zoom: a tall strip fits its width and scrolls.
        let room_w = (self.center_w - LIST_W - 3. * space::LG).max(200.);
        let fit_w = if ratio < 0.45 { room_w.min(720.) } else { room_w.min(560. * ratio / 0.7).max(240.) };
        let disp_w = fit_w * ZOOMS[zoom];
        let disp_h = disp_w / ratio;

        let nav = |id: &'static str, icon, label: &'static str, to: Option<usize>| {
            icon_button(id, icon, label).small().when(to.is_none(), |b| b.disabled("No more pages")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let Some(to) = to else { return };
                if this.crop.as_ref().is_some_and(|c| c.dirty) && this.crop_blocker().is_none() {
                    this.crop_save(Some(to), cx);
                } else {
                    this.open_crop(to, cx);
                }
            }))
        };
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::LG))
            .py(px(space::SM))
            .border_b_1()
            .border_color(Tokens::border())
            .child(icon_ph(ph::CROP, Tokens::accent(), icon_px::MD))
            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Crop panels"))
            .child(nav("crop-prev", ph::CARET_LEFT, "Previous page", page.checked_sub(1)))
            .child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).child(format!("Page {} / {total}", page + 1)))
            .child(nav("crop-next", ph::CARET_RIGHT, "Next page", Some(page + 1).filter(|p| *p < total)))
            .when(manual, |d| d.child(mv_ui::tag("Cropped by hand", mv_ui::Tone::Accent)))
            .child(div().flex_1())
            .child(icon_button("crop-zoom-out", ph::MINUS, "Zoom out").small().when(zoom == 0, |b| b.disabled("Smallest")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(c) = this.crop.as_mut() {
                    c.zoom = c.zoom.saturating_sub(1);
                }
                cx.notify();
            })))
            .child(div().w(px(40.)).text_center().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{}%", (ZOOMS[zoom] * 100.) as u32)))
            .child(icon_button("crop-zoom-in", ph::PLUS, "Zoom in").small().when(zoom + 1 >= ZOOMS.len(), |b| b.disabled("Largest")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(c) = this.crop.as_mut() {
                    c.zoom = (c.zoom + 1).min(ZOOMS.len() - 1);
                }
                cx.notify();
            })))
            .when(manual, |d| {
                d.child(
                    btn("crop-reset", "Reset to automatic", ButtonKind::Ghost)
                        .small()
                        .icon(ph::ARROW_COUNTER_CLOCKWISE)
                        .tooltip("Forget these panels: the AI's own panels and crops come back")
                        .when_some(blocker, |b, r| b.disabled(r))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_crop(page, cx))),
                )
            })
            .child(btn("crop-cancel", if dirty { "Discard" } else { "Close" }, ButtonKind::Secondary).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.crop_close(cx))))
            .child(
                btn("crop-save", "Save panels", ButtonKind::Primary)
                    .small()
                    .icon(ph::CHECK)
                    .when(busy, |b| b.loading(true))
                    .when(boxes.is_empty() && !manual, |b| b.disabled("Draw at least one panel"))
                    .when_some(blocker, |b, r| b.disabled(r))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.crop_save(None, cx))),
            );
        let hint = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::LG))
            .py(px(6.))
            .child(muted("Drag on the page to add a panel. Drag a panel to move it, its edges or corners to resize. The numbers are the reading order. Delete removes the picked panel."))
            .when(!auto.is_empty() && boxes != auto, |d| {
                d.child(btn("crop-use-ai", "Start from the AI's panels", ButtonKind::Ghost).small().icon(ph::ARROW_COUNTER_CLOCKWISE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(c) = this.crop.as_mut() {
                        c.boxes = auto.clone();
                        c.sel = None;
                        c.dirty = true;
                    }
                    cx.notify();
                })))
            })
            .when(!boxes.is_empty(), |d| {
                d.child(btn("crop-clear", "Clear all", ButtonKind::Ghost).small().icon(ph::TRASH).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(c) = this.crop.as_mut() {
                        c.boxes.clear();
                        c.sel = None;
                        c.dirty = true;
                    }
                    cx.notify();
                })))
            });

        let accent = Tokens::accent();
        let frames = boxes.iter().enumerate().map(|(n, b)| {
            let [x0, y0, x1, y1] = b.0;
            let picked = sel == Some(n);
            let grip = |l: f32, t: f32| div().absolute().left(relative(l)).top(relative(t)).ml(px(-4.)).mt(px(-4.)).size(px(8.)).rounded(px(2.)).bg(accent).border_1().border_color(Tokens::text_1());
            div()
                .absolute()
                .left(relative(x0))
                .top(relative(y0))
                .w(relative(x1 - x0))
                .h(relative(y1 - y0))
                .border_2()
                .border_color(if picked { accent } else { Rgba { a: 0.85, ..Tokens::warning() } })
                .bg(Rgba { a: if picked { 0.16 } else { 0.06 }, ..accent })
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(2.))
                        .min_w(px(20.))
                        .h(px(20.))
                        .px(px(4.))
                        .rounded(px(radius::SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(if picked { accent } else { Tokens::scrim() })
                        .text_size(px(size::XS))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Tokens::text_1())
                        .child(format!("{}", n + 1)),
                )
                .when(picked, |d| d.children([(0., 0.), (0.5, 0.), (1., 0.), (1., 0.5), (1., 1.), (0.5, 1.), (0., 1.), (0., 0.5)].map(|(l, t)| grip(l, t))))
        });
        let area = c.area.clone();
        let canvas_el = div()
            .id("crop-page")
            .relative()
            .flex_none()
            .w(px(disp_w))
            .h(px(disp_h))
            .bg(Tokens::surface())
            .cursor_crosshair()
            .child(img(path.clone()).size_full())
            .child(canvas(move |b, _, _| area.set(b), |_, _, _, _| {}).absolute().inset_0())
            .children(frames)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.crop_down(ev.position, cx);
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    this.crop_move(ev.position, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.crop_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.crop_up(cx)));
        let scroller = div()
            .id("crop-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_scroll()
            .track_scroll(&c.scroll)
            .bg(Tokens::well())
            .rounded(px(radius::MD))
            .p(px(space::MD))
            .child(div().flex().justify_center().min_w(px(disp_w)).child(canvas_el));

        let n_boxes = boxes.len();
        let rows = boxes.iter().enumerate().map(|(n, b)| {
            let [x0, y0, x1, y1] = b.0;
            let picked = sel == Some(n);
            div()
                .id(("crop-row", n))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(space::SM))
                .py(px(4.))
                .rounded(px(radius::SM))
                .cursor_pointer()
                .when(picked, |d| d.bg(Tokens::surface_2()))
                .hover(|s| s.bg(Tokens::surface_2()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(c) = this.crop.as_mut() {
                        c.sel = Some(n);
                    }
                    cx.notify();
                }))
                .child(div().w(px(22.)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(if picked { accent } else { Tokens::text_1() }).child(format!("{}", n + 1)))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{}×{}%", ((x1 - x0) * 100.).round(), ((y1 - y0) * 100.).round())))
                .child(icon_button(("crop-up", n), ph::ARROW_UP, "Earlier in the reading order").small().when(n == 0, |b| b.disabled("Already first")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.crop_reorder(n, true, cx))))
                .child(
                    icon_button(("crop-down", n), ph::ARROW_DOWN, "Later in the reading order")
                        .small()
                        .when(n + 1 == n_boxes, |b| b.disabled("Already last"))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.crop_reorder(n, false, cx))),
                )
                .child(icon_button(("crop-del", n), ph::TRASH, "Delete this panel").small().danger().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.crop_remove(n, cx))))
        });
        let list = div()
            .id("crop-list")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(LIST_W))
            .h_full()
            .overflow_y_scroll()
            .gap(px(2.))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(div().px(px(space::SM)).py(px(4.)).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{n_boxes} panel{}", plural(n_boxes))))
            .children(rows)
            .when(n_boxes == 0, |d| d.child(div().px(px(space::SM)).child(muted("No panels yet. Drag on the page to draw one."))))
            .child(div().mt(px(space::SM)).px(px(space::SM)).child(muted("Saved panels are used as they are: the next reads skip finding and cropping panels on this page.")));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(hint)
            .child(div().flex().flex_1().min_h_0().gap(px(space::MD)).px(px(space::LG)).pb(px(space::LG)).child(scroller).child(list))
            .into_any_element()
    }
}

/// The box from one corner to the other.
pub(crate) fn span(a: (f32, f32), b: (f32, f32)) -> Bbox {
    Bbox([a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)])
}

/// `start` with the dragged sides (or the whole box) moved by `d`, kept on the page.
pub(crate) fn edit(start: Bbox, sides: Sides, d: (f32, f32)) -> Bbox {
    let [x0, y0, x1, y1] = start.0;
    if !sides.any() {
        let (w, h) = (x1 - x0, y1 - y0);
        let (nx, ny) = ((x0 + d.0).clamp(0., 1. - w), (y0 + d.1).clamp(0., 1. - h));
        return Bbox([nx, ny, nx + w, ny + h]);
    }
    let c = |v: f32| v.clamp(0., 1.);
    let (mut a, mut b, mut e, mut f) = (x0, y0, x1, y1);
    if sides.l {
        a = c(x0 + d.0).min(x1 - MIN_SIDE);
    }
    if sides.r {
        e = c(x1 + d.0).max(x0 + MIN_SIDE);
    }
    if sides.t {
        b = c(y0 + d.1).min(y1 - MIN_SIDE);
    }
    if sides.b {
        f = c(y1 + d.1).max(y0 + MIN_SIDE);
    }
    Bbox([a, b, e, f])
}

/// What a press at `p` takes: a box's edge or corner (the picked box first), else the smallest
/// box under it to move, else nothing (a new box is drawn).
pub(crate) fn hit(boxes: &[Bbox], sel: Option<usize>, p: (f32, f32), tol: (f32, f32)) -> Option<(usize, Sides)> {
    let (x, y) = p;
    let near = |n: usize| {
        let [x0, y0, x1, y1] = boxes[n].0;
        let (in_x, in_y) = (x >= x0 - tol.0 && x <= x1 + tol.0, y >= y0 - tol.1 && y <= y1 + tol.1);
        let s = Sides { l: in_y && (x - x0).abs() <= tol.0, r: in_y && (x - x1).abs() <= tol.0, t: in_x && (y - y0).abs() <= tol.1, b: in_x && (y - y1).abs() <= tol.1 };
        // A box thinner than the grips: the nearer side only.
        Sides { l: s.l && !(s.r && (x - x1).abs() < (x - x0).abs()), r: s.r && !(s.l && (x - x0).abs() <= (x - x1).abs()), t: s.t && !(s.b && (y - y1).abs() < (y - y0).abs()), b: s.b && !(s.t && (y - y0).abs() <= (y - y1).abs()) }
    };
    let order: Vec<usize> = sel.filter(|s| *s < boxes.len()).into_iter().chain((0..boxes.len()).rev().filter(|n| Some(*n) != sel)).collect();
    if let Some(n) = order.iter().copied().find(|&n| near(n).any()) {
        return Some((n, near(n)));
    }
    let inside = |n: &usize| {
        let [x0, y0, x1, y1] = boxes[*n].0;
        x >= x0 && x <= x1 && y >= y0 && y <= y1
    };
    order.into_iter().filter(inside).min_by(|a, b| boxes[*a].area().total_cmp(&boxes[*b].area())).map(|n| (n, Sides::default()))
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{edit, hit, span, Bbox, Sides};

    const TOL: (f32, f32) = (0.01, 0.01);

    #[test]
    fn a_press_takes_an_edge_a_box_or_nothing() {
        let boxes = [Bbox([0.1, 0.1, 0.5, 0.5]), Bbox([0.2, 0.2, 0.3, 0.3])];
        assert_eq!(hit(&boxes, None, (0.1, 0.3), TOL), Some((0, Sides { l: true, ..Default::default() })));
        assert_eq!(hit(&boxes, None, (0.5, 0.5), TOL), Some((0, Sides { r: true, b: true, ..Default::default() })));
        // Inside both: the smaller, inner box moves.
        assert_eq!(hit(&boxes, None, (0.25, 0.25), TOL), Some((1, Sides::default())));
        assert_eq!(hit(&boxes, None, (0.4, 0.4), TOL), Some((0, Sides::default())));
        assert_eq!(hit(&boxes, None, (0.8, 0.8), TOL), None);
    }

    #[test]
    fn moving_and_resizing_stay_on_the_page() {
        let b = Bbox([0.1, 0.1, 0.3, 0.9]);
        let m = edit(b, Sides::default(), (0.5, 0.5)).0;
        for (got, want) in m.iter().zip([0.6, 0.2, 0.8, 1.0]) {
            assert!((got - want).abs() < 1e-5, "{m:?}");
        }
        let r = edit(b, Sides { r: true, b: true, ..Default::default() }, (0.9, -0.95));
        assert_eq!(r.0[2], 1.0);
        assert!(r.0[3] > r.0[1], "a side never crosses the other");
        // A tall strip panel: almost the whole height.
        assert_eq!(span((0.0, 0.02), (1.0, 0.97)).0, [0.0, 0.02, 1.0, 0.97]);
        assert_eq!(span((0.6, 0.6), (0.2, 0.1)).0, [0.2, 0.1, 0.6, 0.6]);
    }
}
