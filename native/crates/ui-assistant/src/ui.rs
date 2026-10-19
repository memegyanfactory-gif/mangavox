//! Small UI helpers shared by the dock, providers panel and model hub. Controls, menus, tags and
//! popups come from the mv-ui kit; what is here is specific to AI providers and chats.
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;
use mv_ai::{AiProviderId, AiProviderState};
use mv_ui::{icon_tag, ph, ph_icon, Hue, Ph, Tokens, Tone, Weight};

pub use mv_ui::muted;

/// Official brand logo for a provider (`brand/*.svg`, served by `mv_ui::Assets`). Marks that
/// have colours of their own render as images; single-colour marks are tinted. llama.cpp has no
/// mark in `brand/` yet, so it gets a CPU glyph instead of sharing Ollama's llama (two identical
/// logos side by side in the picker rail could not be told apart).
pub fn provider_logo(id: AiProviderId, size: f32) -> AnyElement {
    enum Mark {
        Color(&'static str),
        Mono(&'static str, Rgba),
        Glyph(Ph),
    }
    let mono = Tokens::text_1();
    let mark = match id {
        AiProviderId::Anthropic => Mark::Color("brand/claude.svg"),
        AiProviderId::ClaudeCode => Mark::Color("brand/claudecode.svg"),
        AiProviderId::Codex => Mark::Color("brand/codex.svg"),
        AiProviderId::Openai => Mark::Mono("brand/openai.svg", mono),
        AiProviderId::Gemini => Mark::Color("brand/gemini.svg"),
        AiProviderId::GeminiCli => Mark::Color("brand/geminicli.svg"),
        AiProviderId::Antigravity => Mark::Color("brand/antigravity.svg"),
        AiProviderId::Opencode => Mark::Mono("brand/opencode.svg", mono),
        AiProviderId::GrokCli => Mark::Mono("brand/grok.svg", mono),
        AiProviderId::Xai => Mark::Mono("brand/xai.svg", mono),
        AiProviderId::Ollama => Mark::Mono("brand/ollama.svg", mono),
        AiProviderId::Llamacpp => Mark::Glyph(ph::CPU),
        AiProviderId::Lmstudio => Mark::Mono("brand/lmstudio.svg", mono),
        AiProviderId::Openrouter => Mark::Color("brand/openrouter.svg"),
        AiProviderId::Groq => Mark::Mono("brand/groq.svg", Tokens::hue(Hue::Orange)),
        AiProviderId::Deepseek => Mark::Color("brand/deepseek.svg"),
        AiProviderId::Mistral => Mark::Color("brand/mistral.svg"),
        AiProviderId::Custom => Mark::Mono("brand/generic.svg", Tokens::text_3()),
    };
    match mark {
        Mark::Color(path) => img(path).size(px(size)).flex_none().into_any_element(),
        Mark::Mono(path, color) => svg().path(path).size(px(size)).flex_none().text_color(color).into_any_element(),
        Mark::Glyph(g) => ph_icon(g, if size <= 14. { Weight::Bold } else { Weight::Regular }, mono, size).into_any_element(),
    }
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

/// Status pill: an icon and a word in a tonal fill, so the state never rests on colour alone
/// (WCAG 1.4.1).
pub fn status_pill(s: AiProviderState) -> Div {
    let (icon, tone) = match s {
        AiProviderState::Ready => (ph::CHECK_CIRCLE, Tone::Success),
        AiProviderState::NeedsKey => (ph::KEY, Tone::Warning),
        AiProviderState::NotInstalled => (ph::DOWNLOAD_SIMPLE, Tone::Neutral),
        AiProviderState::Offline => (ph::WIFI_SLASH, Tone::Neutral),
        AiProviderState::Disabled => (ph::PROHIBIT, Tone::Neutral),
        AiProviderState::Error => (ph::WARNING_CIRCLE, Tone::Danger),
    };
    icon_tag(icon, state_text(s), tone)
}

/// "Ollama isn't running" style title for a provider that is not ready.
pub fn not_ready_title(label: &str, s: AiProviderState) -> String {
    match s {
        AiProviderState::Ready => format!("{label} is ready"),
        AiProviderState::NeedsKey => format!("{label} needs an API key"),
        AiProviderState::NotInstalled => format!("{label} isn't installed"),
        AiProviderState::Offline => format!("{label} isn't running"),
        AiProviderState::Disabled => format!("{label} is turned off"),
        AiProviderState::Error => format!("{label} reported a problem"),
    }
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

/// Leading magnifying glass for search fields.
pub fn search_prefix() -> Svg {
    ph_icon(ph::MAGNIFYING_GLASS, Weight::Bold, Tokens::text_3(), mv_ui::theme::icon::SM)
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

/// "3m" style age from epoch millis, for chat rows.
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

/// Chat-history group for a timestamp, by local calendar day (Claude and ChatGPT group the same way).
pub fn date_group(ms: u64) -> &'static str {
    use chrono::{Local, TimeZone};
    let today = Local::now().date_naive();
    let day = Local.timestamp_millis_opt(ms as i64).single().map(|d| d.date_naive()).unwrap_or(today);
    day_bucket((today - day).num_days())
}

/// Group name for a chat last used `days` calendar days ago.
pub fn day_bucket(days: i64) -> &'static str {
    match days {
        i64::MIN..=0 => "Today",
        1 => "Yesterday",
        2..=6 => "Previous 7 days",
        7..=29 => "Previous 30 days",
        _ => "Older",
    }
}

/// "12s", "1m 5s" for a duration in milliseconds (never "0s").
pub fn fmt_secs(ms: u64) -> String {
    let s = ((ms + 500) / 1000).max(1);
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {}s", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn formatting() {
        assert_eq!(fmt_count(1_500_000), "1.5M");
        assert_eq!(fmt_count(12_400), "12K");
        assert_eq!(fmt_count(12), "12");
        assert_eq!(fmt_secs(0), "1s");
        assert_eq!(fmt_secs(12_400), "12s");
        assert_eq!(fmt_secs(65_000), "1m 5s");
    }

    #[::core::prelude::v1::test]
    fn chat_groups() {
        assert_eq!(day_bucket(-1), "Today");
        assert_eq!(day_bucket(0), "Today");
        assert_eq!(day_bucket(1), "Yesterday");
        assert_eq!(day_bucket(6), "Previous 7 days");
        assert_eq!(day_bucket(29), "Previous 30 days");
        assert_eq!(day_bucket(400), "Older");
        assert_eq!(date_group(crate::store::now_ms()), "Today");
    }

    #[::core::prelude::v1::test]
    fn not_ready_titles() {
        assert_eq!(not_ready_title("Ollama", AiProviderState::Offline), "Ollama isn't running");
        assert_eq!(not_ready_title("OpenAI", AiProviderState::NeedsKey), "OpenAI needs an API key");
    }
}
