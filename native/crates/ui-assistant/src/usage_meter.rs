//! Usage ring: how much of the current limit window is used, drawn as a ring next to the
//! percentage. Clicking opens a card with every limit (session, weekly), their reset countdowns
//! and this app session's tokens. Data comes from `mv_ai::usage`.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::usage::{self, ProviderUsage, UsageWindow};
use mv_ai::AiProviderId;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::{appear, bg, ph, ph_icon, popup_surface, size, tip, usage_bar, Interactive as _, Tokens, Weight};

use crate::shared::{shared, Shared};
use crate::ui::provider_logo;

const POLL: Duration = Duration::from_secs(5);

pub struct UsageMeter {
    shared: Entity<Shared>,
    usage: Option<ProviderUsage>,
    open: bool,
    /// When an outside click closed the card, so the same click on the ring doesn't reopen it.
    closed_at: Option<Instant>,
    /// The open card takes the keyboard so Esc closes it; the previous focus comes back after.
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    _sub: Subscription,
    _poll: Task<()>,
}

pub fn create(_window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(UsageMeter::new).into()
}

impl UsageMeter {
    fn new(cx: &mut Context<Self>) -> Self {
        let shared = shared(cx);
        let sub = cx.observe(&shared, |_, _, cx| cx.notify());
        // `usage::fetch` caches for 60 s and is invalidated after every step, so a 5 s tick is cheap
        // and picks up a finished step quickly. Only a change re-renders.
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok(provider) = this.update(cx, |t, cx| t.shared.read(cx).store.prefs.provider) else { break };
            let data = bg::run(usage::fetch(provider)).await.ok().flatten();
            let calls = usage::session_totals(provider).2;
            if this
                .update(cx, |t, cx| {
                    if t.usage != data || t.open {
                        t.usage = data;
                        cx.notify();
                    } else if calls > 0 {
                        // Session token counts move with every step.
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        Self { shared, usage: None, open: false, closed_at: None, focus: cx.focus_handle(), restore: None, _sub: sub, _poll: poll }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        if self.open {
            self.close(window, cx);
            return;
        }
        self.open = true;
        self.restore = window.focused(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Closes the card; the keyboard goes back where it was if the card still had it.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        if let Some(prev) = self.restore.take().filter(|_| self.focus.is_focused(window)) {
            prev.focus(window, cx);
        }
        cx.notify();
    }

    fn card(&self, provider: AiProviderId, data: Option<&ProviderUsage>, cx: &mut Context<Self>) -> impl IntoElement {
        let label = mv_ai::provider_def(provider).label;
        let (input, output, calls) = usage::session_totals(provider);
        let windows: Vec<UsageWindow> = data.map(|u| u.windows.clone()).unwrap_or_default();
        let surface = popup_surface()
            .w(px(300.))
            .p(px(14.))
            .gap(px(14.))
            .track_focus(&self.focus)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                // Runs before the clicked element takes focus, so a click into a field still lands there.
                this.close(window, cx);
                this.closed_at = Some(Instant::now());
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ph_icon(ph::GAUGE, Weight::Regular, Tokens::text_2(), icon_px::MD))
                    .child(div().flex_1().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Usage"))
                    .child(provider_logo(provider, icon_px::SM))
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(label)),
            )
            .children(windows.iter().map(limit_block))
            .when(windows.is_empty(), |d| {
                d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{label} doesn't report plan limits. Token use below is counted by MangaVox.")))
            })
            .when_some(data.and_then(|u| u.plan.clone()), |d, p| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Plan: {p}"))))
            .when_some(data.and_then(|u| u.note.clone()), |d, n| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(n)))
            .child(div().h(px(1.)).bg(Tokens::border()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .child(div().flex_1().text_size(px(size::SM)).text_color(Tokens::text_1()).child("Since MangaVox opened"))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(format!("{} tokens", fmt_tokens(input + output)))),
                    )
                    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!(
                        "{} in · {} out · {calls} call{}",
                        fmt_tokens(input),
                        fmt_tokens(output),
                        if calls == 1 { "" } else { "s" }
                    ))),
            );
        appear("usage-card", surface)
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// "1 hr 57 min", "3 days 4 hr", "12 min".
fn fmt_reset(at: u64) -> String {
    let s = at.saturating_sub(now());
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d} day{} {h} hr", if d == 1 { "" } else { "s" })
    } else if h > 0 {
        format!("{h} hr {m} min")
    } else {
        format!("{} min", m.max(1))
    }
}

/// Brand accent while there is room, warning from 75% and danger from 90%.
fn usage_color(pct: f32) -> Rgba {
    if pct >= 90. {
        Tokens::danger()
    } else if pct >= 75. {
        Tokens::warning()
    } else {
        Tokens::accent()
    }
}

fn window_title(label: &str) -> String {
    match label {
        "5h" => "Session limit".into(),
        "Week" => "Weekly limit".into(),
        "Month" => "Monthly limit".into(),
        l => format!("{l} limit"),
    }
}

fn limit_block(w: &UsageWindow) -> Div {
    let pct = w.used_percent.clamp(0., 100.);
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .flex()
                .items_baseline()
                .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(window_title(&w.label)))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{}% used", pct.round() as u32))),
        )
        .child(usage_bar(&[(pct, usage_color(pct))], 100.))
        .when_some(w.resets_at, |d, at| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Resets in {}", fmt_reset(at)))))
}

/// A ring `pct` (0..=100) full, starting at 12 o'clock and running clockwise.
fn ring(pct: f32, diameter: f32, color: Rgba) -> impl IntoElement {
    let frac = (pct / 100.).clamp(0., 1.);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let stroke = 2.4_f32;
            let c = bounds.center();
            let r = diameter / 2. - stroke / 2.;
            let at = |a: f32| point(c.x + px(r * a.sin()), c.y - px(r * a.cos()));
            let arc = |from: f32, to: f32| {
                let mut b = PathBuilder::stroke(px(stroke));
                b.move_to(at(from));
                b.arc_to(point(px(r), px(r)), px(0.), to - from > std::f32::consts::PI, true, at(to));
                b.build().ok()
            };
            let tau = std::f32::consts::TAU;
            // Full circles are drawn as two halves: one arc can't end where it starts.
            for (from, to) in [(0., tau / 2.), (tau / 2., tau)] {
                if let Some(p) = arc(from, to) {
                    window.paint_path(p, Tokens::track());
                }
            }
            let end = frac * tau;
            if frac >= 0.999 {
                for (from, to) in [(0., tau / 2.), (tau / 2., tau)] {
                    if let Some(p) = arc(from, to) {
                        window.paint_path(p, color);
                    }
                }
            } else if frac > 0.004 {
                if let Some(p) = arc(0., end) {
                    window.paint_path(p, color);
                }
            }
        },
    )
    .size(px(diameter))
    .flex_none()
}

impl Render for UsageMeter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let provider: AiProviderId = self.shared.read(cx).store.prefs.provider;
        let (input, output, calls) = usage::session_totals(provider);
        let data = self.usage.as_ref().filter(|u| u.provider == provider && !u.windows.is_empty()).cloned();
        if data.is_none() && calls == 0 {
            return div().into_any_element();
        }
        // The ring follows the window closest to running out.
        let worst = data.as_ref().and_then(|u| u.windows.iter().max_by(|a, b| a.used_percent.total_cmp(&b.used_percent)).cloned());
        let tooltip = match &worst {
            Some(w) => format!("Usage limits · {}% of the {} used", w.used_percent.clamp(0., 100.).round() as u32, window_title(&w.label).to_lowercase()),
            None => "Usage this session".to_string(),
        };
        let trigger = div()
            .id("usage-meter")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(radius::MD))
            .role(Role::Button)
            .aria_expanded(self.open)
            .when(self.open, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()))
            .pressable()
            .when(!self.open, |d| d.tooltip(tip(tooltip)))
            .map(|d| match &worst {
                Some(w) => {
                    let pct = w.used_percent.clamp(0., 100.);
                    d.child(ring(pct, icon_px::MD, usage_color(pct))).child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{}%", pct.round() as u32)))
                }
                None => d.child(provider_logo(provider, icon_px::SM)).child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{} tokens", fmt_tokens(input + output)))),
            })
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle(window, cx)));
        // Fade in when data first arrives instead of popping into the header.
        let trigger = appear("usage-meter-in", div().flex_none().child(trigger));
        div()
            .relative()
            .flex_none()
            .child(trigger)
            .when(self.open, |d| {
                let card = self.card(provider, data.as_ref(), cx);
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .right_0()
                        .child(deferred(anchored().anchor(Anchor::TopRight).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(card)).with_priority(2)),
                )
            })
            .into_any_element()
    }
}
