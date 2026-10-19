//! Compact usage pill: provider logo, session tokens and (for Claude Code / Codex) the 5h / weekly
//! limit bars. Hover shows "% left" and the reset countdown. Data comes from `mv_ai::usage`.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::usage::{self, ProviderUsage, UsageWindow};
use mv_ai::AiProviderId;
use mv_ui::{bg, size, Tokens};

use crate::shared::{shared, Shared};
use crate::ui::provider_logo;

const POLL: Duration = Duration::from_secs(5);

pub struct UsageMeter {
    shared: Entity<Shared>,
    usage: Option<ProviderUsage>,
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
        // and picks up a finished step quickly.
        let poll = cx.spawn(async move |this, cx| loop {
            let Ok(provider) = this.update(cx, |t, cx| t.shared.read(cx).store.prefs.provider) else { break };
            let data = bg::run(usage::fetch(provider)).await.ok().flatten();
            if this.update(cx, |t, cx| {
                if t.usage != data {
                    t.usage = data;
                }
                cx.notify();
            })
            .is_err()
            {
                break;
            }
            cx.background_executor().timer(POLL).await;
        });
        Self { shared, usage: None, _sub: sub, _poll: poll }
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

fn fmt_reset(at: u64) -> String {
    let s = at.saturating_sub(now());
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{}m", m.max(1))
    }
}

fn bar_color(pct: f32) -> Rgba {
    if pct >= 90. {
        Tokens::danger()
    } else if pct >= 75. {
        Tokens::warning()
    } else {
        Tokens::accent()
    }
}

fn bar(w: &UsageWindow) -> Div {
    let pct = w.used_percent.clamp(0., 100.);
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} {}%", w.label, pct.round() as u32)))
        .child(div().w(px(34.)).h(px(4.)).rounded(px(2.)).bg(Tokens::border()).overflow_hidden().child(div().h_full().w(relative(pct / 100.)).rounded(px(2.)).bg(bar_color(pct))))
}

fn tooltip_text(label: &str, input: u64, output: u64, calls: u32, u: Option<&ProviderUsage>) -> String {
    let mut lines = vec![format!("{label} · this session: {} in / {} out · {calls} call{}", fmt_tokens(input), fmt_tokens(output), if calls == 1 { "" } else { "s" })];
    if let Some(u) = u {
        if let Some(p) = &u.plan {
            lines.push(format!("Plan: {p}"));
        }
        for w in &u.windows {
            let left = (100. - w.used_percent).clamp(0., 100.).round() as u32;
            lines.push(match w.resets_at {
                Some(at) => format!("{}: {left}% left · resets {}", w.label, fmt_reset(at)),
                None => format!("{}: {left}% left", w.label),
            });
        }
        if let Some(n) = &u.note {
            lines.push(n.clone());
        }
    }
    lines.join("\n")
}

impl Render for UsageMeter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let provider: AiProviderId = self.shared.read(cx).store.prefs.provider;
        let (input, output, calls) = usage::session_totals(provider);
        let data = self.usage.as_ref().filter(|u| u.provider == provider && !u.windows.is_empty());
        if data.is_none() && calls == 0 {
            return div().into_any_element();
        }
        let label = mv_ai::provider_def(provider).label;
        let tip = tooltip_text(label, input, output, calls, data);
        let mut row = div()
            .id("usage-meter")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(Tokens::border())
            .hover(|s| s.bg(Tokens::hover()))
            .child(provider_logo(provider, 13.))
            .when(calls > 0, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child(format!("{} tokens", fmt_tokens(input + output)))));
        for w in data.into_iter().flat_map(|u| u.windows.iter().take(2)) {
            row = row.child(bar(w));
        }
        row.tooltip(move |w, cx| Tooltip::new(tip.clone()).build(w, cx)).into_any_element()
    }
}
