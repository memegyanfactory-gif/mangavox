//! Small UI helpers shared by the dock, providers panel and model hub.
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{AiProviderId, AiProviderState};
use mv_ui::{icon, size, Tokens};

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::SM)).text_color(Tokens::text_3()).child(text.into())
}

/// Brand logo for a provider, tinted with its brand colour (`brand/*.svg`, served by `mv_ui::Assets`).
pub fn provider_logo(id: AiProviderId, size: f32) -> impl IntoElement {
    let (path, color): (&'static str, u32) = match id {
        AiProviderId::Anthropic | AiProviderId::ClaudeCode => ("brand/claude.svg", 0xD97757),
        AiProviderId::Openai | AiProviderId::Codex => ("brand/openai.svg", 0xE6E8EB),
        AiProviderId::Gemini | AiProviderId::GeminiCli => ("brand/gemini.svg", 0x8E75FF),
        AiProviderId::Opencode => ("brand/opencode.svg", 0xE6E8EB),
        AiProviderId::Ollama | AiProviderId::Llamacpp => ("brand/ollama.svg", 0xE6E8EB),
        AiProviderId::Lmstudio => ("brand/lmstudio.svg", 0x8B7CF6),
        AiProviderId::Openrouter => ("brand/openrouter.svg", 0x94A3B8),
        AiProviderId::Groq => ("brand/groq.svg", 0xF55036),
        AiProviderId::Deepseek => ("brand/deepseek.svg", 0x4D6BFE),
        AiProviderId::Mistral => ("brand/mistral.svg", 0xFA520F),
        AiProviderId::Xai => ("brand/xai.svg", 0xE6E8EB),
        AiProviderId::Custom => ("brand/generic.svg", 0x9AA3AE),
    };
    svg().path(path).size(px(size)).flex_none().text_color(rgb(color))
}

pub fn state_text(s: AiProviderState) -> &'static str {
    match s {
        AiProviderState::Ready => "Ready",
        AiProviderState::NeedsKey => "Needs key",
        AiProviderState::NotInstalled => "Not installed",
        AiProviderState::Offline => "Offline",
        AiProviderState::Disabled => "Disabled",
        AiProviderState::Error => "Error",
    }
}

pub fn state_color(s: AiProviderState) -> Rgba {
    match s {
        AiProviderState::Ready => Tokens::accent(),
        AiProviderState::NeedsKey => Tokens::warning(),
        AiProviderState::Error => Tokens::danger(),
        _ => Tokens::text_3(),
    }
}

/// Status pill: coloured dot + label.
pub fn status_pill(s: AiProviderState) -> Div {
    let c = state_color(s);
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .h(px(20.))
        .px(px(8.))
        .rounded(px(999.))
        .bg(rgba(0xffffff0d))
        .text_size(px(size::XS))
        .text_color(c)
        .child(div().size(px(6.)).rounded(px(3.)).bg(c))
        .child(state_text(s))
}

/// Small static label chip.
pub fn tag(text: impl Into<SharedString>, color: Rgba) -> Div {
    div().flex().items_center().h(px(18.)).px(px(7.)).rounded(px(6.)).bg(rgba(0xffffff0d)).text_size(px(size::XS)).text_color(color).child(text.into())
}

/// Single-line input state with a placeholder.
pub fn text_state(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let ph = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(ph))
}

pub fn password_state(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let ph = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(ph).masked(true))
}

pub fn value_of(state: &Entity<InputState>, cx: &App) -> String {
    state.read(cx).value().to_string()
}

pub fn set_value(state: &Entity<InputState>, value: &str, window: &mut Window, cx: &mut App) {
    let v = value.to_string();
    state.update(cx, |s, cx| s.set_value(v, window, cx));
}

/// A bordered input field.
pub fn field(state: &Entity<InputState>) -> Input {
    Input::new(state)
}

/// Popup menu row.
pub fn menu_row(id: impl Into<ElementId>, label: impl Into<SharedString>, hint: Option<&str>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(7.))
        .rounded(px(7.))
        .cursor_pointer()
        .hover(|s| s.bg(Tokens::hover()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(label.into()))
                .when_some(hint.map(String::from), |d, h| d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(h))),
        )
        .when(selected, |d| d.child(icon("icons/check.svg", Tokens::accent(), 14.)))
}

/// Floating panel surface used by popups.
pub fn popup_surface() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(rgba(0xffffff1f))
        .bg(Tokens::surface())
        .shadow_lg()
        .occlude()
}

pub fn fmt_bytes(n: u64) -> String {
    if n == 0 {
        return "—".into();
    }
    let gb = n as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{:.*} GB", if gb >= 10.0 { 0 } else { 1 }, gb)
    } else {
        format!("{} MB", (n as f64 / 1024f64.powi(2)).round() as u64)
    }
}

pub fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{}K", (n as f64 / 1e3).round() as u64)
    } else {
        n.to_string()
    }
}

/// "3m ago" style label from epoch millis.
pub fn ago_ms(ms: u64) -> String {
    let now = crate::store::now_ms();
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn formatting() {
        assert_eq!(fmt_bytes(0), "—");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(fmt_bytes(32 * 1024 * 1024 * 1024), "32 GB");
        assert_eq!(fmt_bytes(300 * 1024 * 1024), "300 MB");
        assert_eq!(fmt_count(1_500_000), "1.5M");
        assert_eq!(fmt_count(12_400), "12K");
        assert_eq!(fmt_count(12), "12");
    }
}
