//! Activity cards: the persistent progress notifications for long work that keeps running in the
//! background (chapter downloads, Studio's Generate). One card per independent activity (one
//! title's downloads, one manga's Generate run), stacked in the bottom-right corner of the page
//! area with the toast below them, so nothing covers the tabs or the title bar.
//!
//! ```ignore
//! let cards = vec![Card::new("dl-7", order, Lead::Progress(0.4), "Solo Leveling", "Ch. 25 · page 3 of 20")
//!     .progress(0.4, true)
//!     .count("25 of 33 chapters")
//!     .action(cancel_button)
//!     .on_close(|_, cx| hide(cx))];
//! page.child(mv_ui::activity::corner(cards, Some(mv_ui::toast::view(cx)), cx))
//! ```
//!
//! - Producers turn their own state into [`Card`]s on every render; the shell draws them once.
//! - Sorted by [`Card::order`] (when the activity started, see [`next_order`]): newest at the
//!   bottom next to the corner, so running cards never swap places.
//! - Beyond [`MAX_SHOWN`] the older cards fold into a "+N more" row that expands the stack.
//! - Opaque surfaces: text over a busy page stays readable.
//!
//! Fluent 2 toast (stacked at the bottom end): <https://fluent2.microsoft.design/components/web/react/core/toast/usage>;
//! NN/g visibility of system status: <https://www.nngroup.com/articles/visibility-system-status/>
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::icon_button;
use crate::feedback::progress_ring_sized;
use crate::icons::{ph, ph_icon, Weight};
use crate::interact::Interactive as _;
use crate::motion::{progress_bar, reduced};
use crate::theme::{duration, easing, icon as icon_px, radius, size, Tokens};

/// Width of every card, and of the toast under them.
pub const WIDTH: f32 = 360.;
/// Distance from the page area's right and bottom edges.
pub const MARGIN: f32 = 16.;
/// Space between stacked cards (and above the toast).
pub const GAP: f32 = 8.;
/// Cards shown before the older ones fold into "+N more".
pub const MAX_SHOWN: usize = 4;

/// What leads a card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lead {
    /// Work in progress: a ring filled to the fraction.
    Progress(f32),
    Done,
    Failed,
}

pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One activity. Build with [`Card::new`]; draw a list of them with [`corner`].
pub struct Card {
    key: SharedString,
    order: u64,
    lead: Lead,
    title: SharedString,
    line: SharedString,
    progress: Option<(f32, bool)>,
    count: Option<SharedString>,
    actions: Vec<AnyElement>,
    on_close: Option<Handler>,
}

impl Card {
    /// `key` is unique per activity run ("dl-<serial>"): it scopes the card's buttons and plays
    /// the slide-in once. `title` is what the work is about (the manga), `line` what happens now.
    pub fn new(key: impl Into<SharedString>, order: u64, lead: Lead, title: impl Into<SharedString>, line: impl Into<SharedString>) -> Self {
        Card { key: key.into(), order, lead, title: title.into(), line: line.into(), progress: None, count: None, actions: vec![], on_close: None }
    }

    /// A bar under the text; `moving` is false while the work is paused.
    pub fn progress(mut self, fraction: f32, moving: bool) -> Self {
        self.progress = Some((fraction, moving));
        self
    }

    /// Left of the actions ("25 of 33 chapters").
    pub fn count(mut self, text: impl Into<SharedString>) -> Self {
        self.count = Some(text.into());
        self
    }

    /// A small button on the bottom row (ids only need to be unique within the card).
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.actions.push(el.into_any_element());
        self
    }

    /// The close button. It hides the card; the work carries on.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }

    pub fn order(&self) -> u64 {
        self.order
    }
}

static ORDER: AtomicU64 = AtomicU64::new(0);

/// A fresh [`Card::order`] for an activity that starts now, shared by every producer so their
/// cards interleave by age.
pub fn next_order() -> u64 {
    ORDER.fetch_add(1, Ordering::Relaxed) + 1
}

/// Which cards show: indices into `orders`, oldest first, and how many older ones fold away.
pub fn arrange(orders: &[u64], expanded: bool) -> (Vec<usize>, usize) {
    let mut idx: Vec<usize> = (0..orders.len()).collect();
    idx.sort_by_key(|&i| orders[i]);
    let hidden = if expanded { 0 } else { idx.len().saturating_sub(MAX_SHOWN) };
    (idx.split_off(hidden), hidden)
}

/// The "+N more" row is open.
#[derive(Default)]
struct Expanded(bool);
impl Global for Expanded {}

/// The bottom-right corner of the page area: `cards` (newest at the bottom) with `toast` (the
/// toast view, [`crate::toast::view`]) under them. Mount it once as the last child of the page
/// area (a `relative()` container). Empty space lets clicks through to the page.
pub fn corner(cards: Vec<Card>, toast: Option<AnyView>, cx: &mut App) -> AnyElement {
    let expanded = cx.try_global::<Expanded>().is_some_and(|e| e.0);
    let many = cards.len() > MAX_SHOWN;
    let orders: Vec<u64> = cards.iter().map(Card::order).collect();
    let (shown, hidden) = arrange(&orders, expanded);
    let mut slots: Vec<Option<Card>> = cards.into_iter().map(Some).collect();
    let list = div()
        .id("activity-list")
        .flex()
        .flex_col()
        .min_h_0()
        // Opened past the window's height, the stack scrolls instead of running off the top.
        .when(expanded, |d| d.overflow_y_scroll())
        .when(many, |d| d.child(more_row(hidden, expanded)))
        .children(shown.into_iter().filter_map(|i| slots[i].take()).map(card));
    div()
        .absolute()
        .top(px(MARGIN))
        .bottom(px(MARGIN))
        .right(px(MARGIN))
        .w(px(WIDTH))
        .flex()
        .flex_col()
        .justify_end()
        .child(list)
        .children(toast)
        .into_any_element()
}

/// The opaque floating surface every card (and the "+N more" row) sits on.
fn surface(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id).mt(px(GAP)).flex_none().rounded(px(radius::LG)).bg(Tokens::surface_2()).border_1().border_color(Tokens::border_strong()).shadow_lg().occlude()
}

fn more_row(hidden: usize, expanded: bool) -> AnyElement {
    let label = if expanded { "Show fewer".to_string() } else { format!("{hidden} more") };
    surface("activity-more")
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .text_size(px(size::SM))
        .font_weight(FontWeight::MEDIUM)
        .text_color(Tokens::text_2())
        .role(Role::Button)
        // Own hover and press colours: the kit's state layers are translucent.
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::surface_3()))
        .active(|s| s.bg(Tokens::surface()))
        .focus_ring()
        .child(ph_icon(if expanded { ph::CARET_DOWN } else { ph::CARET_UP }, Weight::Bold, Tokens::text_2(), icon_px::XS))
        .child(label)
        .on_click(move |_, _, cx| {
            cx.set_global(Expanded(!expanded));
            cx.refresh_windows();
        })
        .into_any_element()
}

fn card(c: Card) -> AnyElement {
    let lead = match c.lead {
        Lead::Progress(p) => progress_ring_sized(p, icon_px::LG),
        Lead::Done => ph_icon(ph::CHECK_CIRCLE, Weight::Fill, Tokens::success(), icon_px::LG).into_any_element(),
        Lead::Failed => ph_icon(ph::WARNING, Weight::Fill, Tokens::warning(), icon_px::LG).into_any_element(),
    };
    let footer = c.count.is_some() || !c.actions.is_empty();
    let el = surface(c.key.clone())
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .role(Role::Status)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(div().mt(px(1.)).flex_none().child(lead))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(c.title))
                        .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_3()).child(c.line)),
                )
                .when_some(c.on_close, |d, f| d.child(icon_button("activity-close", ph::X, "Hide").small().on_click(move |_, w, cx| f(w, cx)))),
        )
        .when_some(c.progress, |el, (p, moving)| el.child(progress_bar("activity-bar", p, moving)))
        .when(footer, |el| {
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(size::SM))
                    .text_color(Tokens::text_2())
                    .child(div().flex_1().min_w_0().truncate().children(c.count))
                    .children(c.actions),
            )
        });
    if reduced() {
        return el.into_any_element();
    }
    el.with_animation(SharedString::from(format!("{}-in", c.key)), Animation::new(duration::SLOW).with_easing(easing::enter), |el, t| el.opacity(t).mr(px((1. - t) * -(WIDTH + MARGIN))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of `super` would pull in GPUI's `test` macro over Rust's.
    use super::{arrange, next_order, MAX_SHOWN};

    #[test]
    fn oldest_first_newest_next_to_the_corner() {
        // Producers hand cards over in any order; the stack sorts by age.
        let (shown, hidden) = arrange(&[30, 10, 20], false);
        assert_eq!(shown, vec![1, 2, 0]);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn older_cards_fold_into_more() {
        let orders: Vec<u64> = (1..=7).collect();
        let (shown, hidden) = arrange(&orders, false);
        assert_eq!(hidden, 7 - MAX_SHOWN);
        // The newest MAX_SHOWN stay, still oldest first.
        assert_eq!(shown, (7 - MAX_SHOWN..7).collect::<Vec<_>>());
        // Exactly MAX_SHOWN fit without a fold.
        assert_eq!(arrange(&orders[..MAX_SHOWN], false).1, 0);
    }

    #[test]
    fn expanded_shows_everything() {
        let orders: Vec<u64> = (1..=7).collect();
        let (shown, hidden) = arrange(&orders, true);
        assert_eq!(hidden, 0);
        assert_eq!(shown.len(), 7);
        assert_eq!(arrange(&[], true), (vec![], 0));
    }

    #[test]
    fn orders_increase() {
        let a = next_order();
        assert!(next_order() > a);
    }
}
