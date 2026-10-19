//! T3-style model picker: a compact button (logo, model name, chevron) that opens a popover
//! with a provider rail, search, model rows with Ctrl+1..9 hints, favourites and a collapsible
//! "Legacy models" group. Shared by the dock composer and the Studio toolbar.
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{provider_def, AiModel, AiProviderId, AiProviderSettings, AiProviderState};
use mv_ui::{icon, size, Route, Tokens};

use crate::shared::{shared, Shared};
use crate::store::model_key;
use crate::ui::{field, muted, popup_surface, provider_logo, set_value, text_state, value_of};

#[derive(Clone)]
struct Row {
    p: AiProviderId,
    m: AiModel,
    fav: bool,
}

pub struct ModelPicker {
    shared: Entity<Shared>,
    search: Entity<InputState>,
    custom_model: Entity<InputState>,
    open: bool,
    /// `None` shows favourites across all providers.
    provider: Option<AiProviderId>,
    legacy_open: bool,
    /// Popover opens above the button (composer) instead of below (toolbar).
    above: bool,
    _subs: Vec<Subscription>,
}

pub fn create(above: bool, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| ModelPicker::new(above, window, cx)).into()
}

impl ModelPicker {
    fn new(above: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = shared(cx);
        let search = text_state("Search models...", window, cx);
        let custom_model = text_state("Custom model id", window, cx);
        let subs = vec![cx.observe(&search, |_, _, cx| cx.notify()), cx.observe(&shared, |_, _, cx| cx.notify())];
        shared.update(cx, |s, cx| s.refresh_providers(false, cx));
        Self { shared, search, custom_model, open: false, provider: None, legacy_open: false, above, _subs: subs }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
            return;
        }
        let p = self.shared.read(cx).store.prefs.provider;
        self.open = true;
        self.provider = Some(p);
        self.legacy_open = false;
        set_value(&self.search, "", window, cx);
        self.search.update(cx, |s, cx| s.focus(window, cx));
        self.shared.update(cx, |s, cx| {
            s.load_models(p, false, cx);
            s.refresh_providers(false, cx);
        });
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    fn pick(&mut self, p: AiProviderId, model: String, cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| s.use_in_assistant(p, &model, cx));
        self.close(cx);
    }

    fn add_custom_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = value_of(&self.custom_model, cx).trim().to_string();
        let provider = self.provider.unwrap_or(self.shared.read(cx).store.prefs.provider);
        if id.is_empty() {
            return;
        }
        let reg = self.shared.read(cx).registry.clone();
        let mut extra = reg.read_settings(provider).extra_models.unwrap_or_default();
        if !extra.contains(&id) {
            extra.push(id.clone());
        }
        let _ = reg.update_provider_settings(provider, &AiProviderSettings { extra_models: Some(extra), ..Default::default() });
        set_value(&self.custom_model, "", window, cx);
        self.shared.update(cx, |s, cx| {
            s.models.remove(&provider);
            s.load_models(provider, true, cx);
        });
        self.pick(provider, id, cx);
    }

    /// (main rows, legacy rows) for the current rail selection and search text.
    fn rows(&self, cx: &App) -> (Vec<Row>, Vec<Row>) {
        let q = value_of(&self.search, cx).to_lowercase();
        let s = self.shared.read(cx);
        let prefs = &s.store.prefs;
        let mut all: Vec<Row> = vec![];
        match self.provider {
            Some(p) => all.extend(s.models.get(&p).into_iter().flatten().map(|m| Row { p, m: m.clone(), fav: prefs.favourites.contains(&model_key(p, &m.id)) })),
            None => {
                for key in &prefs.favourites {
                    let Some((pid, mid)) = key.split_once("::") else { continue };
                    let Some(p) = mv_ai::PROVIDERS.iter().find(|d| d.id.as_str() == pid).map(|d| d.id) else { continue };
                    let m = s.models.get(&p).and_then(|l| l.iter().find(|m| m.id == mid).cloned()).unwrap_or_else(|| AiModel::new(p, mid, mid));
                    all.push(Row { p, m, fav: true });
                }
            }
        }
        all.retain(|r| q.is_empty() || r.m.id.to_lowercase().contains(&q) || r.m.label.to_lowercase().contains(&q) || pretty_model(r.p, &r.m).to_lowercase().contains(&q));
        // Favourites float to the top (stable).
        all.sort_by_key(|r| !r.fav);
        let (legacy, main): (Vec<Row>, Vec<Row>) = all.into_iter().partition(|r| is_legacy(r.p, &r.m));
        (main, legacy)
    }

    /// Rows in display order (what Ctrl+1..9 index into).
    fn ordered(&self, cx: &App) -> Vec<Row> {
        let (mut main, legacy) = self.rows(cx);
        if self.legacy_open || !value_of(&self.search, cx).is_empty() {
            main.extend(legacy);
        }
        main
    }

    fn rail_providers(&self, cx: &App) -> Vec<AiProviderId> {
        let s = self.shared.read(cx);
        let current = s.store.prefs.provider;
        let usable: Vec<AiProviderId> = mv_ai::PROVIDERS
            .iter()
            .map(|d| d.id)
            .filter(|id| *id == current || s.models.get(id).map(|m| !m.is_empty()).unwrap_or(false) || s.status(*id).map(|st| st.state == AiProviderState::Ready).unwrap_or(false))
            .collect();
        if !s.providers_loaded || usable.is_empty() {
            mv_ai::PROVIDERS.iter().map(|d| d.id).collect()
        } else {
            usable
        }
    }

    fn button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (provider, label) = {
            let s = self.shared.read(cx);
            (s.store.prefs.provider, current_label(s))
        };
        div()
            .id("model-btn")
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if self.open { Tokens::text_1() } else { Tokens::text_2() })
            .when(self.open, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .child(provider_logo(provider, 14.))
            .child(div().max_w(px(96.)).truncate().child(label))
            .child(icon("icons/chevron-down.svg", Tokens::text_3(), 12.))
            .on_click(cx.listener(|this, _, w, cx| this.toggle(w, cx)))
    }

    fn popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (current_p, current_m) = {
            let s = self.shared.read(cx);
            (s.store.prefs.provider, s.store.prefs.model.clone())
        };
        let sel = self.provider;
        let searching = !value_of(&self.search, cx).is_empty();
        let (main, legacy) = self.rows(cx);
        let (loading, status) = {
            let s = self.shared.read(cx);
            (sel.map(|p| s.models_loading.contains(&p)).unwrap_or(false), sel.and_then(|p| s.status(p).cloned()))
        };
        let legacy_shown = self.legacy_open || searching;

        // Rail: favourites, then a logo per usable provider. Active item gets an accent bar.
        let rail_item = |id: &'static str, active: bool, child: AnyElement| {
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .w_full()
                .h(px(38.))
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .when(active, |d| d.bg(Tokens::hover()).child(div().absolute().left_0().top(px(10.)).w(px(2.)).h(px(18.)).rounded_r(px(2.)).bg(Tokens::accent())))
                .child(child)
        };
        let mut rail = div().id("picker-rail").flex().flex_col().w(px(46.)).flex_none().py(px(4.)).overflow_y_scroll().border_r_1().border_color(Tokens::border());
        rail = rail.child(
            rail_item("rail-fav", sel.is_none(), svg().path("brand/star.svg").size(px(16.)).text_color(if sel.is_none() { Tokens::text_1() } else { Tokens::text_3() }).into_any_element()).on_click(cx.listener(|this, _, _, cx| {
                this.provider = None;
                cx.notify();
            })),
        );
        rail = rail.child(div().mx(px(10.)).my(px(3.)).h(px(1.)).bg(Tokens::border()));
        for id in self.rail_providers(cx) {
            let key: &'static str = id.as_str();
            rail = rail.child(rail_item(key, sel == Some(id), provider_logo(id, 18.).into_any_element()).on_click(cx.listener(move |this, _, _, cx| {
                this.provider = Some(id);
                this.shared.update(cx, |s, cx| s.load_models(id, false, cx));
                cx.notify();
            })));
        }

        let mut n = 0usize;
        let render_row = |r: Row, n: &mut usize, cx: &mut Context<Self>| {
            let idx = *n;
            *n += 1;
            let current = current_p == r.p && current_m == r.m.id;
            let (p, mid, fav) = (r.p, r.m.id.clone(), r.fav);
            let (pick_id, fav_id) = (mid.clone(), mid);
            div()
                .id(("model-row", idx))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .h(px(48.))
                .flex_none()
                .rounded(px(8.))
                .cursor_pointer()
                .when(current, |d| d.bg(Tokens::active()))
                .hover(|s| s.bg(Tokens::hover()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(pretty_model(p, &r.m)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .text_size(px(size::XS))
                                .text_color(Tokens::text_3())
                                .child(provider_logo(p, 11.))
                                .child(provider_def(p).label)
                                .when_some(r.m.size_bytes.filter(|b| *b > 0), |d, b| d.child(format!("· {}", crate::ui::fmt_bytes(b)))),
                        ),
                )
                .when(idx < 9, |d| d.child(div().flex_none().px(px(6.)).py(px(2.)).rounded(px(5.)).bg(rgba(0xffffff0f)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Ctrl+{}", idx + 1))))
                .child(
                    div()
                        .id(("fav", idx))
                        .flex_none()
                        .p(px(4.))
                        .rounded(px(6.))
                        .hover(|s| s.bg(Tokens::active()))
                        .child(svg().path(if fav { "brand/star.svg" } else { "icons/star.svg" }).size(px(14.)).text_color(if fav { Tokens::warning() } else { Tokens::text_3() }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.shared.update(cx, |s, cx| {
                                s.store.toggle_favourite(p, &fav_id);
                                s.save();
                                cx.notify();
                            });
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick(p, pick_id.clone(), cx)))
        };

        let mut list = div().id("picker-list").flex().flex_col().gap(px(1.)).flex_1().min_h_0().overflow_y_scroll().px(px(6.)).pb(px(6.));
        for r in main {
            list = list.child(render_row(r, &mut n, cx));
        }
        if !legacy.is_empty() {
            let count = legacy.len();
            list = list.child(
                div()
                    .id("legacy-hdr")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .h(px(48.))
                    .flex_none()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Tokens::hover()))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Legacy models"))
                            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{count} model{}", if count == 1 { "" } else { "s" }))),
                    )
                    .child(icon(if legacy_shown { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" }, Tokens::text_3(), 14.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.legacy_open = !this.legacy_open;
                        cx.notify();
                    })),
            );
            if legacy_shown {
                for r in legacy {
                    list = list.child(render_row(r, &mut n, cx));
                }
            }
        }
        if n == 0 && !loading && sel.is_none() {
            list = list.child(div().p(px(12.)).child(muted("Star models to pin them here.")));
        }
        if loading {
            list = list.child(div().p(px(12.)).child(muted("Loading models…")));
        }
        if let Some(s) = status.filter(|s| s.state != AiProviderState::Ready) {
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .child(muted(s.detail.clone().unwrap_or_else(|| format!("{} is {}.", s.label, crate::ui::state_text(s.state).to_lowercase()))))
                    .child(
                        div().id("picker-setup").text_size(px(size::SM)).text_color(Tokens::accent()).cursor_pointer().child("Set up in Settings").on_click(cx.listener(|this, _, w, cx| {
                            this.close(cx);
                            mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx);
                        })),
                    ),
            );
        }

        let search = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mx(px(10.))
            .mt(px(6.))
            .mb(px(4.))
            .h(px(36.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(icon("icons/search.svg", Tokens::text_3(), 14.))
            .child(div().flex_1().child(field(&self.search).appearance(false).bordered(false).focus_bordered(false)));

        let footer = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .p(px(8.))
            .border_t_1()
            .border_color(Tokens::border())
            .child(div().flex_1().child(field(&self.custom_model).appearance(false).bordered(false).focus_bordered(false)))
            .child(
                div()
                    .id("custom-use")
                    .px(px(10.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .bg(Tokens::active())
                    .cursor_pointer()
                    .text_size(px(size::XS))
                    .text_color(Tokens::text_1())
                    .hover(|s| s.opacity(0.85))
                    .child("Use custom id")
                    .on_click(cx.listener(|this, _, w, cx| this.add_custom_model(w, cx))),
            );

        popup_surface()
            .w(px(440.))
            .h(px(400.))
            .flex_row()
            .rounded(px(14.))
            .bg(rgb(0x111214))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let k = ev.keystroke.key.as_str();
                if k == "escape" {
                    this.close(cx);
                    cx.stop_propagation();
                } else if ev.keystroke.modifiers.control {
                    if let Some(i) = k.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                        if let Some(r) = this.ordered(cx).into_iter().nth(i - 1) {
                            this.pick(r.p, r.m.id, cx);
                        }
                        cx.stop_propagation();
                    }
                }
            }))
            .child(rail)
            .child(div().flex().flex_col().flex_1().min_w_0().child(search).child(list).child(footer))
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let above = self.above;
        div().relative().child(self.button(cx)).when(self.open, |d| {
            let pop = deferred(
                anchored()
                    .anchor(if above { Anchor::BottomLeft } else { Anchor::TopLeft })
                    .offset(point(px(0.), px(if above { -6. } else { 6. })))
                    .snap_to_window_with_margin(px(8.))
                    .child(self.popover(cx)),
            )
            .with_priority(2);
            // Zero-size anchor point at the button's top-left (above) or bottom-left (below).
            d.child(if above { div().absolute().top_0().left_0().child(pop) } else { div().absolute().bottom_0().left_0().child(pop) })
        })
    }
}

/// Models the registry/provider marks as older: an explicit "legacy" tag, else old family versions.
pub fn is_legacy(_p: AiProviderId, m: &AiModel) -> bool {
    if m.tags.as_ref().map(|t| t.iter().any(|t| t.eq_ignore_ascii_case("legacy"))).unwrap_or(false) {
        return true;
    }
    let id = m.id.to_lowercase();
    let id = id.rsplit('/').next().unwrap_or(&id);
    ["claude-1", "claude-2", "claude-3", "claude-instant", "gpt-3", "gpt-4", "gemini-1.", "gemini-pro", "text-davinci", "o1-", "o3-mini"].iter().any(|p| id.starts_with(p))
}

/// Human label: the model's own label when it has one, else a tidied id ("claude-opus-5-5" → "Opus 5.5").
pub fn pretty_model(p: AiProviderId, m: &AiModel) -> String {
    if m.label != m.id && !m.label.is_empty() {
        return m.label.clone();
    }
    pretty_id(p, &m.id)
}

pub fn pretty_id(_p: AiProviderId, id: &str) -> String {
    if id == "default" {
        return "Default model".into();
    }
    let base = id.rsplit('/').next().unwrap_or(id);
    let lower = base.to_lowercase();
    if lower.starts_with("claude-") || lower.starts_with("gemini-") {
        let rest = if lower.starts_with("claude-") { &base[7..] } else { base };
        let mut out: Vec<String> = vec![];
        let mut prev_num = false;
        for tok in rest.split('-').filter(|t| !t.is_empty()) {
            let num = tok.chars().all(|c| c.is_ascii_digit() || c == '.');
            if num && prev_num {
                if let Some(last) = out.last_mut() {
                    last.push('.');
                    last.push_str(tok);
                }
            } else {
                let mut cs = tok.chars();
                out.push(if num { tok.to_string() } else { cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default() });
            }
            prev_num = num;
        }
        return out.join(" ");
    }
    if lower.starts_with("gpt-") {
        return format!("GPT-{}", &base[4..]);
    }
    if base.chars().count() > 28 {
        format!("{}…", base.chars().take(27).collect::<String>())
    } else {
        base.to_string()
    }
}

/// Label for the currently selected model (uses the cached list's label when available).
pub fn current_label(s: &Shared) -> String {
    let prefs = &s.store.prefs;
    if prefs.model.is_empty() {
        return "Select model".into();
    }
    match s.models.get(&prefs.provider).and_then(|l| l.iter().find(|m| m.id == prefs.model)) {
        Some(m) => pretty_model(prefs.provider, m),
        None => pretty_id(prefs.provider, &prefs.model),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn pretty_ids() {
        assert_eq!(pretty_id(AiProviderId::Anthropic, "claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(pretty_id(AiProviderId::Gemini, "gemini-2.5-flash"), "Gemini 2.5 Flash");
        assert_eq!(pretty_id(AiProviderId::Openai, "gpt-5.6-terra"), "GPT-5.6-terra");
        assert_eq!(pretty_id(AiProviderId::Codex, "default"), "Default model");
    }

    #[::core::prelude::v1::test]
    fn legacy_heuristic() {
        let m = |id: &str| AiModel::new(AiProviderId::Anthropic, id, id);
        assert!(is_legacy(AiProviderId::Anthropic, &m("claude-3-5-sonnet")));
        assert!(!is_legacy(AiProviderId::Anthropic, &m("claude-sonnet-5-5")));
        assert!(is_legacy(AiProviderId::Openai, &m("gpt-4o")));
    }
}
