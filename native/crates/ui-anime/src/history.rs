//! The take history of one card (design 5.6, 9.3): every version it ever had, newest first,
//! with "Use this" (restore = select), Compare (the version in use beside an earlier one) and
//! Open file; the one in use is outlined in accent. Pictures show as 16:9 thumbnails, voices as
//! play rows. Nothing here deletes a take: history never shrinks.
//!
//! ```ignore
//! let history = cx.new(|_| History::new(&ctx.project, card.takes.clone(), card.selected.clone())
//!     .title("Frame 3.4")
//!     .on_select(move |take, window, cx| { /* set card.selected = Some(take), save */ }));
//! // later, after the card changed:
//! history.update(cx, |h, cx| { h.set(card.takes.clone(), card.selected.clone()); cx.notify() });
//! ```


use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::{Store, Take, TakeKind, TakeSource};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, icon_button, icon_ph, ph, size, tag, ButtonKind, Tokens, Tone};

use crate::style::HISTORY_THUMB;
use crate::widgets::{ago, count, label_caps, now_secs};

type Handler = Rc<dyn Fn(String, &mut Window, &mut App)>;
type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The History button every card carries.
pub fn history_button(id: impl Into<ElementId>, open: bool) -> mv_ui::IconBtn {
    icon_button(id, ph::CLOCK_COUNTER_CLOCKWISE, if open { "Hide earlier versions" } else { "Earlier versions: use or compare any of them" }).selected(open)
}

/// One card's versions. An entity: create it with `cx.new(|_| History::new(..))`.
pub struct History {
    store: Store,
    takes: Vec<String>,
    selected: Option<String>,
    title: SharedString,
    compare: Option<String>,
    cache: HashMap<String, Option<Take>>,
    /// The versions are being read from disk.
    reading: bool,
    on_select: Option<Handler>,
    on_close: Option<CloseHandler>,
    scroll: ScrollHandle,
}

impl History {
    /// `takes` oldest first (as a card stores them); `selected` is the take in use.
    pub fn new(project: &str, takes: Vec<String>, selected: Option<String>) -> Self {
        History { store: Store::open(project), takes, selected, title: SharedString::default(), compare: None, cache: HashMap::new(), reading: false, on_select: None, on_close: None, scroll: ScrollHandle::new() }
    }

    /// What the versions are of ("Frame 3.4"), shown in the header.
    pub fn title(mut self, t: impl Into<SharedString>) -> Self {
        self.title = t.into();
        self
    }

    /// "Use this" pressed on a take: make it the card's selected take and save.
    pub fn on_select(mut self, f: impl Fn(String, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Shows a close button that calls `f`.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    /// The card changed (a new take, another one selected).
    pub fn set(&mut self, takes: Vec<String>, selected: Option<String>) {
        if self.compare.as_ref().is_some_and(|c| !takes.contains(c)) {
            self.compare = None;
        }
        // A version that could not be read before (still being written) is read again.
        self.cache.retain(|_, t| t.is_some());
        self.takes = takes;
        self.selected = selected;
    }

    /// Reads the versions not read yet, in the background (never on the UI thread).
    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        let missing: Vec<String> = self.takes.iter().filter(|id| !self.cache.contains_key(*id)).cloned().collect();
        if missing.is_empty() {
            return;
        }
        self.reading = true;
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || missing
                .into_iter()
                .map(|id| {
                    let take = store.take(&id);
                    (id, take)
                })
                .collect::<Vec<_>>()).await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                if let Ok(read) = res {
                    this.cache.extend(read);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn file(&self, t: &Take) -> PathBuf {
        self.store.path(&t.file)
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        self.compare = None;
        if let Some(f) = self.on_select.clone() {
            // After this update: the handler may update this entity again.
            window.defer(cx, move |w, cx| f(id, w, cx));
        }
        cx.notify();
    }
}

/// Who made a take, in a word.
pub fn source_label(s: &TakeSource) -> String {
    match s {
        TakeSource::MagnificWeb { .. } => "Magnific".into(),
        // The voice's name from the catalog; an unknown voice is never shown by its id.
        TakeSource::Voice { voice, .. } => mv_studio::tts::find_voice(voice).map(|v| v.label.trim()).filter(|l| !l.is_empty()).map(str::to_string).unwrap_or_else(|| "Voice".into()),
        TakeSource::Upload { .. } => "Your file".into(),
        TakeSource::MangaCrop(_) => "From the manga".into(),
        TakeSource::Ffmpeg => "Animatic".into(),
        TakeSource::Unknown => String::new(),
    }
}

impl Render for History {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        self.fetch(cx);
        let now = now_secs();
        let takes: Vec<Take> = self.takes.iter().rev().filter_map(|id| self.cache.get(id).cloned().flatten()).collect();
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(label_caps("Earlier versions"))
            .when(!self.title.is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(self.title.clone())))
            .child(div().flex_1())
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(count(takes.len(), "version")))
            .when_some(self.on_close.clone(), |d, f| d.child(icon_button("ah-close", ph::X, "Close earlier versions").small().on_click(move |_, w, cx| f(w, cx))));
        if takes.is_empty() && self.reading {
            return div().flex().flex_col().gap(px(space::SM)).child(head).child(div().flex().gap(px(space::SM)).children((0..3).map(|_| mv_ui::skeleton_block(px(HISTORY_THUMB.0), px(HISTORY_THUMB.1)))));
        }
        if takes.is_empty() {
            return div().flex().flex_col().gap(px(space::SM)).child(head).child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("Nothing made yet. Every version you make stays here, so you can go back to any of them."));
        }
        let compare = self.compare.clone().and_then(|c| takes.iter().find(|t| t.id == c).cloned());
        let current = self.selected.clone().and_then(|s| takes.iter().find(|t| t.id == s).cloned());
        let viewer = compare.map(|c| self.compare_view(current.as_ref(), &c, now, cx));
        let mut strip = div().flex().gap(px(space::SM));
        for (i, t) in takes.iter().enumerate() {
            strip = strip.child(self.tile(i, t, now, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(head)
            .children(viewer)
            .child(div().id("ah-strip").overflow_x_scroll().track_scroll(&self.scroll).pb(px(space::XS)).child(strip))
    }
}

impl History {
    fn tile(&self, i: usize, t: &Take, now: i64, cx: &mut Context<Self>) -> Div {
        let (w, h) = HISTORY_THUMB;
        let in_use = self.selected.as_deref() == Some(t.id.as_str());
        let file = self.file(t);
        let group = SharedString::from(format!("ah-tile-{i}"));
        let id = t.id.clone();
        let frame = div()
            .id(("ah-frame", i))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w(px(w))
            .h(px(h))
            .rounded(px(radius::MD))
            .overflow_hidden()
            .bg(Tokens::surface_2())
            .map(|d| if in_use { d.border_2().border_color(Tokens::accent()) } else { d.border_1().border_color(Tokens::border()) })
            .map(|d| match t.kind {
                // The whole picture, at its own aspect (a 3:4 design is never cut to 16:9).
                TakeKind::Image => d.child(img(mv_ui::thumbs::cover_source(file.clone())).size_full().object_fit(ObjectFit::Contain).with_fallback(|| icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::LG).into_any_element())),
                TakeKind::Audio => {
                    let f = file.clone();
                    let playing = ui_studio::audio::now_playing().as_deref() == Some(file.as_path());
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(space::XS))
                            .child(icon_button(("ah-play", i), if playing { ph::STOP } else { ph::PLAY }, if playing { "Stop" } else { "Play this version" }).on_click(move |_, _, _| ui_studio::audio::toggle(&f)))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{:.1} s", t.seconds))),
                    )
                }
                TakeKind::Video => d.child(icon_ph(ph::FILM_STRIP, Tokens::text_3(), icon_px::LG)),
                _ => d.child(icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::LG)),
            })
            .when(in_use, |d| d.child(div().absolute().bottom(px(4.)).left(px(4.)).child(tag("In use", Tone::Accent))));
        let mut actions = crate::widgets::hover_actions(group.clone());
        if !in_use {
            let pick = id.clone();
            actions = actions.child(icon_button(("ah-use", i), ph::CHECK, "Use this version").small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.select(pick.clone(), w, cx))));
            if t.kind == TakeKind::Image {
                let cmp = id.clone();
                actions = actions.child(icon_button(("ah-cmp", i), ph::COLUMNS, "Compare with the version in use").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.compare = Some(cmp.clone());
                    cx.notify();
                })));
            }
        }
        let open = file.clone();
        actions = actions.child(icon_button(("ah-open", i), ph::ARROW_SQUARE_OUT, "Open the file").small().on_click(move |_, _, cx| cx.open_with_system(&open)));
        div()
            .relative()
            .group(group)
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(space::XS))
            .w(px(w))
            .child(frame)
            .child(actions)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS))
                    .text_size(px(size::XS))
                    .text_color(Tokens::text_3())
                    .child(div().flex_none().child(ago(t.created_at, now)))
                    .child(div().min_w_0().truncate().child(source_label(&t.source))),
            )
    }

    fn compare_view(&self, current: Option<&Take>, other: &Take, now: i64, cx: &mut Context<Self>) -> Div {
        let side = |t: Option<&Take>, caption: String| {
            let path = t.map(|t| self.file(t));
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(space::XS))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w_full()
                        .h(px(180.))
                        .rounded(px(radius::MD))
                        .overflow_hidden()
                        .bg(Tokens::well())
                        .border_1()
                        .border_color(Tokens::border())
                        .map(|d| match path {
                            Some(p) => d.child(img(p).size_full().object_fit(ObjectFit::Contain).with_fallback(|| icon_ph(ph::IMAGE_BROKEN, Tokens::text_3(), icon_px::LG).into_any_element())),
                            None => d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("No version in use")),
                        }),
                )
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(caption))
        };
        let pick = other.id.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border())
            .child(div().flex().gap(px(space::MD)).child(side(current, current.map(|t| format!("In use · {}", ago(t.created_at, now))).unwrap_or_else(|| "In use".into()))).child(side(Some(other), format!("Earlier · {}", ago(other.created_at, now)))))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::SM))
                    .child(btn("ah-cmp-close", "Close", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.compare = None;
                        cx.notify();
                    })))
                    .child(btn("ah-cmp-use", "Use the earlier one", ButtonKind::Secondary).small().icon(ph::CHECK).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.select(pick.clone(), w, cx)))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::source_label;
    use mv_anime::TakeSource;

    #[test]
    fn sources_in_words() {
        assert_eq!(source_label(&TakeSource::MagnificWeb { model: "m".into(), res: "1K".into(), free: true, site_id: "x".into() }), "Magnific");
        assert_eq!(source_label(&TakeSource::Upload { name: "a.png".into() }), "Your file");
        let heart = source_label(&TakeSource::Voice { engine: "kokoro".into(), voice: "af_heart".into() });
        assert!(!heart.is_empty() && heart != "af_heart", "a voice by its name, never its id: {heart}");
        assert_eq!(source_label(&TakeSource::Voice { engine: "kokoro".into(), voice: "made_up_voice".into() }), "Voice");
    }
}
