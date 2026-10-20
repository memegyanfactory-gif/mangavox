//! App-wide toast (snackbar): brief, non-blocking feedback with at most one action, one at a
//! time, at the bottom centre of the page area.
//!
//! ```ignore
//! mv_ui::toast::show(cx, Toast::success("Chapter deleted").action("Undo", |window, cx| restore(window, cx)));
//! ```
//!
//! - Auto-dismisses after 4 s, or 6 s when it has an action or is a warning or error; hovering
//!   pauses the timer (WCAG 2.2.1 Timing Adjustable).
//! - A new toast waits for the current one, which then leaves after at most 1.5 s more. The same
//!   message twice restarts the timer instead of stacking.
//! - Never takes focus. The shell renders [`host`] once over the page.
//!
//! Material 3 snackbar: <https://m3.material.io/components/snackbar/guidelines>;
//! Fluent 2 toast: <https://fluent2.microsoft.design/components/web/react/core/toast/usage>
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::controls::{btn, icon_button, ButtonKind, WindowHandler};
use crate::feedback::AlertKind;
use crate::icons::{Ph, Weight};
use crate::interact::{glyph, IconSrc};
use crate::motion::reduced;
use crate::theme::{duration, easing, icon as icon_px, radius, size, Tokens};

/// Severity of a toast; picks its icon and colour.
pub type ToastKind = AlertKind;

/// How long a plain toast stays.
pub const SHORT: Duration = Duration::from_secs(4);
/// How long a toast with an action, a warning or an error stays.
pub const LONG: Duration = Duration::from_secs(6);
/// Once another toast is waiting, the visible one leaves after showing at least this long.
pub const MIN_SHOW: Duration = Duration::from_millis(1500);
/// After the pointer leaves a paused toast it stays at least this long.
const RESUME: Duration = Duration::from_millis(1500);
/// Waiting toasts beyond this drop the oldest.
const MAX_PENDING: usize = 3;

/// One message. Build with [`Toast::info`], [`Toast::success`], [`Toast::warning`] or
/// [`Toast::error`] and show with [`show`].
#[derive(Clone)]
pub struct Toast {
    kind: ToastKind,
    message: SharedString,
    icon: Option<Ph>,
    action: Option<(SharedString, WindowHandler)>,
    duration: Option<Duration>,
    sticky: bool,
}

impl Toast {
    pub fn new(kind: ToastKind, message: impl Into<SharedString>) -> Self {
        Toast { kind, message: message.into(), icon: None, action: None, duration: None, sticky: false }
    }

    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::new(AlertKind::Info, message)
    }

    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new(AlertKind::Success, message)
    }

    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self::new(AlertKind::Warning, message)
    }

    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::new(AlertKind::Error, message)
    }

    /// One text action ("Undo", "Retry", "View"). The toast closes when it is clicked.
    pub fn action(mut self, label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(handler)));
        self
    }

    /// Replaces the severity icon.
    pub fn icon(mut self, icon: Ph) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Overrides the 4 s / 6 s default.
    pub fn duration(mut self, d: Duration) -> Self {
        self.duration = Some(d);
        self
    }

    /// Stays until dismissed (or until a newer toast replaces it).
    pub fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    pub fn kind(&self) -> ToastKind {
        self.kind
    }

    pub fn message(&self) -> &SharedString {
        &self.message
    }

    /// How long it stays on its own; `None` for sticky.
    pub fn timeout(&self) -> Option<Duration> {
        if self.sticky {
            return None;
        }
        Some(self.duration.unwrap_or(if self.action.is_some() || matches!(self.kind, AlertKind::Warning | AlertKind::Error) { LONG } else { SHORT }))
    }

    fn same_as(&self, other: &Toast) -> bool {
        self.kind == other.kind && self.message == other.message
    }
}

/// The toast on screen.
pub(crate) struct Shown {
    pub id: u64,
    pub toast: Toast,
    shown_at: Instant,
    deadline: Option<Instant>,
    /// Time left while the pointer rests on it.
    paused: Option<Duration>,
}

/// One-at-a-time queue with timing; pure logic so it can be tested without a window.
#[derive(Default)]
pub(crate) struct ToastQueue {
    pub current: Option<Shown>,
    pending: VecDeque<Toast>,
    next_id: u64,
}

impl ToastQueue {
    pub fn push(&mut self, toast: Toast, now: Instant) {
        if let Some(cur) = self.current.as_mut().filter(|c| c.toast.same_as(&toast)) {
            cur.toast = toast;
            cur.shown_at = now;
            let left = cur.toast.timeout();
            if cur.paused.is_some() {
                cur.paused = left;
            } else {
                cur.deadline = left.map(|d| now + d);
            }
            return;
        }
        if self.pending.iter().any(|p| p.same_as(&toast)) {
            return;
        }
        if self.current.is_none() {
            self.show(toast, now);
            return;
        }
        self.pending.push_back(toast);
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
        self.hurry(now);
    }

    /// Expires the current toast if its time is up. True when something changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.current.as_ref().and_then(|c| c.deadline) {
            Some(at) if at <= now => {
                self.advance(now);
                true
            }
            _ => false,
        }
    }

    /// Closes toast `id` (the close button or its action). True when it was showing.
    pub fn dismiss(&mut self, id: u64, now: Instant) -> bool {
        if self.current.as_ref().is_some_and(|c| c.id == id) {
            self.advance(now);
            true
        } else {
            false
        }
    }

    /// Pauses the timer while hovered; resumes with what was left (at least [`RESUME`]).
    pub fn hover(&mut self, id: u64, hovered: bool, now: Instant) {
        let waiting = !self.pending.is_empty();
        let Some(cur) = self.current.as_mut().filter(|c| c.id == id) else { return };
        if hovered {
            if cur.paused.is_none() {
                cur.paused = Some(cur.deadline.map_or(Duration::MAX, |d| d.saturating_duration_since(now)));
                cur.deadline = None;
            }
        } else if let Some(left) = cur.paused.take() {
            // A sticky toast (MAX) stays sticky unless something is waiting behind it.
            let left = if waiting { left.min(MIN_SHOW) } else { left };
            cur.deadline = (left != Duration::MAX).then(|| now + left.max(RESUME));
        }
    }

    /// When [`ToastQueue::tick`] next has work.
    pub fn next_wake(&self) -> Option<Instant> {
        self.current.as_ref().and_then(|c| c.deadline)
    }

    #[cfg(test)]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn show(&mut self, toast: Toast, now: Instant) {
        self.next_id += 1;
        let deadline = toast.timeout().map(|d| now + d);
        self.current = Some(Shown { id: self.next_id, toast, shown_at: now, deadline, paused: None });
        if !self.pending.is_empty() {
            self.hurry(now);
        }
    }

    fn advance(&mut self, now: Instant) {
        self.current = None;
        if let Some(next) = self.pending.pop_front() {
            self.show(next, now);
        }
    }

    /// Something is waiting: the current toast leaves once it has been up for [`MIN_SHOW`].
    fn hurry(&mut self, now: Instant) {
        if let Some(cur) = self.current.as_mut().filter(|c| c.paused.is_none()) {
            let soon = (cur.shown_at + MIN_SHOW).max(now);
            cur.deadline = Some(cur.deadline.map_or(soon, |d| d.min(soon)));
        }
    }
}

/// The view the shell mounts once; renders the current toast.
pub struct ToastHost {
    queue: ToastQueue,
    timer: Option<Task<()>>,
}

impl ToastHost {
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.schedule(cx);
        cx.notify();
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.timer = self.queue.next_wake().map(|at| {
            let wait = at.saturating_duration_since(Instant::now());
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |h, cx| {
                    if h.queue.tick(Instant::now()) {
                        cx.notify();
                    }
                    h.schedule(cx);
                });
            })
        });
    }
}

struct Center(Entity<ToastHost>);
impl Global for Center {}

fn center(cx: &mut App) -> Entity<ToastHost> {
    if let Some(c) = cx.try_global::<Center>() {
        return c.0.clone();
    }
    let host = cx.new(|_| ToastHost { queue: ToastQueue::default(), timer: None });
    cx.set_global(Center(host.clone()));
    host
}

/// Shows `toast` (or queues it behind the current one).
pub fn show(cx: &mut App, toast: Toast) {
    center(cx).update(cx, |h, cx| {
        h.queue.push(toast, Instant::now());
        h.changed(cx);
    });
}

/// Closes the toast on screen, if any.
pub fn dismiss(cx: &mut App) {
    center(cx).update(cx, |h, cx| {
        if let Some(id) = h.queue.current.as_ref().map(|c| c.id) {
            h.queue.dismiss(id, Instant::now());
            h.changed(cx);
        }
    });
}

/// The toast layer. Mount it once as the last child of the page area (a `relative()` container):
/// `.child(mv_ui::toast::host(cx))`.
pub fn host(cx: &mut App) -> AnyView {
    center(cx).into()
}

impl Render for ToastHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(cur) = self.queue.current.as_ref() else {
            return div().into_any_element();
        };
        let id = cur.id;
        let t = cur.toast.clone();
        let (fg, _, _, default_icon) = t.kind.colors();
        let this = cx.entity().downgrade();
        let close = this.clone();
        let card = div()
            .id(ElementId::NamedInteger("mv-toast".into(), id))
            .relative()
            .occlude()
            .flex()
            .items_center()
            .gap(px(12.))
            .min_w(px(320.))
            .max_w(px(560.))
            .min_h(px(48.))
            .pl(px(14.))
            .pr(px(6.))
            .py(px(6.))
            .rounded(px(radius::LG))
            .bg(Tokens::glass_strong())
            .border_1()
            .border_color(Tokens::glass_border())
            .shadow_lg()
            .role(if t.kind == AlertKind::Error { Role::Alert } else { Role::Status })
            .on_hover(cx.listener(move |h, hovered: &bool, _, cx| {
                h.queue.hover(id, *hovered, Instant::now());
                h.schedule(cx);
            }))
            .child(glyph(IconSrc::Ph(t.icon.unwrap_or(default_icon)), Weight::Fill, fg, icon_px::LG))
            .child(div().flex_1().min_w_0().py(px(6.)).text_size(px(size::MD)).line_height(px(20.)).text_color(Tokens::text_1()).child(t.message.clone()))
            .when_some(t.action.clone(), |d, (label, f)| {
                d.child(btn("mv-toast-action", label, ButtonKind::Ghost).small().text_color(Tokens::accent()).on_click(move |_, w, cx| {
                    // Close first: the action may show another toast.
                    let _ = this.update(cx, |h, cx| {
                        h.queue.dismiss(id, Instant::now());
                        h.changed(cx);
                    });
                    f(w, cx);
                }))
            })
            .child(icon_button("mv-toast-close", crate::icons::ph::X, "Dismiss").small().on_click(move |_, _, cx| {
                let _ = close.update(cx, |h, cx| {
                    h.queue.dismiss(id, Instant::now());
                    h.changed(cx);
                });
            }));
        let card = if reduced() {
            card.into_any_element()
        } else {
            card.with_animation(ElementId::NamedInteger("mv-toast-in".into(), id), Animation::new(duration::BASE).with_easing(easing::enter), |d, t| d.opacity(t).top(px((1. - t) * 12.))).into_any_element()
        };
        div().absolute().left_0().right_0().bottom(px(24.)).px(px(16.)).flex().justify_center().child(card).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: a glob of `super` would pull in GPUI's `test` macro over Rust's.
    use super::{Toast, ToastQueue, LONG, MAX_PENDING, MIN_SHOW, SHORT};
    use std::time::{Duration, Instant};

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn timeouts_follow_kind_and_action() {
        assert_eq!(Toast::success("Saved").timeout(), Some(SHORT));
        assert_eq!(Toast::error("Failed").timeout(), Some(LONG));
        assert_eq!(Toast::warning("Low space").timeout(), Some(LONG));
        assert_eq!(Toast::info("Deleted").action("Undo", |_, _| {}).timeout(), Some(LONG));
        assert_eq!(Toast::info("Hi").duration(Duration::from_secs(9)).timeout(), Some(Duration::from_secs(9)));
        assert_eq!(Toast::info("Offline").sticky().timeout(), None);
    }

    #[test]
    fn shows_one_and_expires() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::success("Saved"), now);
        assert_eq!(q.current.as_ref().map(|c| c.toast.message().as_ref()), Some("Saved"));
        assert!(!q.tick(now + Duration::from_secs(3)));
        assert!(q.tick(now + SHORT));
        assert!(q.current.is_none());
        assert_eq!(q.next_wake(), None);
    }

    #[test]
    fn queues_and_hurries_the_current_one() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::success("One"), now);
        q.push(Toast::success("Two"), now + Duration::from_millis(200));
        assert_eq!(q.pending_len(), 1);
        // "One" leaves once it has been up MIN_SHOW, not after its full 4 s.
        assert_eq!(q.next_wake(), Some(now + MIN_SHOW));
        assert!(q.tick(now + MIN_SHOW));
        assert_eq!(q.current.as_ref().map(|c| c.toast.message().as_ref()), Some("Two"));
        assert_eq!(q.next_wake(), Some(now + MIN_SHOW + SHORT));
    }

    #[test]
    fn duplicate_restarts_instead_of_stacking() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::success("Saved"), now);
        let first = q.current.as_ref().unwrap().id;
        q.push(Toast::success("Saved"), now + Duration::from_secs(3));
        assert_eq!(q.pending_len(), 0);
        assert_eq!(q.current.as_ref().unwrap().id, first);
        assert_eq!(q.next_wake(), Some(now + Duration::from_secs(3) + SHORT));
        // A duplicate of something waiting is dropped too.
        q.push(Toast::info("Next"), now);
        q.push(Toast::info("Next"), now);
        assert_eq!(q.pending_len(), 1);
    }

    #[test]
    fn pending_is_capped() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::info("current"), now);
        for i in 0..6 {
            q.push(Toast::info(format!("m{i}")), now);
        }
        assert_eq!(q.pending_len(), MAX_PENDING);
    }

    #[test]
    fn hover_pauses_and_resumes() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::success("Saved"), now);
        let id = q.current.as_ref().unwrap().id;
        q.hover(id, true, now + Duration::from_secs(1));
        assert_eq!(q.next_wake(), None);
        assert!(!q.tick(now + Duration::from_secs(60)));
        q.hover(id, false, now + Duration::from_secs(60));
        // Three seconds were left when the pointer arrived.
        assert_eq!(q.next_wake(), Some(now + Duration::from_secs(63)));
    }

    #[test]
    fn sticky_stays_until_dismissed_or_replaced() {
        let now = t0();
        let mut q = ToastQueue::default();
        q.push(Toast::warning("Offline").sticky(), now);
        let id = q.current.as_ref().unwrap().id;
        assert_eq!(q.next_wake(), None);
        assert!(!q.dismiss(id + 1, now));
        assert!(q.dismiss(id, now));
        assert!(q.current.is_none());
        q.push(Toast::warning("Offline").sticky(), now);
        q.push(Toast::success("Back online"), now + Duration::from_secs(5));
        assert_eq!(q.next_wake(), Some(now + Duration::from_secs(5)));
    }
}
