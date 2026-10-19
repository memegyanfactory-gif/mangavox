//! T3-style model picker: a compact button (logo, model name, caret) that opens a popover with a
//! provider rail, search, model rows (Up/Down/Enter and Ctrl+1..9), favourites and a collapsible
//! "Legacy models" group. Shared by the dock composer and the Studio toolbar.
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{provider_def, AiModel, AiProviderId, AiProviderSettings, AiProviderState};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::{appear, btn, empty_view, favorite_star, fmt_bytes, glyphs, icon_button, inline_alert, kbd, list_row, ph, ph_icon, popup_surface, size, skeleton_line, tip, AlertKind, ButtonKind, Interactive as _, Route, Tokens, Weight};

use crate::shared::{shared, Shared};
use crate::store::model_key;
use crate::ui::{field, muted, not_ready_title, provider_logo, search_prefix, set_value, status_pill, text_state, value_of};

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
    /// The custom model id field is showing.
    adding: bool,
    /// Keyboard highlight (index into [`ModelPicker::ordered`]); follows the pointer too.
    highlight: usize,
    list_scroll: ScrollHandle,
    /// Popover opens above the button (composer) instead of below (toolbar).
    above: bool,
    /// Up/Down/Enter while the search field has focus (dropped when the popover closes).
    keys: Option<Subscription>,
    /// What had the keyboard before the popover opened (the composer, usually); it gets it back
    /// on close, so you can type straight after choosing a model.
    restore: Option<FocusHandle>,
    /// Where the press that closed the popover from outside landed. When that press was on the
    /// model button, its click must not open the popover again (it would never close by clicking).
    closed_by_press: Option<Point<Pixels>>,
    _subs: Vec<Subscription>,
}

pub fn create(above: bool, window: &mut Window, cx: &mut App) -> AnyView {
    create_entity(above, window, cx).into()
}

/// The picker as an entity, so the dock can open it (first run, "Choose a model").
pub(crate) fn create_entity(above: bool, window: &mut Window, cx: &mut App) -> Entity<ModelPicker> {
    cx.new(|cx| ModelPicker::new(above, window, cx))
}

impl ModelPicker {
    fn new(above: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shared = shared(cx);
        let search = text_state("Search models…", window, cx);
        let custom_model = text_state("Model id, e.g. qwen3:8b", window, cx);
        let subs = vec![
            cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    this.highlight = 0;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&custom_model, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.add_custom_model(window, cx);
                }
            }),
            cx.observe(&shared, |_, _, cx| cx.notify()),
        ];
        shared.update(cx, |s, cx| s.refresh_providers(false, cx));
        Self { shared, search, custom_model, open: false, provider: None, legacy_open: false, adding: false, highlight: 0, list_scroll: ScrollHandle::new(), above, keys: None, restore: None, closed_by_press: None, _subs: subs }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
            return;
        }
        let p = self.shared.read(cx).store.prefs.provider;
        self.open_at(Some(p), window, cx);
    }

    /// Opens the popover on `provider` (favourites for `None`) with the search field focused.
    pub(crate) fn open_at(&mut self, provider: Option<AiProviderId>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            self.restore = window.focused(cx);
        }
        self.open = true;
        self.closed_by_press = None;
        self.provider = provider;
        self.legacy_open = false;
        self.adding = false;
        set_value(&self.search, "", window, cx);
        self.search.update(cx, |s, cx| s.focus(window, cx));
        self.shared.update(cx, |s, cx| {
            if let Some(p) = provider {
                s.load_models(p, false, cx);
            }
            s.refresh_providers(false, cx);
        });
        self.highlight = self.current_index(cx).unwrap_or(0);
        self.watch_keys(cx);
        cx.notify();
    }

    /// Closes the popover. Focus goes back to where it was only when it is still inside the
    /// popover (a click elsewhere has already put it where the user wants it).
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_with(false, window, cx);
    }

    /// `from_inside`: the close came from the popover itself (a pick, Esc). The keyboard was then
    /// in it even when Tab had moved it from the fields to a row or rail logo, whose focus
    /// handles this view doesn't hold, so it always goes back.
    fn close_with(&mut self, from_inside: bool, window: &mut Window, cx: &mut Context<Self>) {
        let inside = from_inside || self.search.read(cx).focus_handle(cx).is_focused(window) || self.custom_model.read(cx).focus_handle(cx).is_focused(window);
        self.open = false;
        self.keys = None;
        if let Some(prev) = self.restore.take().filter(|_| inside) {
            prev.focus(window, cx);
        }
        cx.notify();
    }

    /// Arrow keys are bound to the text field's own actions, which run before any key listener, so
    /// they are taken here with a keystroke interceptor while the search field has focus.
    fn watch_keys(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let focus = self.search.read(cx).focus_handle(cx);
        self.keys = Some(cx.intercept_keystrokes(move |ev, window, cx| {
            let ks = &ev.keystroke;
            if ks.modifiers.control || ks.modifiers.alt || ks.modifiers.shift || ks.modifiers.platform || !focus.is_focused(window) {
                return;
            }
            let step = match ks.key.as_str() {
                "up" => -1,
                "down" => 1,
                "enter" => 0,
                _ => return,
            };
            cx.stop_propagation();
            let _ = weak.update(cx, |this, cx| this.nav_key(step, window, cx));
        }));
    }

    fn nav_key(&mut self, step: i32, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.ordered(cx);
        if rows.is_empty() {
            return;
        }
        let n = rows.len();
        if step == 0 {
            let r = rows[self.highlight.min(n - 1)].clone();
            self.pick(r.p, r.m.id, window, cx);
            return;
        }
        self.highlight = (self.highlight as i32 + step).rem_euclid(n as i32) as usize;
        // The legacy header sits between the main rows and the legacy ones.
        let main = self.rows(cx).0.len();
        self.list_scroll.scroll_to_item(if self.highlight < main { self.highlight } else { self.highlight + 1 });
        cx.notify();
    }

    fn current_index(&self, cx: &App) -> Option<usize> {
        let s = self.shared.read(cx);
        let (p, m) = (s.store.prefs.provider, s.store.prefs.model.clone());
        self.ordered(cx).iter().position(|r| r.p == p && r.m.id == m)
    }

    fn pick(&mut self, p: AiProviderId, model: String, window: &mut Window, cx: &mut Context<Self>) {
        self.shared.update(cx, |s, cx| s.use_in_assistant(p, &model, cx));
        self.close_with(true, window, cx);
    }

    fn add_custom_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = value_of(&self.custom_model, cx).trim().to_string();
        let Some(provider) = self.provider else { return };
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
        self.adding = false;
        self.shared.update(cx, |s, cx| {
            s.models.remove(&provider);
            s.load_models(provider, true, cx);
        });
        self.pick(provider, id, window, cx);
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

    /// Rows in display order (what Ctrl+1..9 and the arrow keys index into).
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
        let (provider, label, model) = {
            let s = self.shared.read(cx);
            (s.store.prefs.provider, current_label(s), s.store.prefs.model.clone())
        };
        let empty = model.is_empty();
        let hint = if empty { "Choose a model".to_string() } else { format!("{} · {model}", provider_def(provider).label) };
        div()
            .id("model-btn")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded(px(radius::MD))
            .text_size(px(size::SM))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if self.open { Tokens::text_1() } else if empty { Tokens::accent() } else { Tokens::text_2() })
            .role(Role::Button)
            .aria_expanded(self.open)
            .when(self.open, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            .pressable()
            .when(!self.open, |d| d.tooltip(tip(hint)))
            .child(if empty { ph_icon(glyphs::MODEL, Weight::Bold, Tokens::accent(), icon_px::SM).into_any_element() } else { provider_logo(provider, icon_px::SM) })
            .child(div().max_w(px(140.)).truncate().child(label))
            .child(ph_icon(if self.above { ph::CARET_UP } else { ph::CARET_DOWN }, Weight::Bold, Tokens::text_3(), icon_px::XS))
            .on_click(cx.listener(|this, ev: &ClickEvent, w, cx| {
                // The composer focuses its text box on click; the search field must keep focus.
                cx.stop_propagation();
                // This press already closed the open popover (outside-click dismissal runs on
                // mouse down); toggling now would reopen it.
                let pressed_at = this.closed_by_press.take();
                if let ClickEvent::Mouse(m) = ev {
                    if pressed_at == Some(m.down.position) {
                        return;
                    }
                }
                this.toggle(w, cx)
            }))
    }

    fn rail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let sel = self.provider;
        let has_favs = !self.shared.read(cx).store.prefs.favourites.is_empty();
        let rail_item = |id: SharedString, active: bool, label: SharedString, child: AnyElement| {
            div()
                .id(id)
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(36.))
                .h(px(36.))
                .mx_auto()
                .rounded(px(radius::MD))
                .role(Role::Tab)
                .aria_selected(active)
                .aria_label(label.clone())
                .map(|d| if active { d.bg(Tokens::active()).hover(|s| s.bg(Tokens::active_hover())) } else { d.hover(|s| s.bg(Tokens::hover())) })
                .pressable()
                .tooltip(tip(label))
                .when(active, |d| d.child(div().absolute().left(px(-5.)).top(px(10.)).w(px(3.)).h(px(16.)).rounded_full().bg(Tokens::accent())))
                .child(child)
        };
        // Filled gold when favourites exist (or the tab is open), an outline when there are none.
        let star_on = sel.is_none() || has_favs;
        let mut rail = div().id("picker-rail").flex().flex_col().gap(px(2.)).w(px(48.)).flex_none().py(px(6.)).overflow_y_scroll().border_r_1().border_color(Tokens::border());
        rail = rail.child(
            rail_item("rail-fav".into(), sel.is_none(), "Favourites".into(), ph_icon(glyphs::FAVORITES, if star_on { Weight::Fill } else { Weight::Regular }, if star_on { Tokens::favorite() } else { Tokens::text_3() }, icon_px::MD).into_any_element()).on_click(cx.listener(|this, _, _, cx| {
                this.provider = None;
                this.highlight = 0;
                cx.notify();
            })),
        );
        rail = rail.child(div().mx(px(12.)).my(px(3.)).h(px(1.)).flex_none().bg(Tokens::border()));
        for id in self.rail_providers(cx) {
            rail = rail.child(rail_item(SharedString::from(id.as_str()), sel == Some(id), provider_def(id).label.into(), provider_logo(id, icon_px::LG)).on_click(cx.listener(move |this, _, _, cx| {
                this.provider = Some(id);
                this.highlight = 0;
                this.adding = false;
                this.shared.update(cx, |s, cx| s.load_models(id, false, cx));
                cx.notify();
            })));
        }
        rail
    }

    fn render_row(&self, r: Row, idx: usize, current: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let highlighted = idx == self.highlight;
        let in_favs = self.provider.is_none();
        let (p, mid, fav) = (r.p, r.m.id.clone(), r.fav);
        let (pick_id, fav_id) = (mid.clone(), mid.clone());
        let title = pretty_model(p, &r.m);
        let meta = row_meta(p, &r.m, in_favs, &title);
        list_row(("model-row", idx), current)
            .group("model-row")
            .flex_none()
            .h(px(48.))
            .py(px(0.))
            .gap(px(8.))
            // The keyboard highlight is the hover layer, one step stronger on the current row, so
            // Up/Down landing on it still shows where the highlight is.
            .when(highlighted, |d| d.bg(if current { Tokens::active_hover() } else { Tokens::hover() }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(title))
                    .children(meta),
            )
            // Shortcut hint only on the highlighted or hovered row, so nine chips don't compete with the names.
            .when(idx < 9, |d| d.child(kbd(format!("ctrl-{}", idx + 1)).when(!highlighted, |k| k.invisible().group_hover("model-row", |s| s.visible()))))
            .child(
                div()
                    .flex_none()
                    .when(!fav && !highlighted, |d| d.invisible().group_hover("model-row", |s| s.visible()))
                    .child(favorite_star(("fav", idx), fav).size(icon_px::SM).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.shared.update(cx, |s, cx| {
                            s.store.toggle_favourite(p, &fav_id);
                            s.save();
                            cx.notify();
                        });
                    }))),
            )
            .child(div().flex_none().w(px(14.)).when(current, |d| d.child(ph_icon(ph::CHECK, Weight::Bold, Tokens::accent(), icon_px::SM))))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.highlight != idx {
                    this.highlight = idx;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, w, cx| this.pick(p, pick_id.clone(), w, cx)))
    }

    fn popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (current_p, current_m) = {
            let s = self.shared.read(cx);
            (s.store.prefs.provider, s.store.prefs.model.clone())
        };
        let sel = self.provider;
        let query = value_of(&self.search, cx);
        let searching = !query.is_empty();
        let (main, legacy) = self.rows(cx);
        let (loading, status, loaded) = {
            let s = self.shared.read(cx);
            (sel.map(|p| s.models_loading.contains(&p)).unwrap_or(false), sel.and_then(|p| s.status(p).cloned()), sel.map(|p| s.models.contains_key(&p)).unwrap_or(true))
        };
        let legacy_shown = self.legacy_open || searching;
        let fav_count = self.shared.read(cx).store.prefs.favourites.len();

        let mut n = 0usize;
        let mut list = div().id("picker-list").flex().flex_col().gap(px(1.)).flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.list_scroll).px(px(6.)).pb(px(6.));
        for r in main {
            let current = current_p == r.p && current_m == r.m.id;
            list = list.child(self.render_row(r, n, current, cx));
            n += 1;
        }
        if !legacy.is_empty() {
            let count = legacy.len();
            list = list.child(
                list_row("legacy-hdr", false)
                    .flex_none()
                    .h(px(40.))
                    .py(px(0.))
                    .role(Role::Button)
                    .aria_expanded(legacy_shown)
                    .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_2()).child("Legacy models"))
                    .child(muted(count.to_string()))
                    .child(ph_icon(if legacy_shown { ph::CARET_DOWN } else { ph::CARET_RIGHT }, Weight::Bold, Tokens::text_3(), icon_px::SM))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.legacy_open = !this.legacy_open;
                        cx.notify();
                    })),
            );
            if legacy_shown {
                for r in legacy {
                    let current = current_p == r.p && current_m == r.m.id;
                    list = list.child(self.render_row(r, n, current, cx));
                    n += 1;
                }
            }
        }
        let ready = status.as_ref().map(|s| s.state == AiProviderState::Ready).unwrap_or(true);
        if loading && n == 0 {
            // Skeleton rows shaped like model rows while the list loads.
            list = list.children((0..4usize).map(|i| div().id(("model-skel", i)).flex().flex_col().justify_center().gap(px(7.)).h(px(48.)).flex_none().px(px(10.)).child(skeleton_line(relative(0.55))).child(skeleton_line(relative(0.3)).h(px(9.)))));
        } else if n == 0 && searching {
            list = list.child(empty_view(glyphs::SEARCH, "No models match", format!("Nothing matches “{query}”.")).compact());
        } else if n == 0 && sel.is_none() {
            list = list.child(empty_view(glyphs::FAVORITES, "No favourites yet", "Click the star on any model to pin it here.").compact());
        } else if let (0, Some(p), true, true) = (n, sel, ready, loaded) {
            let label = provider_def(p).label;
            list = list.child(
                empty_view(glyphs::MODEL, format!("No models found for {label}"), "Refresh the list, or add a model id below.")
                    .compact()
                    .action(btn("picker-refresh-empty", "Refresh", ButtonKind::Secondary).small().icon(glyphs::REFRESH).on_click(cx.listener(move |this, _, _, cx| this.shared.update(cx, |s, cx| s.load_models(p, true, cx))))),
            );
        }
        if let Some(s) = status.filter(|s| s.state != AiProviderState::Ready) {
            let fix = cx.listener(|this, _, w, cx| {
                this.close(w, cx);
                mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx);
            });
            list = list.child(div().pt(px(6.)).child(inline_alert(AlertKind::Warning, not_ready_title(&s.label, s.state), s.detail.clone().unwrap_or_default()).action("Set up", fix)));
        }

        // Header: what the list shows, its status and a refresh.
        let head = div().flex().items_center().gap(px(8.)).h(px(34.)).px(px(12.)).flex_none().map(|d| match sel {
            Some(p) => {
                let st = self.shared.read(cx).status(p).map(|s| s.state);
                d.child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(provider_def(p).label))
                    .children(st.map(status_pill))
                    .child(
                        icon_button("picker-refresh", glyphs::REFRESH, "Refresh models")
                            .small()
                            .when(loading, |b| b.disabled("Loading models…"))
                            .on_click(cx.listener(move |this, _, _, cx| this.shared.update(cx, |s, cx| s.load_models(p, true, cx)))),
                    )
            }
            None => d.child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Favourites")).child(muted(fav_count.to_string())),
        });

        let search = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .mx(px(12.))
            .mb(px(4.))
            .h(px(34.))
            .border_b_1()
            .border_color(Tokens::border())
            .child(search_prefix())
            .child(div().flex_1().child(field(&self.search).appearance(false).bordered(false).focus_bordered(false)));

        // Progressive disclosure: the custom id field only when asked for, and it says where it goes.
        let footer = div().flex().flex_col().flex_none().gap(px(6.)).px(px(8.)).py(px(6.)).border_t_1().border_color(Tokens::border()).map(|d| match (self.adding, sel) {
            (true, Some(p)) => d
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(div().flex_1().child(field(&self.custom_model).small()))
                        .child(btn("custom-add", "Add", ButtonKind::Secondary).small().icon(glyphs::ADD).on_click(cx.listener(|this, _, w, cx| this.add_custom_model(w, cx))))
                        .child(icon_button("custom-cancel", glyphs::CLOSE, "Cancel").small().on_click(cx.listener(|this, _, _, cx| {
                            this.adding = false;
                            cx.notify();
                        }))),
                )
                .child(div().px(px(2.)).child(muted(format!("Adds to {}", provider_def(p).label)))),
            _ => d.child(
                btn("custom-open", "Add a model id", ButtonKind::Ghost)
                    .small()
                    .icon(glyphs::ADD)
                    .when(sel.is_none(), |b| b.disabled("Pick a provider on the left first"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.adding = true;
                        this.custom_model.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    })),
            ),
        });

        let surface = popup_surface()
            .w(px(460.))
            .h(px(420.))
            .p(px(0.))
            .flex_row()
            .overflow_hidden()
            .on_mouse_down_out(cx.listener(|this, ev: &MouseDownEvent, w, cx| {
                this.closed_by_press = Some(ev.position);
                this.close(w, cx);
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, w, cx| {
                let k = ev.keystroke.key.as_str();
                if k == "escape" {
                    if this.adding {
                        // Back to the search field rather than leaving focus nowhere.
                        this.adding = false;
                        this.search.update(cx, |s, cx| s.focus(w, cx));
                        cx.notify();
                    } else {
                        // Key events reach the surface only while the keyboard is inside it.
                        this.close_with(true, w, cx);
                    }
                    cx.stop_propagation();
                } else if ev.keystroke.modifiers.control {
                    if let Some(i) = k.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                        if let Some(r) = this.ordered(cx).into_iter().nth(i - 1) {
                            this.pick(r.p, r.m.id, w, cx);
                        }
                        cx.stop_propagation();
                    }
                }
            }))
            .child(self.rail(cx))
            .child(div().flex().flex_col().flex_1().min_w_0().pt(px(4.)).child(head).child(search).child(list).child(footer));
        appear("picker-pop", surface)
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let above = self.above;
        if self.open {
            // Keep the highlight inside the list as it shrinks with the search.
            let n = self.ordered(cx).len();
            if n > 0 && self.highlight >= n {
                self.highlight = n - 1;
            }
        }
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

/// Second line of a model row: what helps you choose (context, size, parameters). In Favourites
/// it names the provider instead, since the rail does not.
fn row_meta(p: AiProviderId, m: &AiModel, in_favs: bool, title: &str) -> Option<Div> {
    let mut parts: Vec<String> = vec![];
    if let Some(c) = m.context.filter(|c| *c > 0) {
        parts.push(format!("{} context", fmt_context(c)));
    }
    if let Some(ps) = param_size(m) {
        parts.push(ps);
    }
    if let Some(b) = m.size_bytes.filter(|b| *b > 0) {
        parts.push(fmt_bytes(b));
    }
    let line = div().flex().items_center().gap(px(5.)).min_w_0().text_size(px(size::XS)).text_color(Tokens::text_3());
    if in_favs {
        let mut text = provider_def(p).label.to_string();
        for part in &parts {
            text.push_str(" · ");
            text.push_str(part);
        }
        return Some(line.child(provider_logo(p, icon_px::XS)).child(div().truncate().child(text)));
    }
    if !parts.is_empty() {
        return Some(line.child(div().truncate().child(parts.join(" · "))));
    }
    // Nothing else to say: the raw id still tells two similar names apart.
    (m.id != title && m.id != "default").then(|| line.child(div().truncate().child(m.id.clone())))
}

/// "128K" / "1M" for a context length in tokens.
pub fn fmt_context(c: u64) -> String {
    let (unit, k) = if c.is_multiple_of(1024) { (1024u64, c / 1024) } else { (1000, c / 1000) };
    if k >= unit {
        let m = k as f64 / unit as f64;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as u64)
        } else {
            format!("{m:.1}M")
        }
    } else {
        format!("{k}K")
    }
}

/// Parameter count from the provider's tags (Ollama reports e.g. "8.0B").
fn param_size(m: &AiModel) -> Option<String> {
    let t = m.tags.as_ref()?.iter().find(|t| t.len() > 1 && t.ends_with('B') && t[..t.len() - 1].parse::<f64>().is_ok())?;
    let n: f64 = t[..t.len() - 1].parse().ok()?;
    Some(if (n - n.round()).abs() < 0.05 { format!("{}B params", n.round() as u64) } else { format!("{n:.1}B params") })
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
        return "Choose a model".into();
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

    #[::core::prelude::v1::test]
    fn model_facts() {
        assert_eq!(fmt_context(131_072), "128K");
        assert_eq!(fmt_context(200_000), "200K");
        assert_eq!(fmt_context(1_048_576), "1M");
        let mut m = AiModel::new(AiProviderId::Ollama, "qwen3:8b", "qwen3:8b");
        m.tags = Some(vec!["8.0B".into(), "qwen3".into()]);
        assert_eq!(param_size(&m).as_deref(), Some("8B params"));
        m.tags = Some(vec!["added".into()]);
        assert_eq!(param_size(&m), None);
    }
}
