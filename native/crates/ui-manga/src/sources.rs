//! Settings -> Sources: installed sources first (test, remove), then adding one (built-in sources,
//! extension repositories ([`RepoBrowser`]), a plugin link ([`PluginLinkBox`])), Smart bypass and the chapter language. The
//! repository browser and the link box are also used by the welcome setup (ui-welcome).
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use anyhow::anyhow;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_plugins::repo::{RepoExtension, RepoUpdate};
use mv_sources::SourceInfo;
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    btn, confirm_dialog, confirm_inline, empty_view, error_view, filter_chip, glyphs, icon_button, icon_ph, menu_button, muted, ph, section_header, section_label, size, skeleton, skeleton_line,
    status_tag, switch, tag, ButtonKind, MenuItem, Ph, StatusKind, Tokens, Tone,
};

use crate::backend;

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

/// The source lists: what Browse shows, and the built-in sources not added yet.
struct Lists {
    installed: Vec<Row>,
    available: Vec<SourceInfo>,
}

pub struct SourcesPanel {
    lists: Option<Result<Lists, String>>,
    /// Sources being removed.
    removing: HashSet<String>,
    repos: Entity<RepoBrowser>,
    plugin_box: Entity<PluginLinkBox>,
    /// Preferred chapter language (manga pages show only chapters in it, when there are any).
    lang: String,
    smart_bypass: bool,
    /// [`backend::sources_version`] the lists were loaded at.
    seen_sources: u64,
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

/// Why a plugin link cannot be installed, or `None` when it looks right.
fn url_problem(url: &str) -> Option<&'static str> {
    let u = url.trim();
    if u.is_empty() {
        None
    } else if !(u.starts_with("https://") || u.starts_with("http://")) {
        Some("Paste a link that starts with https://")
    } else if u.contains(char::is_whitespace) {
        Some("The link has spaces in it")
    } else {
        None
    }
}

/// Square source avatar: its icon, or its initial on a tint.
pub fn source_avatar(info: &SourceInfo, px_size: f32, active: bool) -> Div {
    let letter: String = info.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .size(px(px_size))
        .flex_none()
        .rounded(px(radius::SM))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(if active { Tokens::accent() } else { Tokens::surface_3() })
        .text_size(px(size::XS))
        .font_weight(FontWeight::BOLD)
        .text_color(if active { Tokens::on_accent() } else { Tokens::text_2() })
        .map(|d| match info.icon.clone().filter(|u| u.starts_with("http")) {
            Some(u) => d.child(img(SharedString::from(u)).size_full().object_fit(ObjectFit::Cover)),
            None => d.child(letter),
        })
}

impl SourcesPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repos = cx.new(|cx| RepoBrowser::new(window, cx));
        let plugin_box = cx.new(|cx| PluginLinkBox::new(window, cx));
        let mut p = Self {
            lists: None,
            removing: HashSet::new(),
            repos,
            plugin_box,
            lang: backend::preferred_language(),
            smart_bypass: backend::smart_bypass(),
            seen_sources: 0,
        };
        p.reload(false, cx);
        p
    }

    /// Reloads both lists. Runs on open and whenever the installed sources change (an install
    /// from a repository or a link, a removal, a built-in source added).
    fn reload(&mut self, force: bool, cx: &mut Context<Self>) {
        self.seen_sources = backend::sources_version();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let reg = if force { backend::reload_registry().await } else { backend::registry().await };
                let installed: Vec<Row> = backend::installed_sources(reg.list())
                    .into_iter()
                    .map(|info| Row { plugin: backend::is_installed_plugin(&info.id), info, health: Health::Unknown })
                    .collect();
                let off = backend::disabled_sources();
                let available = backend::builtin_sources().into_iter().filter(|s| off.contains(&s.id)).collect();
                Lists { installed, available }
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                let mut res = res.map_err(|e| format!("{e:#}"));
                // Test results survive a reload.
                if let (Ok(new), Some(Ok(old))) = (&mut res, &this.lists) {
                    for r in &mut new.installed {
                        if let Some(o) = old.installed.iter().find(|o| o.info.id == r.info.id) {
                            r.health = o.health.clone();
                        }
                    }
                }
                this.lists = Some(res);
                cx.notify();
            });
        })
        .detach();
    }

    fn set_health(&mut self, id: &str, h: Health, cx: &mut Context<Self>) {
        if let Some(Ok(lists)) = &mut self.lists {
            if let Some(r) = lists.installed.iter_mut().find(|r| r.info.id == id) {
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
                    return Err(anyhow!("It answered but returned no titles"));
                }
                Ok(format!("OK · {} results · {} ms", list.len(), t.elapsed().as_millis()))
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

    fn add_builtin(&mut self, info: SourceInfo, cx: &mut Context<Self>) {
        match backend::set_source_enabled(&info.id, true) {
            Ok(()) => toast::show(cx, Toast::success(format!("Added {}. It is in Browse now.", info.name))),
            Err(e) => toast::show(cx, Toast::error(format!("Could not add {}: {e:#}", info.name))),
        }
        self.reload(false, cx);
    }

    /// Removing is not undoable from here, so it asks first.
    /// NN/g confirmation dialogs: https://www.nngroup.com/articles/confirmation-dialog/
    fn confirm_remove(&mut self, id: String, name: String, plugin: bool, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let body = if plugin {
            "Its files are deleted from this computer and Browse stops listing it. Titles from it stay in your library, but they cannot be refreshed until you install it again."
        } else {
            "Browse stops listing it. You can add it again any time under Add a source. Titles from it stay in your library."
        };
        confirm_dialog(format!("Remove {name}?"), body, "Remove", true)
            .icon(glyphs::DELETE)
            .cancel_label("Keep it")
            .on_confirm(move |_, cx| {
                let (id, name) = (id.clone(), name.clone());
                let _ = weak.update(cx, |this, cx| this.remove(id, name, cx));
            })
            .open(window, cx);
    }

    fn remove(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        if !self.removing.insert(id.clone()) {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sid = id.clone();
            let res = mv_ui::bg::run(async move { backend::remove_source(&sid).await }).await.and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.removing.remove(&id);
                match res {
                    Ok(()) => toast::show(cx, Toast::success(format!("Removed {name}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("Could not remove {name}: {e:#}"))),
                }
                this.reload(false, cx);
            });
        })
        .detach();
    }

    fn set_smart_bypass(&mut self, on: bool, cx: &mut Context<Self>) {
        match backend::set_smart_bypass(on) {
            Ok(()) => self.smart_bypass = on,
            Err(e) => toast::show(cx, Toast::error(format!("Could not save: {e:#}"))),
        }
        cx.notify();
    }

    fn source_row(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> Stateful<Div> {
        let (tid, rid, name, plugin) = (r.info.id.clone(), r.info.id.clone(), r.info.name.clone(), r.plugin);
        let removing = self.removing.contains(&r.info.id);
        let health: Option<AnyElement> = match &r.health {
            Health::Unknown => None,
            Health::Testing => Some(status_tag(StatusKind::Running).label("Testing").into_any_element()),
            Health::Ok(s) => Some(status_tag(StatusKind::Done).label(s.clone()).into_any_element()),
            Health::Fail(_) => Some(status_tag(StatusKind::Failed).label("Not working").into_any_element()),
        };
        let fail = match &r.health {
            Health::Fail(e) => Some(e.clone()),
            _ => None,
        };
        div()
            .id(("src-row", i))
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(12.))
            .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
            .child(source_avatar(&r.info, 32., false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(r.info.name.clone()))
                            .children(health),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .when(!r.info.lang.is_empty(), |d| d.child(tag(r.info.lang.to_uppercase(), Tone::Neutral)))
                            .when(r.info.nsfw, |d| d.child(tag("18+", Tone::Danger)))
                            .child(tag(if r.plugin { "Extension" } else { "Built in" }, if r.plugin { Tone::Accent } else { Tone::Neutral }))
                            .child(muted(format!("v{}", r.info.version))),
                    )
                    .when_some(fail, |d, e| d.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e))),
            )
            .child(
                btn(("test", i), "Test", ButtonKind::Ghost)
                    .small()
                    .icon(ph::PULSE)
                    .loading(matches!(r.health, Health::Testing))
                    .tooltip("Load one page of titles to check that the source answers")
                    .on_click(cx.listener(move |this, _, _, cx| this.test(tid.clone(), cx))),
            )
            .child(
                btn(("rm", i), if removing { "Removing" } else { "Remove" }, ButtonKind::DangerGhost)
                    .small()
                    .icon(glyphs::DELETE)
                    .loading(removing)
                    .tooltip(if plugin { "Uninstall: delete it from this computer and Browse" } else { "Take it out of Browse" })
                    .on_click(cx.listener(move |this, _, w, cx| this.confirm_remove(rid.clone(), name.clone(), plugin, w, cx))),
            )
    }

    fn available_row(&self, i: usize, info: &SourceInfo, cx: &mut Context<Self>) -> Stateful<Div> {
        let add = info.clone();
        div()
            .id(("avail-row", i))
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(10.))
            .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
            .child(source_avatar(info, 32., false))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(info.name.clone()))
                    .when(info.nsfw, |d| d.child(tag("18+", Tone::Danger))),
            )
            .child(btn(("avail-add", i), "Add", ButtonKind::Secondary).small().icon(ph::PLUS).on_click(cx.listener(move |this, _, _, cx| this.add_builtin(add.clone(), cx))))
    }
}

fn row_skeleton() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(12.))
        .child(skeleton(px(32.), px(32.), radius::SM))
        .child(div().flex().flex_col().gap(px(8.)).flex_1().child(skeleton_line(px(160.))).child(skeleton_line(px(240.)).h(px(10.))))
}

fn section(title: &'static str, icon: Ph, hint: Option<&'static str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(section_header(title, Some(icon)).mt(px(0.)).mb(px(0.)))
        .when_some(hint, |d, h| d.child(muted(h)))
}

impl Render for SourcesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if backend::sources_version() != self.seen_sources {
            self.reload(false, cx);
        }
        let installed: AnyElement = match &self.lists {
            None => div().flex().flex_col().children((0..3).map(|_| row_skeleton())).into_any_element(),
            Some(Err(e)) => error_view(e.clone())
                .title("Could not list sources")
                .compact()
                .retry(cx.listener(|this, _, _, cx| {
                    this.lists = None;
                    this.reload(true, cx);
                    cx.notify();
                }))
                .into_any_element(),
            Some(Ok(l)) if l.installed.is_empty() => {
                empty_view(glyphs::SOURCES, "Nothing installed", "Add a built-in source, or install an extension from a repository below. It shows up in Browse straight away.").compact().into_any_element()
            }
            Some(Ok(l)) => div().flex().flex_col().children(l.installed.iter().enumerate().map(|(i, r)| self.source_row(i, r, cx))).into_any_element(),
        };
        let count = match &self.lists {
            Some(Ok(l)) => Some(l.installed.len()),
            _ => None,
        };
        let available: Vec<SourceInfo> = match &self.lists {
            Some(Ok(l)) => l.available.clone(),
            _ => vec![],
        };
        div()
            .id("sources-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(section_header("Installed", Some(glyphs::SOURCES)).when_some(count, |h, n| h.count(n)).mt(px(0.)).mb(px(4.)))
                    .child(muted("Everything here is listed in Browse. Remove takes it out; an extension's files are deleted."))
                    .child(installed),
            )
            .child(
                section("Add a source", ph::PLUS, None)
                    .when(!available.is_empty(), |d| {
                        d.child(section_label("Built-in sources").pt(px(4.)))
                            .child(div().flex().flex_col().children(available.iter().enumerate().map(|(i, s)| self.available_row(i, s, cx))))
                    })
                    .child(section_label("From an extension repository").pt(px(8.)))
                    .child(muted("Add a Mihon or Tachiyomi repository, then press Install on a site."))
                    .child(self.repos.clone())
                    .child(section_label("From a plugin link").pt(px(8.)))
                    .child(self.plugin_box.clone())
                    .child(trust_note()),
            )
            .child(
                section("Connection", ph::SHIELD_CHECK, None).child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(div().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child("Smart bypass"))
                                .child(muted(
                                    "When a site shows an anti-bot check (Cloudflare and similar), MangaVox passes it in a hidden built-in browser and loads the page again. Turn off to see the block as an error instead.",
                                )),
                        )
                        .child(
                            switch("smart-bypass", self.smart_bypass)
                                .tooltip(if self.smart_bypass { "On: anti-bot checks are passed automatically" } else { "Off" })
                                .on_click(cx.listener(|this, on: &bool, _, cx| this.set_smart_bypass(*on, cx))),
                        ),
                ),
            )
            .child(
                section("Chapter language", glyphs::LANGUAGE, Some("Manga pages list chapters in this language. Titles without any fall back to every language.")).child(
                    div().flex().flex_wrap().gap(px(6.)).pt(px(4.)).children(LANGUAGES.map(|(code, name)| {
                        let active = crate::logic::lang_matches(&self.lang, code) && (code == "all") == (self.lang == "all");
                        filter_chip(SharedString::from(format!("lang-{code}")), name, active).on_click(cx.listener(move |this, _, _, cx| {
                            this.lang = code.to_string();
                            backend::set_config_str("chapterLanguage", code);
                            mv_sources::set_chapter_language(code);
                            backend::bump_data();
                            cx.notify();
                        }))
                    })),
                ),
            )
    }
}

// ---------------------------------------------------------------- shared with the welcome setup

/// What [`RepoBrowser`] and [`PluginLinkBox`] tell the view they sit in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourcesEvent {
    /// A plugin or repository extension was installed, updated or removed, and the source
    /// registry was reloaded.
    Changed,
}

/// "Plugins are third-party code" under the add boxes. NN/g on trust and security cues:
/// <https://www.nngroup.com/articles/communicating-trustworthiness/>
pub fn trust_note() -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .text_size(px(size::SM))
        .line_height(px(18.))
        .text_color(Tokens::text_3())
        .child(div().pt(px(2.)).flex_none().child(icon_ph(ph::SHIELD_CHECK, Tokens::text_3(), icon_px::SM)))
        .child(div().flex_1().min_w_0().child("Plugins and extensions are third-party code. They run in a sandbox (128 MB of memory, 2 minutes per request), but only add ones you trust."))
}

fn result_line(res: &Result<String, String>) -> Div {
    let (color, glyph, text) = match res {
        Ok(t) => (Tokens::success(), ph::CHECK_CIRCLE, t.clone()),
        Err(e) => (Tokens::danger(), ph::WARNING_CIRCLE, e.clone()),
    };
    div()
        .flex()
        .items_start()
        .gap(px(6.))
        .text_size(px(size::SM))
        .line_height(px(18.))
        .text_color(color)
        .child(div().pt(px(2.)).flex_none().child(icon_ph(glyph, color, icon_px::SM)))
        .child(div().min_w_0().child(text))
}

fn problem_line(p: &'static str) -> Div {
    div().flex().items_center().gap(px(6.)).text_size(px(size::SM)).text_color(Tokens::warning()).child(icon_ph(ph::WARNING, Tokens::warning(), icon_px::SM)).child(p)
}

fn clipboard_text(cx: &App) -> Option<String> {
    cx.read_from_clipboard().and_then(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Paste a plugin link (GitHub repo or `manifest.json`), install it with a visible result line.
pub struct PluginLinkBox {
    input: Entity<InputState>,
    installing: bool,
    result: Option<Result<String, String>>,
    /// Plugins installed through this box since it opened.
    installed: usize,
    _sub: Subscription,
}

impl EventEmitter<SourcesEvent> for PluginLinkBox {}

impl PluginLinkBox {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Plugin link (GitHub repo or manifest.json)"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::PressEnter { .. } => this.install(cx),
            InputEvent::Change => {
                this.result = None;
                cx.notify();
            }
            _ => {}
        });
        Self { input, installing: false, result: None, installed: 0, _sub: sub }
    }

    pub fn is_busy(&self) -> bool {
        self.installing
    }

    /// Plugins installed through this box since it opened.
    pub fn installed_count(&self) -> usize {
        self.installed
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = clipboard_text(cx) {
            self.input.update(cx, |s, cx| s.set_value(t, window, cx));
        }
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let url = self.input.read(cx).value().trim().to_string();
        if url.is_empty() || self.installing || url_problem(&url).is_some() {
            return;
        }
        self.installing = true;
        self.result = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let m = mv_plugins::repo::install_plugin_from_url(&url, &backend::plugins_dir(), backend::plugin_backend()).await?;
                backend::set_source_enabled(&m.id, true)?;
                backend::reload_registry().await;
                anyhow::Ok(m)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.installing = false;
                match res {
                    Ok(m) => {
                        this.installed += 1;
                        this.result = Some(Ok(format!("Installed {} {}. It is switched on in Browse.", m.name, m.version)));
                        cx.emit(SourcesEvent::Changed);
                    }
                    Err(e) => this.result = Some(Err(format!("Could not install it: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for PluginLinkBox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url = self.input.read(cx).value().to_string();
        let problem = url_problem(&url);
        let empty = url.trim().is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.input).cleanable(true).prefix(icon_ph(ph::LINK_SIMPLE, Tokens::text_3(), icon_px::SM))))
                    .child(icon_button("plugin-paste", ph::CLIPBOARD_TEXT, "Paste link").on_click(cx.listener(|this, _, w, cx| this.paste(w, cx))))
                    .child(
                        btn("plugin-install", if self.installing { "Installing" } else { "Install" }, ButtonKind::Secondary)
                            .icon(ph::PACKAGE)
                            .loading(self.installing)
                            .map(|b| match (empty, problem) {
                                (true, _) => b.disabled("Paste a plugin link first"),
                                (false, Some(p)) => b.disabled(p),
                                _ => b,
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
                    ),
            )
            .when_some(problem.filter(|_| !empty), |d, p| d.child(problem_line(p)))
            .when_some(self.result.as_ref(), |d, r| d.child(result_line(r)))
    }
}

/// How many extension rows show before "Show more" (a repository lists over a thousand).
const PAGE: usize = 40;

/// Language names for the codes repositories use; anything else shows its code.
fn lang_name(code: &str) -> String {
    let name = match code {
        "all" => "Multi-language",
        "en" => "English",
        "es" | "es-419" => "Spanish",
        "pt" | "pt-BR" => "Portuguese",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        "id" => "Indonesian",
        "ru" => "Russian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "zh" | "zh-Hans" | "zh-Hant" => "Chinese",
        "ar" => "Arabic",
        "tr" => "Turkish",
        "vi" => "Vietnamese",
        "th" => "Thai",
        "pl" => "Polish",
        "hi" => "Hindi",
        _ => return code.to_uppercase(),
    };
    name.to_string()
}

/// The source of `ext` to install: the one in the preferred chapter language, else the first.
pub fn preferred_source_index(ext: &RepoExtension, lang: &str) -> usize {
    ext.sources.iter().position(|s| s.lang == lang).unwrap_or(0)
}

/// Every plugin id `ext` can install as (one per source).
fn plugin_ids(ext: &RepoExtension) -> Vec<String> {
    (0..ext.sources.len().max(1)).map(|i| mv_plugins::repo::repo_plugin_id(ext, i)).collect()
}

/// The built-in source that reads `ext`'s site (MangaDex, WEBTOON, ...): installing the extension
/// adds that source instead of generating a scraper.
fn builtin_of(ext: &RepoExtension) -> Option<&'static str> {
    ext.sources.iter().find_map(|s| mv_sources::builtin_for_host(&s.base_url))
}

/// Extensions matching the search text, language and 18+ choice, in index order.
pub fn filter_extensions<'a>(exts: &'a [RepoExtension], query: &str, lang: Option<&str>, nsfw: bool) -> Vec<&'a RepoExtension> {
    let q = query.trim().to_lowercase();
    exts.iter()
        .filter(|e| nsfw || e.nsfw == 0)
        .filter(|e| match lang {
            Some(l) => e.lang == l || e.sources.iter().any(|s| s.lang == l),
            None => true,
        })
        .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.pkg.to_lowercase().contains(&q) || e.sources.iter().any(|s| s.name.to_lowercase().contains(&q)))
        .collect()
}

/// Short name for a repository link: `owner/repo` on GitHub, else the host.
pub fn repo_label(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["raw.githubusercontent.com", owner, repo, ..] | ["github.com", owner, repo, ..] => format!("{owner}/{repo}"),
        [host, ..] => host.to_string(),
        [] => url.to_string(),
    }
}

/// Add an extension repository (Mihon/Tachiyomi index), list its extensions with search,
/// language and 18+ filters, and install or update them. Repositories are saved in
/// `<data>/repos.json`. Mihon's add-repository flow:
/// <https://mihon.app/docs/guides/getting-started>
pub struct RepoBrowser {
    url: Entity<InputState>,
    search: Entity<InputState>,
    adding: bool,
    add_error: Option<String>,
    clear_url: bool,
    repos: Vec<String>,
    current: Option<String>,
    exts: Option<Result<Vec<RepoExtension>, String>>,
    lang: Option<String>,
    show_nsfw: bool,
    /// Installed repository plugin id -> version.
    installed: HashMap<String, String>,
    /// Built-in sources that are added (an extension for their site shows as installed).
    builtins_on: HashSet<String>,
    /// [`backend::sources_version`] `installed` was read at.
    seen_sources: u64,
    updates: Vec<RepoUpdate>,
    /// Extension packages being installed or updated.
    busy: HashSet<String>,
    limit: usize,
    confirm_remove: Option<String>,
    /// Bumped per load, so a slow answer for a repository left meanwhile is dropped.
    serial: u64,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SourcesEvent> for RepoBrowser {}

impl RepoBrowser {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx).placeholder("Repository link (GitHub repo or index.min.json)"));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search extensions"));
        let subs = vec![
            cx.subscribe_in(&url, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } => this.add(cx),
                InputEvent::Change => {
                    this.add_error = None;
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&search, |this, _, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.limit = PAGE;
                    cx.notify();
                }
            }),
        ];
        let repos = mv_plugins::repo::list_repos(&backend::repos_root());
        let mut b = Self {
            url,
            search,
            adding: false,
            add_error: None,
            clear_url: false,
            current: repos.first().cloned(),
            repos,
            exts: None,
            lang: None,
            show_nsfw: false,
            installed: HashMap::new(),
            builtins_on: HashSet::new(),
            seen_sources: 0,
            updates: vec![],
            busy: HashSet::new(),
            limit: PAGE,
            confirm_remove: None,
            serial: 0,
            _subs: subs,
        };
        b.refresh_installed(cx);
        if let Some(u) = b.current.clone() {
            b.load(u, cx);
        }
        b
    }

    /// Saved repositories.
    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    /// Extensions installed from any repository.
    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    pub fn is_busy(&self) -> bool {
        self.adding || !self.busy.is_empty()
    }

    pub fn refresh_installed(&mut self, cx: &mut Context<Self>) {
        self.seen_sources = backend::sources_version();
        self.installed = mv_plugins::repo::installed_repo_plugins(&backend::plugins_dir()).into_iter().map(|p| (p.manifest.id, p.manifest.version)).collect();
        self.builtins_on = backend::installed_sources(backend::builtin_sources()).into_iter().map(|s| s.id).collect();
        cx.notify();
    }

    /// Uninstalls every source of `ext` (or takes its built-in source out of Browse).
    fn remove_ext(&mut self, ext: RepoExtension, cx: &mut Context<Self>) {
        let mut ids: Vec<String> = plugin_ids(&ext).into_iter().filter(|id| self.installed.contains_key(id)).collect();
        ids.extend(builtin_of(&ext).filter(|b| self.builtins_on.contains(*b)).map(String::from));
        if ids.is_empty() || !self.busy.insert(ext.pkg.clone()) {
            return;
        }
        cx.notify();
        let name = display_name(&ext);
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                for id in &ids {
                    backend::remove_source(id).await?;
                }
                anyhow::Ok(())
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&ext.pkg);
                this.refresh_installed(cx);
                match res {
                    Ok(()) => toast::show(cx, Toast::success(format!("Removed {name}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("Could not remove {name}: {e:#}"))),
                }
                cx.emit(SourcesEvent::Changed);
            });
        })
        .detach();
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = clipboard_text(cx) {
            self.url.update(cx, |s, cx| s.set_value(t, window, cx));
        }
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        if url.is_empty() || self.adding {
            return;
        }
        self.adding = true;
        self.add_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let b = backend::plugin_backend();
                mv_plugins::repo::add_repo_verified(&backend::repos_root(), &url, b.as_ref()).await
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.adding = false;
                match res {
                    Ok((stored, exts)) => {
                        this.repos = mv_plugins::repo::list_repos(&backend::repos_root());
                        this.serial += 1;
                        toast::show(cx, Toast::success(format!("Added {} · {} extensions", repo_label(&stored), exts.len())));
                        this.current = Some(stored);
                        this.exts = Some(Ok(exts));
                        this.reset_filters();
                        this.clear_url = true;
                        this.check_updates(cx);
                    }
                    Err(e) => this.add_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn reset_filters(&mut self) {
        self.limit = PAGE;
        self.lang = None;
    }

    fn select(&mut self, url: String, cx: &mut Context<Self>) {
        if self.current.as_deref() == Some(url.as_str()) {
            return;
        }
        self.current = Some(url.clone());
        self.confirm_remove = None;
        self.reset_filters();
        self.load(url, cx);
    }

    fn load(&mut self, url: String, cx: &mut Context<Self>) {
        self.serial += 1;
        let serial = self.serial;
        self.exts = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let b = backend::plugin_backend();
                mv_plugins::repo::fetch_repo_extensions(&url, b.as_ref()).await
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if this.serial != serial {
                    return;
                }
                this.exts = Some(res.map_err(|e| format!("{e:#}")));
                this.check_updates(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Looks for newer versions of installed extensions in every saved repository. No network
    /// is used when nothing from a repository is installed.
    fn check_updates(&mut self, cx: &mut Context<Self>) {
        if self.installed.is_empty() {
            self.updates.clear();
            return;
        }
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let found = mv_ui::bg::run(async move {
                let b = backend::plugin_backend();
                mv_plugins::repo::check_repo_updates(&backend::plugins_dir(), &repos, b.as_ref()).await
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.updates = found;
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_repo(&mut self, url: String, cx: &mut Context<Self>) {
        self.confirm_remove = None;
        match mv_plugins::repo::remove_repo(&backend::repos_root(), &url) {
            Ok(_) => {
                self.repos = mv_plugins::repo::list_repos(&backend::repos_root());
                toast::show(cx, Toast::success("Removed the repository. Extensions you installed from it stay installed."));
                if self.current.as_deref() == Some(url.as_str()) {
                    self.current = self.repos.first().cloned();
                    self.exts = None;
                    self.serial += 1;
                    if let Some(u) = self.current.clone() {
                        self.load(u, cx);
                    }
                }
            }
            Err(e) => toast::show(cx, Toast::error(format!("Could not remove the repository: {e:#}"))),
        }
        cx.notify();
    }

    fn install(&mut self, ext: RepoExtension, index: usize, update: bool, cx: &mut Context<Self>) {
        let name = display_name(&ext);
        if let Some(builtin) = builtin_of(&ext) {
            // MangaVox reads this site itself, far better than a generated scraper could.
            match backend::set_source_enabled(builtin, true) {
                Ok(()) => toast::show(cx, Toast::success(format!("Installed {name}. It uses MangaVox's built-in {name} reader."))),
                Err(e) => toast::show(cx, Toast::error(format!("Could not install {name}: {e:#}"))),
            }
            self.refresh_installed(cx);
            cx.emit(SourcesEvent::Changed);
            return;
        }
        if !self.busy.insert(ext.pkg.clone()) {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let e = ext.clone();
            let res = mv_ui::bg::run(async move {
                let r = mv_plugins::repo::install_repo_extension(&e, index, &backend::plugins_dir(), backend::plugin_backend()).await?;
                // A switch saved for an earlier install of it must not hide it from Browse.
                backend::set_source_enabled(&r.id, true)?;
                backend::reload_registry().await;
                anyhow::Ok(r)
            })
            .await
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&ext.pkg);
                match res {
                    Ok(r) => {
                        this.updates.retain(|u| u.extension.pkg != ext.pkg);
                        this.refresh_installed(cx);
                        match r.warning {
                            Some(w) => toast::show(cx, Toast::warning(format!("Installed {name}, but its first page came back empty ({w}). Use Test in Settings › Sources to check it."))),
                            None => toast::show(cx, Toast::success(format!("{} {name}", if update { "Updated" } else { "Installed" }))),
                        }
                        cx.emit(SourcesEvent::Changed);
                    }
                    Err(e) => toast::show(cx, Toast::error(format!("Could not install {name}: {e:#}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn repo_chip(&self, i: usize, url: &str, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.current.as_deref() == Some(url);
        let (sel, rm) = (url.to_string(), url.to_string());
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(filter_chip(("repo", i), repo_label(url), selected).on_click(cx.listener(move |this, _, _, cx| this.select(sel.clone(), cx))))
            .child(icon_button(("repo-rm", i), ph::X, "Remove repository").small().on_click(cx.listener(move |this, _, _, cx| {
                this.confirm_remove = Some(rm.clone());
                cx.notify();
            })))
            .into_any_element()
    }

    fn ext_row(&self, i: usize, ext: &RepoExtension, repo: &str, pref_lang: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let installed = plugin_ids(ext).iter().any(|id| self.installed.contains_key(id)) || builtin_of(ext).is_some_and(|b| self.builtins_on.contains(b));
        let update = self.updates.iter().find(|u| u.extension.pkg == ext.pkg).cloned();
        let busy = self.busy.contains(&ext.pkg);
        let name = display_name(ext);
        let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let icon_url = mv_plugins::repo::extension_icon_url(repo, ext);
        let sources = ext.sources.len();
        let action: AnyElement = match (&update, installed) {
            (Some(u), _) => {
                let (e, ix) = (u.extension.clone(), u.source_index);
                btn(("ext-update", i), "Update", ButtonKind::Secondary)
                    .small()
                    .icon(ph::ARROW_CIRCLE_UP)
                    .loading(busy)
                    .tooltip(format!("v{} → v{}", u.installed_version, u.available_version))
                    .on_click(cx.listener(move |this, _, _, cx| this.install(e.clone(), ix, true, cx)))
                    .into_any_element()
            }
            (None, true) => {
                let e = ext.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(status_tag(StatusKind::Done).label("Installed"))
                    .child(
                        btn(("ext-remove", i), if busy { "Removing" } else { "Remove" }, ButtonKind::DangerGhost)
                            .small()
                            .icon(glyphs::DELETE)
                            .loading(busy)
                            .tooltip("Uninstall: delete it from this computer and Browse")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_ext(e.clone(), cx))),
                    )
                    .into_any_element()
            }
            (None, false) => {
                let e = ext.clone();
                let ix = preferred_source_index(ext, pref_lang);
                btn(("ext-install", i), if busy { "Installing" } else { "Install" }, ButtonKind::Secondary)
                    .small()
                    .icon(ph::DOWNLOAD_SIMPLE)
                    .loading(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.install(e.clone(), ix, false, cx)))
                    .into_any_element()
            }
        };
        div()
            .id(("ext", i))
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(10.))
            .when(i > 0, |d| d.border_t_1().border_color(Tokens::border()))
            .child(
                // The letter shows until (or unless) the icon loads over it.
                div()
                    .relative()
                    .size(px(32.))
                    .flex_none()
                    .rounded(px(radius::SM))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(Tokens::surface_3())
                    .text_size(px(size::XS))
                    .font_weight(FontWeight::BOLD)
                    .text_color(Tokens::text_2())
                    .child(letter)
                    .child(div().absolute().inset_0().child(img(SharedString::from(icon_url)).size_full().object_fit(ObjectFit::Cover))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().truncate().text_size(px(size::MD)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(name))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(tag(lang_name(&ext.lang), Tone::Neutral))
                            .when(ext.nsfw != 0, |d| d.child(tag("18+", Tone::Danger)))
                            .child(muted(if sources > 1 { format!("v{} · {sources} sites", ext.version) } else { format!("v{}", ext.version) })),
                    ),
            )
            .child(action)
    }

    fn extension_list(&self, repo: &str, exts: &[RepoExtension], cx: &mut Context<Self>) -> AnyElement {
        let query = self.search.read(cx).value().to_string();
        let shown = filter_extensions(exts, &query, self.lang.as_deref(), self.show_nsfw);
        let hidden_nsfw = if self.show_nsfw { 0 } else { filter_extensions(exts, &query, self.lang.as_deref(), true).len() - shown.len() };
        // Languages, most extensions first.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for e in exts {
            *counts.entry(e.lang.as_str()).or_default() += 1;
        }
        let mut langs: Vec<(&str, usize)> = counts.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let weak = cx.entity().downgrade();
        let pick = |code: Option<String>| {
            let weak = weak.clone();
            move |_: &mut Window, cx: &mut App| {
                let code = code.clone();
                let _ = weak.update(cx, |this, cx| {
                    this.lang = code;
                    this.limit = PAGE;
                    cx.notify();
                });
            }
        };
        let mut items = vec![MenuItem::new("All languages", pick(None)).checked(self.lang.is_none()), MenuItem::separator()];
        items.extend(langs.iter().map(|(code, n)| MenuItem::new(format!("{} ({n})", lang_name(code)), pick(Some(code.to_string()))).checked(self.lang.as_deref() == Some(*code))));
        let lang_label = self.lang.as_deref().map(lang_name).unwrap_or_else(|| "All languages".into());
        let pref = backend::preferred_language();
        let total = shown.len();
        let rows: Vec<Stateful<Div>> = shown.iter().take(self.limit).enumerate().map(|(i, e)| self.ext_row(i, e, repo, &pref, cx)).collect();
        let more = total.saturating_sub(self.limit);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w(px(200.)).child(Input::new(&self.search).cleanable(true).prefix(icon_ph(ph::MAGNIFYING_GLASS, Tokens::text_3(), icon_px::SM))))
                    .child(menu_button("ext-lang", ph::GLOBE, "Language", items).label(lang_label))
                    .child(switch("ext-nsfw", self.show_nsfw).label("Show 18+").on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.show_nsfw = *on;
                        this.limit = PAGE;
                        cx.notify();
                    }))),
            )
            .child(muted(match hidden_nsfw {
                0 => format!("{total} of {} extensions", exts.len()),
                n => format!("{total} of {} extensions · {n} 18+ hidden", exts.len()),
            }))
            .child(if rows.is_empty() {
                empty_view(ph::MAGNIFYING_GLASS, "No extensions match", "Try another name or language.").compact().into_any_element()
            } else {
                div().flex().flex_col().children(rows).into_any_element()
            })
            .when(more > 0, |d| {
                d.child(btn("ext-more", format!("Show {} more", more.min(PAGE)), ButtonKind::Ghost).small().icon(ph::CARET_DOWN).on_click(cx.listener(|this, _, _, cx| {
                    this.limit += PAGE;
                    cx.notify();
                })))
            })
            .into_any_element()
    }
}

fn display_name(ext: &RepoExtension) -> String {
    ext.name.trim_start_matches("Tachiyomi: ").to_string()
}

impl Render for RepoBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.clear_url) {
            self.url.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if backend::sources_version() != self.seen_sources {
            self.refresh_installed(cx);
        }
        let url = self.url.read(cx).value().to_string();
        let empty = url.trim().is_empty();
        let add_box = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(Input::new(&self.url).cleanable(true).prefix(icon_ph(ph::GIT_BRANCH, Tokens::text_3(), icon_px::SM))))
            .child(icon_button("repo-paste", ph::CLIPBOARD_TEXT, "Paste link").on_click(cx.listener(|this, _, w, cx| this.paste(w, cx))))
            .child(
                btn("repo-add", if self.adding { "Checking" } else { "Add" }, ButtonKind::Secondary)
                    .icon(ph::PLUS)
                    .loading(self.adding)
                    .map(|b| if empty { b.disabled("Paste a repository link first") } else { b })
                    .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
            );

        let mut col = div().flex().flex_col().gap(px(12.)).child(add_box);
        if let Some(e) = &self.add_error {
            col = col.child(result_line(&Err(e.clone())));
        }
        if self.repos.is_empty() {
            return col;
        }
        let repos = self.repos.clone();
        col = col.child(div().flex().flex_wrap().items_center().gap(px(6.)).children(repos.iter().enumerate().map(|(i, u)| self.repo_chip(i, u, cx))));
        if let Some(rm) = self.confirm_remove.clone() {
            let keep = rm.clone();
            col = col.child(
                confirm_inline(format!("Remove {}? Extensions you installed from it stay installed.", repo_label(&rm)), "Remove repository", true)
                    .on_confirm(cx.listener(move |this, _, _, cx| this.remove_repo(keep.clone(), cx)))
                    .on_cancel(cx.listener(|this, _, _, cx| {
                        this.confirm_remove = None;
                        cx.notify();
                    })),
            );
        }
        let Some(repo) = self.current.clone() else { return col };
        let list: AnyElement = match &self.exts {
            None => div().flex().flex_col().children((0..4).map(|_| row_skeleton())).into_any_element(),
            Some(Err(e)) => {
                let u = repo.clone();
                error_view(e.clone()).title("Could not load this repository").compact().retry(cx.listener(move |this, _, _, cx| this.load(u.clone(), cx))).into_any_element()
            }
            Some(Ok(_)) => {
                // Moved out while the rows render (no copy of a 1,000-entry list per frame).
                let exts = match self.exts.take() {
                    Some(Ok(e)) => e,
                    _ => vec![],
                };
                let el = self.extension_list(&repo, &exts, cx);
                self.exts = Some(Ok(exts));
                el
            }
        };
        col.child(list)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in gpui's `test` macro, which shadows the built-in one.
    use super::{filter_extensions, lang_name, plugin_ids, preferred_source_index, repo_label};
    use mv_plugins::repo::RepoExtension;
    use mv_plugins::repo::RepoSource;

    fn ext(name: &str, lang: &str, nsfw: i64, sources: &[(&str, &str)]) -> RepoExtension {
        RepoExtension {
            name: name.into(),
            pkg: format!("eu.kanade.tachiyomi.extension.{lang}.{}", name.to_lowercase()),
            lang: lang.into(),
            nsfw,
            version: "1.4.1".into(),
            sources: sources.iter().map(|(n, l)| RepoSource { name: n.to_string(), lang: l.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn extension_filters_hide_18_plus_by_default() {
        let exts = vec![ext("Asura", "en", 0, &[("Asura Scans", "en")]), ext("Hentai", "en", 1, &[("H", "en")]), ext("Multi", "all", 0, &[("Multi", "en"), ("Multi", "es")])];
        assert_eq!(filter_extensions(&exts, "", None, false).len(), 2);
        assert_eq!(filter_extensions(&exts, "", None, true).len(), 3);
        // Language matches the extension or any of its sources.
        assert_eq!(filter_extensions(&exts, "", Some("es"), false).iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["Multi"]);
        // Search matches names, packages and source names.
        assert_eq!(filter_extensions(&exts, "scans", None, false).len(), 1);
        assert_eq!(filter_extensions(&exts, "  ASURA ", None, false).len(), 1);
    }

    #[test]
    fn installs_the_source_in_the_reading_language() {
        let e = ext("Multi", "all", 0, &[("Multi", "en"), ("Multi", "es")]);
        assert_eq!(preferred_source_index(&e, "es"), 1);
        assert_eq!(preferred_source_index(&e, "ja"), 0);
        assert_eq!(plugin_ids(&e).len(), 2);
        assert_eq!(plugin_ids(&ext("None", "en", 0, &[])).len(), 1);
    }

    #[test]
    fn repository_labels_are_short() {
        assert_eq!(repo_label("https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json"), "keiyoushi/extensions");
        assert_eq!(repo_label("https://example.org/repo/index.min.json"), "example.org");
        assert_eq!(lang_name("pt-BR"), "Portuguese");
        assert_eq!(lang_name("xx"), "XX");
    }
}
