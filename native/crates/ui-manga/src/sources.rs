use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use anyhow::anyhow;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_sources::SourceInfo;
use mv_ui::{button, chip, size, ButtonKind, Tokens};

use crate::backend;
use crate::ui::muted;

#[derive(Clone)]
enum Health {
    Unknown,
    Testing,
    Ok(String),
    Fail(String),
}

struct Row {
    info: SourceInfo,
    plugin: bool,
    health: Health,
}

pub struct SourcesPanel {
    rows: Option<Result<Vec<Row>, String>>,
    disabled: HashSet<String>,
    input: Entity<InputState>,
    installing: bool,
    note: Option<(bool, String)>,
    /// Preferred chapter language (manga pages show only chapters in it, when there are any).
    lang: String,
}

const LANGUAGES: [(&str, &str); 8] = [
    ("en", "English"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("fr", "French"),
    ("id", "Indonesian"),
    ("ru", "Russian"),
    ("ja", "Japanese"),
    ("all", "All languages"),
];

impl SourcesPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Plugin URL (GitHub repo or manifest.json)"));
        let mut p = Self { rows: None, disabled: backend::disabled_sources(), input, installing: false, note: None, lang: backend::preferred_language() };
        p.reload(false, cx);
        p
    }

    fn reload(&mut self, force: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let reg = if force { backend::reload_registry().await } else { backend::registry().await };
                let rows: Vec<Row> = reg
                    .list()
                    .into_iter()
                    .map(|info| Row { plugin: backend::is_installed_plugin(&info.id), info, health: Health::Unknown })
                    .collect();
                rows
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.disabled = backend::disabled_sources();
                this.rows = Some(res.map_err(|e| format!("{e:#}")));
                cx.notify();
            });
        })
        .detach();
    }

    fn set_health(&mut self, id: &str, h: Health, cx: &mut Context<Self>) {
        if let Some(Ok(rows)) = &mut self.rows {
            if let Some(r) = rows.iter_mut().find(|r| r.info.id == id) {
                r.health = h;
            }
        }
        cx.notify();
    }

    fn test(&mut self, id: String, cx: &mut Context<Self>) {
        self.set_health(&id, Health::Testing, cx);
        let sid = id.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let s = backend::source(&sid).await?;
                let t = Instant::now();
                let list = match s.get_popular_manga(1).await {
                    Ok(l) => l,
                    Err(mv_sources::SourceError::Unsupported(_)) => s.search_manga("a", 1).await.map_err(|e| anyhow!("{e}"))?,
                    Err(e) => return Err(anyhow!("{e}")),
                };
                if list.is_empty() {
                    return Err(anyhow!("responded but returned no results"));
                }
                Ok(format!("{} results in {} ms", list.len(), t.elapsed().as_millis()))
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                let h = match res {
                    Ok(s) => Health::Ok(s),
                    Err(e) => Health::Fail(format!("{e:#}")),
                };
                this.set_health(&id, h, cx);
            });
        })
        .detach();
    }

    fn toggle(&mut self, id: String, cx: &mut Context<Self>) {
        let enable = self.disabled.contains(&id);
        match backend::set_source_enabled(&id, enable) {
            Ok(()) => {
                if enable {
                    self.disabled.remove(&id);
                } else {
                    self.disabled.insert(id);
                }
            }
            Err(e) => self.note = Some((false, format!("Could not save: {e:#}"))),
        }
        cx.notify();
    }

    fn uninstall(&mut self, id: String, cx: &mut Context<Self>) {
        match mv_plugins::repo::uninstall_plugin(&backend::plugins_dir(), &id) {
            Ok(_) => {
                self.note = Some((true, format!("Removed {id}")));
                self.reload(true, cx);
            }
            Err(e) => self.note = Some((false, format!("{e:#}"))),
        }
        cx.notify();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let url = self.input.read(cx).value().trim().to_string();
        if url.is_empty() || self.installing {
            return;
        }
        self.installing = true;
        self.note = Some((true, "Installing...".into()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                mv_plugins::repo::install_plugin_from_url(&url, &backend::plugins_dir(), Arc::new(mv_plugins::ReqwestBackend::new())).await
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.installing = false;
                match res {
                    Ok(m) => {
                        this.note = Some((true, format!("Installed {} {}", m.name, m.version)));
                        this.reload(true, cx);
                    }
                    Err(e) => this.note = Some((false, format!("Install failed: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SourcesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list: AnyElement = match &self.rows {
            None => muted("Loading sources...").into_any_element(),
            Some(Err(e)) => div().text_color(Tokens::danger()).child(e.clone()).into_any_element(),
            Some(Ok(rows)) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(rows.iter().map(|r| {
                    let id = r.info.id.clone();
                    let enabled = !self.disabled.contains(&id);
                    let (hid, hid2, hid3) = (id.clone(), id.clone(), id.clone());
                    let (health_text, health_color) = match &r.health {
                        Health::Unknown => (String::new(), Tokens::text_3()),
                        Health::Testing => ("Testing...".into(), Tokens::text_3()),
                        Health::Ok(s) => (s.clone(), Tokens::accent()),
                        Health::Fail(s) => (s.clone(), Tokens::danger()),
                    };
                    let meta = format!(
                        "{}  -  v{}  -  {}{}{}",
                        r.info.id,
                        r.info.version,
                        r.info.lang,
                        if r.info.nsfw { "  -  18+" } else { "" },
                        if r.plugin { "  -  plugin" } else { "  -  built in" }
                    );
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .p(px(12.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(Tokens::border())
                        .bg(Tokens::surface())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(if enabled { Tokens::text_1() } else { Tokens::text_3() }).child(r.info.name.clone()))
                                .child(muted(meta))
                                .when(!health_text.is_empty(), |d| d.child(div().text_size(px(size::SM)).text_color(health_color).child(health_text))),
                        )
                        .child(button(SharedString::from(format!("test-{id}")), "Test", ButtonKind::Ghost).on_click(cx.listener(move |this, _, _, cx| this.test(hid.clone(), cx))))
                        .child(
                            chip(SharedString::from(format!("en-{id}")), if enabled { "Enabled" } else { "Disabled" }, enabled)
                                .on_click(cx.listener(move |this, _, _, cx| this.toggle(hid2.clone(), cx))),
                        )
                        .when(r.plugin, |d| {
                            d.child(button(SharedString::from(format!("rm-{id}")), "Remove", ButtonKind::Ghost).on_click(cx.listener(move |this, _, _, cx| this.uninstall(hid3.clone(), cx))))
                        })
                }))
                .into_any_element(),
        };
        div()
            .id("sources-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Sources"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(Tokens::border())
                    .bg(Tokens::surface())
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child("Chapter language"))
                    .child(muted("Manga pages list chapters in this language. Titles without any fall back to every language."))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(LANGUAGES.map(|(code, name)| {
                        let active = crate::logic::lang_matches(&self.lang, code) && (code == "all") == (self.lang == "all");
                        chip(SharedString::from(format!("lang-{code}")), name, active).on_click(cx.listener(move |this, _, _, cx| {
                            this.lang = code.to_string();
                            backend::set_config_str("chapterLanguage", code);
                            backend::bump_data();
                            cx.notify();
                        }))
                    }))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().child(Input::new(&self.input)))
                    .child(button("install", if self.installing { "Installing..." } else { "Install plugin" }, ButtonKind::Primary).on_click(cx.listener(|this, _, _, cx| this.install(cx)))),
            )
            .when_some(self.note.clone(), |d, (ok, n)| {
                d.child(div().text_size(px(size::SM)).text_color(if ok { Tokens::text_2() } else { Tokens::danger() }).child(n))
            })
            .child(list)
    }
}
