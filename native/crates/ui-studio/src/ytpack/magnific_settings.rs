//! Settings › Magnific: the API key (paste, test, remove), the thumbnail generator's default
//! model and size, and the credits Studio has spent.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use mv_ui::{icon_tag, segmented, settings_group, settings_note, settings_row, AlertKind, Route, SettingsGroup};

use super::magnific::{self, Usage, Via, CATALOG};
use super::thumb_page::notify_changed;
use super::*;

const ASPECTS: [&str; 4] = ["16:9", "9:16", "1:1", "4:3"];

#[derive(Clone, PartialEq)]
enum Check {
    Idle,
    Checking,
    Ok,
    Failed(String),
}

pub(crate) struct MagnificSettings {
    key_input: Entity<InputState>,
    budget: Entity<InputState>,
    check: Check,
    prefs: magnific::Prefs,
    usage: Usage,
    _subs: Vec<Subscription>,
}

pub(crate) fn create(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| MagnificSettings::new(window, cx)).into()
}

/// "MSa1…24d" for a saved key.
fn masked(k: &str) -> String {
    let n = k.chars().count();
    if n <= 8 {
        return "••••".into();
    }
    format!("{}…{}", k.chars().take(4).collect::<String>(), k.chars().skip(n - 4).collect::<String>())
}

impl MagnificSettings {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = cx.new(|cx| InputState::new(window, cx).placeholder(if magnific::key().is_some() { "Paste a new key to replace it" } else { "Paste your Magnific API key" }).masked(true));
        let usage = Usage::load();
        let budget = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. 10000"));
        if let Some(b) = usage.budget {
            budget.update(cx, |s, cx| s.set_value(b.to_string(), window, cx));
        }
        let subs = vec![
            cx.subscribe_in(&key_input, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save_key(window, cx);
                }
            }),
            cx.subscribe_in(&budget, window, |this, input, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let v: String = input.read(cx).value().chars().filter(|c| c.is_ascii_digit()).collect();
                    this.usage = Usage::load();
                    this.usage.budget = v.parse().ok();
                    this.usage.save();
                    notify_changed(cx);
                    cx.notify();
                }
            }),
        ];
        MagnificSettings { key_input, budget, check: Check::Idle, prefs: magnific::Prefs::load(), usage, _subs: subs }
    }

    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let k = self.key_input.read(cx).value().trim().to_string();
        if k.is_empty() {
            return;
        }
        if let Err(e) = magnific::set_key(Some(&k)) {
            self.check = Check::Failed(format!("Could not save the key: {e:#}"));
            cx.notify();
            return;
        }
        self.key_input.update(cx, |s, cx| s.set_value("", window, cx));
        notify_changed(cx);
        self.test(cx);
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let Some(k) = magnific::key() else { return };
        self.check = Check::Checking;
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move { magnific::verify(&k).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.check = match res {
                    Ok(()) => Check::Ok,
                    Err(e) => Check::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn remove_key(&mut self, cx: &mut Context<Self>) {
        let _ = magnific::set_key(None);
        self.check = Check::Idle;
        notify_changed(cx);
        toast::show(cx, Toast::info("Magnific key removed"));
        cx.notify();
    }

    fn set_prefs(&mut self, f: impl FnOnce(&mut magnific::Prefs), cx: &mut Context<Self>) {
        self.prefs = magnific::Prefs::load();
        f(&mut self.prefs);
        self.prefs.save();
        notify_changed(cx);
        cx.notify();
    }

    fn key_section(&self, cx: &mut Context<Self>) -> SettingsGroup {
        let key = magnific::key();
        let from_env = key.is_some() && std::fs::metadata(mv_core::paths::studio().join("magnific.json")).is_err();
        let (tone, text) = match (&key, &self.check) {
            (None, _) => (Tone::Neutral, "Not connected".to_string()),
            (Some(_), Check::Checking) => (Tone::Info, "Checking…".to_string()),
            (Some(_), Check::Ok) => (Tone::Success, "Connected".to_string()),
            (Some(_), Check::Failed(_)) => (Tone::Danger, "Not working".to_string()),
            (Some(_), Check::Idle) => (Tone::Accent, "Saved".to_string()),
        };
        // Badge, masked key and the two actions wrap inside the row's control column.
        let status = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap(px(space::SM))
            .child(badge(text, tone))
            .when_some(key.as_deref().map(masked), |d, m| d.child(div().text_size(px(size::SM)).font_family(mv_ui::MONO_FONT).text_color(Tokens::text_2()).child(m)))
            .when(key.is_some(), |d| {
                d.child(btn("mg-test", "Test", ButtonKind::Secondary).small().icon(ph::PLUGS_CONNECTED).loading(self.check == Check::Checking).tooltip("Checks the key with Magnific; costs no credits").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test(cx))))
                    .when(!from_env, |d| d.child(btn("mg-remove", "Remove", ButtonKind::DangerGhost).small().icon(ph::TRASH).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.remove_key(cx)))))
            });
        // A failed check is said under the Status label, inside its row.
        let error = match &self.check {
            Check::Failed(e) => Some(
                div()
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .text_size(px(size::SM))
                    .line_height(px(18.))
                    .text_color(Tokens::danger())
                    .child(div().flex_none().pt(px(3.)).child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::XS)))
                    .child(div().flex_1().min_w_0().child(e.clone())),
            ),
            _ => None,
        };
        settings_group("API key", Some("From magnific.com → Developers → API key. Kept on this computer in Studio's data folder, never in the project or sent anywhere but Magnific.".into()))
            .child(settings_row("Status", if from_env { "From the MAGNIFIC_API_KEY environment variable" } else { "" }, status).when_some(error, |r, e| r.below(e)))
            // An input plus Save is wider than a control column: it sits under the label instead.
            .child(
                settings_row(
                    if key.is_some() { "Replace key" } else { "API key" },
                    "Press Enter or Save; Studio checks it right away.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .w_full()
                        .child(div().flex_1().min_w_0().max_w(px(420.)).child(Input::new(&self.key_input).mask_toggle()))
                        .child(div().flex_none().child(btn("mg-save", "Save", ButtonKind::Primary).icon(ph::CHECK).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.save_key(window, cx))))),
                )
                .stacked(),
            )
            .child(settings_row(
                "Get a key",
                "Magnific bills API images in credits from its API balance.",
                btn("mg-get", "Open Magnific", ButtonKind::Ghost).icon(ph::ARROW_SQUARE_OUT).on_click(|_, _, cx| cx.open_url("https://www.magnific.com/developers/dashboard/api-key")),
            ))
    }

    /// The model table: one header line and rows that share its column widths, so names,
    /// references, sizes and credits line up. The name column takes what is left and truncates;
    /// the fixed columns add up to well under the card width.
    fn model_table(&self, model: usize, cx: &mut Context<Self>) -> Div {
        const RADIO_W: f32 = 16.;
        const REFS_W: f32 = 84.;
        const SIZES_W: f32 = 96.;
        const CREDITS_W: f32 = 132.;
        let cell = |w: f32| div().flex_none().w(px(w)).text_size(px(size::XS)).truncate();
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .px(px(space::SM))
            .pb(px(6.))
            .text_size(px(size::XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(Tokens::text_3())
            .child(div().flex_none().w(px(RADIO_W)))
            .child(div().flex_1().min_w_0().child("Model"))
            .child(cell(REFS_W).child("References"))
            .child(cell(SIZES_W).child("Sizes"))
            .child(cell(CREDITS_W).flex().justify_end().child("Credits"));
        let mut list = div().flex().flex_col().gap(px(2.)).w_full().min_w_0().child(head);
        for (i, mm) in CATALOG.iter().enumerate().filter(|(_, m)| m.via == Via::Magnific) {
            let on = i == model;
            list = list.child(
                div()
                    .id(("mg-model", i))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .min_h(px(36.))
                    .py(px(4.))
                    .px(px(space::SM))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .role(Role::RadioButton)
                    .aria_selected(on)
                    .aria_label(mm.name)
                    .when(on, |d| d.bg(Tokens::active()))
                    .hover(|s| s.bg(Tokens::hover()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_prefs(
                            |p| {
                                p.model = mm.id.to_string();
                                if !mm.resolutions.contains(&p.resolution.as_str()) {
                                    p.resolution = mm.resolutions.first().unwrap_or(&"1K").to_string();
                                }
                            },
                            cx,
                        )
                    }))
                    .child(div().flex_none().w(px(RADIO_W)).child(ph_icon(ph::RADIO_BUTTON, if on { Weight::Fill } else { Weight::Regular }, if on { Tokens::accent() } else { Tokens::text_3() }, icon_px::SM)))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.))
                            .child(div().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(mm.name))
                            // Web-only "unlimited" is a note on the name, not a column of its own.
                            .when_some(mm.web_unlimited, |d, r| {
                                d.child(
                                    div()
                                        .id(("mg-unl", i))
                                        .flex_none()
                                        .tooltip(tip("Unlimited in the Magnific web app on your plan; the API still bills credits"))
                                        .child(icon_tag(ph::INFINITY, format!("Web · {r}"), Tone::Info)),
                                )
                            }),
                    )
                    .child(cell(REFS_W).text_color(Tokens::text_3()).child(if mm.refs > 0 { format!("{} refs", mm.refs) } else { "None".into() }))
                    .child(cell(SIZES_W).text_color(Tokens::text_3()).child(mm.res_label()))
                    .child(cell(CREDITS_W).flex().justify_end().text_color(Tokens::text_2()).child(match mm.credits {
                        Some((lo, hi)) if lo == hi => format!("{lo}"),
                        Some((lo, hi)) => format!("{lo} – {hi}"),
                        None => "Not listed".into(),
                    })),
            );
        }
        list
    }

    fn defaults_section(&self, cx: &mut Context<Self>) -> SettingsGroup {
        // The Thumbnails page may default to another service's model; then no row is ticked here.
        let picked = CATALOG.iter().position(|m| m.id == self.prefs.model && m.via == Via::Magnific).or(self.prefs.model.is_empty().then_some(0));
        let model = picked.unwrap_or(usize::MAX);
        let m = &CATALOG[picked.unwrap_or(0)];
        let aspect = ASPECTS.iter().position(|a| *a == self.prefs.aspect).unwrap_or(0);
        let this = cx.entity().downgrade();
        let this2 = this.clone();
        let res_items: Vec<&'static str> = m.resolutions.to_vec();
        let res_ix = res_items.iter().position(|r| *r == self.prefs.resolution).unwrap_or(0);
        settings_group("Defaults", Some("What the Thumbnails page starts with. Every model here is offered by the Magnific API; the ones shown only in the web app are left out. Higgsfield, fal.ai, Replicate, Black Forest Labs and Recraft models are picked on the Thumbnails page, with their keys in Settings › AI services.".into()))
            .child(settings_row(
                "Aspect",
                "16:9 for YouTube; 9:16 for Shorts.",
                segmented("mg-aspect", ASPECTS, aspect).small().on_change(move |ix, _, cx| {
                    let _ = this.update(cx, |this, cx| this.set_prefs(|p| p.aspect = ASPECTS[*ix].to_string(), cx));
                }),
            ))
            .when(!res_items.is_empty(), |d| {
                d.child(settings_row(
                    "Resolution",
                    format!("{}: {}", m.name, res_items.iter().map(|r| format!("{r}{}", m.cost_label(r))).collect::<Vec<_>>().join(", ")),
                    segmented("mg-res", res_items.clone(), res_ix).small().on_change(move |ix, _, cx| {
                        let r = res_items[*ix];
                        let _ = this2.update(cx, |this, cx| this.set_prefs(|p| p.resolution = r.to_string(), cx));
                    }),
                ))
            })
            .child(settings_row("Model", "Credits per image at Magnific's listed API prices.", self.model_table(model, cx)).stacked())
    }

    fn credits_section(&self, cx: &mut Context<Self>) -> SettingsGroup {
        let u = &self.usage;
        let left = u.budget.map(|b| (b as f64 - u.spent).max(0.));
        let p = u.budget.map(|b| (u.spent / b.max(1) as f64) as f32).unwrap_or(0.);
        // Under the label at full width (capped), never a fixed width that can overflow.
        let summary = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w_full()
            .max_w(px(420.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_end()
                    .gap_x(px(space::SM))
                    .child(div().text_size(px(size::XL)).font_weight(FontWeight::BOLD).text_color(Tokens::text_1()).child(format!("{} used", u.spent.round())))
                    .child(div().pb(px(3.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(match left {
                        Some(l) => format!("{} left", l.round()),
                        None => "set your plan's credits below".into(),
                    })),
            )
            .child(mv_ui::progress_bar("mg-credits", p, false))
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} images · {} this month · {} without a listed price", u.images, u.month_spent().round(), u.unpriced)));
        settings_group("Credits", Some("Magnific's API has no balance endpoint, so Studio counts what it spends at Magnific's listed prices. Your exact balance is on magnific.com.".into()))
            .child(settings_row("Spent by Studio", "", summary).stacked())
            .child(settings_row("Credits on your plan", "For \"left\" on the Thumbnails page.", div().w(px(160.)).child(Input::new(&self.budget))))
            .child(settings_row(
                "Balance and usage",
                "",
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(space::SM))
                    .child(btn("mg-dash", "Open dashboard", ButtonKind::Secondary).icon(ph::ARROW_SQUARE_OUT).on_click(|_, _, cx| cx.open_url("https://www.magnific.com/developers/dashboard")))
                    .child(btn("mg-reset", "Reset count", ButtonKind::Ghost).icon(ph::ARROW_COUNTER_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.usage = Usage { budget: this.usage.budget, ..Default::default() };
                        this.usage.save();
                        notify_changed(cx);
                        cx.notify();
                    }))),
            ))
    }
}

impl Render for MagnificSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.usage = Usage::load();
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(28.))
            // The note wraps its text and keeps the button inside it (the button drops under the
            // text when the column is narrow), so nothing reaches past the content column.
            .child(
                settings_note(AlertKind::Info, "\"Unlimited\" on a Magnific plan only covers the web app. Every image Studio makes through the API costs credits from your API balance.")
                    .icon(ph::INFINITY)
                    .action(btn("mg-open-page", "Open Thumbnails", ButtonKind::Secondary).small().icon(ph::IMAGES).on_click(|_, window, cx| mv_ui::navigate(Route::Thumbnails, window, cx))),
            )
            .child(self.key_section(cx))
            .child(self.defaults_section(cx))
            .child(self.credits_section(cx))
    }
}
