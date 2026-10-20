//! Settings → Model Hub: curated picks, Hugging Face search, quant picker with a fit rating,
//! resumable downloads, Ollama import and a downloaded-models manager.
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{AiProviderId, AiProviderState, HubConfigPatch, HubDownload, HubDownloadState, HubModelDetail, HubModelFile, HubModelSummary, HubSystemFit, LocalModelFile, ModelHub, SearchOpts};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::toast::{self, Toast};
use mv_ui::{
    appear, btn, confirm_inline, disclosure, empty_view, error_view, external_link, filter_chip, fmt_bytes, glyphs, icon_button, icon_tag, inline_alert, list_row, ph, ph_icon, popup_surface, progress_bar, section_label, segmented, size, skeleton, skeleton_block,
    skeleton_line, skeleton_row, skeleton_text, tag, tip, AlertKind, ButtonKind, Hue, Ph, Route, Tokens, Tone, Weight,
};

use crate::fit::{detect_system, fit_for, infer_tags, params_from_name, pick_recommended, quant_group, Filter, Fit};
use crate::shared::{shared, Shared};
use crate::ui::{field, fmt_count, muted, password_state, search_prefix, set_value, text_state, value_of};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Picks,
    Hf,
    Local,
}

const TABS: [Tab; 3] = [Tab::Picks, Tab::Hf, Tab::Local];

struct Pick {
    title: &'static str,
    blurb: &'static str,
    query: &'static str,
    tags: &'static [Filter],
}

const PICKS: &[Pick] = &[
    Pick { title: "Qwen3.5 4B", blurb: "Small and quick — runs on almost any PC, even without a graphics card.", query: "Qwen3.5-4B", tags: &[Filter::Reasoning] },
    Pick { title: "Qwen3.5 9B", blurb: "A capable everyday assistant that fits on most gaming GPUs.", query: "Qwen3.5-9B", tags: &[Filter::Reasoning] },
    Pick { title: "Gemma 4 12B", blurb: "Google's mid-size all-rounder that can also read pictures.", query: "gemma-4-12b", tags: &[Filter::Vision] },
    Pick { title: "gpt-oss 20B", blurb: "OpenAI's open model, strong at reasoning and tool use.", query: "gpt-oss-20b", tags: &[Filter::Reasoning, Filter::Moe] },
    Pick { title: "Qwen3.6 35B A3B", blurb: "Mixture-of-experts: big-model quality at small-model speed.", query: "Qwen3.6-35B-A3B", tags: &[Filter::Moe, Filter::Coding] },
    Pick { title: "GLM-4.7 Flash", blurb: "Fast mixture-of-experts model tuned for agents and coding.", query: "GLM-4.7-Flash", tags: &[Filter::Moe, Filter::Coding] },
    Pick { title: "DeepSeek R1 Distill 7B", blurb: "Step-by-step reasoning distilled into a small model.", query: "DeepSeek-R1-Distill-Qwen-7B", tags: &[Filter::Reasoning, Filter::Distilled] },
    Pick { title: "Dolphin 3.0 8B", blurb: "Uncensored general assistant that follows any instruction.", query: "Dolphin3.0-Llama3.1-8B", tags: &[Filter::Uncensored] },
    Pick { title: "Qwen3 8B Abliterated", blurb: "Refusals removed from Qwen3 8B — fully uncensored.", query: "Qwen3-8B-abliterated", tags: &[Filter::Uncensored, Filter::Reasoning] },
    Pick { title: "Llama 3.2 3B", blurb: "Tiny and fast for quick answers on low-end machines.", query: "Llama-3.2-3B-Instruct", tags: &[] },
];

const TRUSTED: &[&str] = &["unsloth", "bartowski", "lmstudio-community", "ggml-org", "mradermacher", "QuantFactory"];
const OLLAMA_URL: &str = "https://ollama.com/download";

/// Choose the repo from a trusted quantizer, else the most downloaded.
pub fn resolve_pick(results: &[HubModelSummary]) -> Option<&HubModelSummary> {
    results.iter().find(|m| is_trusted(&m.author)).or_else(|| results.first())
}

/// A quantizer whose GGUF builds the hub trusts (the seal on a row says so).
fn is_trusted(author: &str) -> bool {
    TRUSTED.iter().any(|t| t.eq_ignore_ascii_case(author))
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

pub struct ModelHubPanel {
    hub: ModelHub,
    shared: Entity<Shared>,
    tab: Tab,
    filters: HashSet<Filter>,
    filters_open: bool,
    /// When an outside click closed the filter popover, so the same click on its button doesn't reopen it.
    filters_closed_at: Option<Instant>,
    /// The open filter popover takes the keyboard (Esc closes it); the previous focus returns after.
    filters_focus: FocusHandle,
    filters_restore: Option<FocusHandle>,
    query: Entity<InputState>,
    url: Entity<InputState>,
    token: Entity<InputState>,
    picks: Vec<Option<HubModelSummary>>,
    picks_loaded: bool,
    hf: Vec<HubModelSummary>,
    hf_loaded: bool,
    hf_loading: bool,
    hf_error: Option<String>,
    selected: Option<String>,
    selected_title: Option<String>,
    detail: Option<Result<HubModelDetail, String>>,
    show_all: bool,
    system: Option<HubSystemFit>,
    downloads: Vec<HubDownload>,
    polling: bool,
    local: Vec<LocalModelFile>,
    /// Path of the file being added to Ollama.
    importing: Option<String>,
    /// Path of the downloaded file waiting for "Delete file" / "Keep".
    confirm_delete: Option<String>,
    has_token: bool,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    let shared = shared(cx);
    let hub = shared.read(cx).hub.clone();
    // The Downloaded tab offers "Use in chat" only when Ollama is known to be ready.
    shared.update(cx, |s, cx| s.refresh_providers(false, cx));
    cx.new(|cx| {
        let query = text_state("Search Hugging Face (GGUF models)", window, cx);
        let url = text_state("Paste a direct .gguf link", window, cx);
        let token = password_state("Hugging Face token (for gated repos)", window, cx);
        let subs = vec![
            cx.subscribe_in(&query, window, |this: &mut ModelHubPanel, _, ev: &InputEvent, _w, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.search(cx);
                }
            }),
            cx.subscribe_in(&url, window, |this: &mut ModelHubPanel, _, ev: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.download_url(w, cx);
                }
            }),
            cx.observe(&shared, |_, _, cx| cx.notify()),
        ];
        let mut p = ModelHubPanel {
            has_token: hub.hub_config_public().has_token,
            hub,
            shared,
            tab: Tab::Picks,
            filters: HashSet::new(),
            filters_open: false,
            filters_closed_at: None,
            filters_focus: cx.focus_handle(),
            filters_restore: None,
            query,
            url,
            token,
            picks: vec![None; PICKS.len()],
            picks_loaded: false,
            hf: vec![],
            hf_loaded: false,
            hf_loading: false,
            hf_error: None,
            selected: None,
            selected_title: None,
            detail: None,
            show_all: false,
            system: None,
            downloads: vec![],
            polling: false,
            local: vec![],
            importing: None,
            confirm_delete: None,
            _subs: subs,
        };
        p.boot(cx);
        p
    })
    .into()
}

impl ModelHubPanel {
    fn boot(&mut self, cx: &mut Context<Self>) {
        // Hardware numbers for the fit rating.
        cx.spawn(async move |this, cx| {
            let sys = mv_ui::bg::blocking(detect_system).await.ok();
            let _ = this.update(cx, |t, cx| {
                t.system = sys;
                cx.notify();
            });
        })
        .detach();
        self.load_picks(cx);
        self.refresh_local(cx);
        self.start_polling(cx);
    }

    /// Resolve the curated picks live (also the Retry when Hugging Face was unreachable).
    fn load_picks(&mut self, cx: &mut Context<Self>) {
        self.picks_loaded = false;
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::run(async move {
                let futs = PICKS.iter().map(|p| {
                    let hub = hub.clone();
                    async move {
                        let r = hub.search_hub(p.query, &SearchOpts { limit: 12, ..Default::default() }).await.unwrap_or_default();
                        resolve_pick(&r).cloned()
                    }
                });
                futures::future::join_all(futs).await
            })
            .await
            .unwrap_or_default();
            let _ = this.update(cx, |t, cx| {
                if res.len() == PICKS.len() {
                    t.picks = res;
                }
                t.picks_loaded = true;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn set_tab(&mut self, t: Tab, cx: &mut Context<Self>) {
        self.tab = t;
        self.filters_open = false;
        if t == Tab::Hf && !self.hf_loaded && !self.hf_loading {
            self.search(cx);
        }
        if t == Tab::Local {
            self.refresh_local(cx);
            self.shared.update(cx, |s, cx| s.refresh_providers(false, cx));
        }
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Hf;
        self.hf_loading = true;
        self.hf_error = None;
        let q = value_of(&self.query, cx);
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.search_hub(&q, &SearchOpts::default()).await.map_err(|e| e.to_string()) }).await;
            let _ = this.update(cx, |t, cx| {
                t.hf_loading = false;
                t.hf_loaded = true;
                match r {
                    Ok(Ok(list)) => t.hf = list,
                    Ok(Err(e)) => t.hf_error = Some(e),
                    Err(e) => t.hf_error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select(&mut self, repo: String, title: Option<String>, cx: &mut Context<Self>) {
        self.selected = Some(repo.clone());
        self.selected_title = title;
        self.detail = None;
        self.show_all = false;
        let hub = self.hub.clone();
        let r2 = repo.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.hub_detail(&r2).await.map_err(|e| e.to_string()) }).await;
            let _ = this.update(cx, |t, cx| {
                if t.selected.as_deref() == Some(repo.as_str()) {
                    t.detail = Some(match r {
                        Ok(r) => r,
                        Err(e) => Err(e.to_string()),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_local(&mut self, cx: &mut Context<Self>) {
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let list = mv_ui::bg::blocking(move || hub.list_local_models()).await.unwrap_or_default();
            let _ = this.update(cx, |t, cx| {
                t.local = list;
                cx.notify();
            });
        })
        .detach();
    }

    /// Poll the hub's download table while anything is active.
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| loop {
            let list = hub.list_downloads();
            let active = list.iter().any(|d| d.state.is_active());
            let finished = this
                .update(cx, |t, cx| {
                    let was_active = t.downloads.iter().any(|d| d.state.is_active());
                    // A download that just finished: say so once.
                    for d in &list {
                        let before = t.downloads.iter().find(|o| o.id == d.id).map(|o| o.state.is_active()).unwrap_or(false);
                        if before && d.state == HubDownloadState::Done {
                            toast::show(cx, Toast::success(format!("Downloaded {}", d.file)).icon(glyphs::DOWNLOAD));
                        }
                    }
                    t.downloads = list;
                    if was_active && !active {
                        t.refresh_local(cx);
                    }
                    if !active {
                        t.polling = false;
                    }
                    cx.notify();
                    !active
                })
                .unwrap_or(true);
            if finished {
                break;
            }
            cx.background_executor().timer(Duration::from_millis(500)).await;
        })
        .detach();
    }

    fn download_file(&mut self, repo: String, file: HubModelFile, cx: &mut Context<Self>) {
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.download_hub_file(&repo, &file, Arc::new(|_| {})).map_err(|e| e.to_string()) }).await;
            let _ = this.update(cx, |t, cx| {
                match r {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => toast::show(cx, Toast::error(format!("Couldn't start the download: {e}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("Couldn't start the download: {e}"))),
                }
                t.start_polling(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn download_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = value_of(&self.url, cx).trim().to_string();
        if url.is_empty() {
            return;
        }
        set_value(&self.url, "", window, cx);
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.download_url(&url, Arc::new(|_| {})).map_err(|e| e.to_string()) }).await;
            let _ = this.update(cx, |t, cx| {
                match r {
                    Ok(Ok(_)) => {
                        t.tab = Tab::Local;
                        toast::show(cx, Toast::info("Download started"));
                    }
                    Ok(Err(e)) => toast::show(cx, Toast::error(format!("Couldn't download that link: {e}"))),
                    Err(e) => toast::show(cx, Toast::error(format!("Couldn't download that link: {e}"))),
                }
                t.start_polling(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn import(&mut self, path: String, cx: &mut Context<Self>) {
        self.importing = Some(path.clone());
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.import_into_ollama(std::path::Path::new(&path), None).await }).await;
            let _ = this.update(cx, |t, cx| {
                t.importing = None;
                match r {
                    Ok(r) if r.ok => toast::show(cx, Toast::success(format!("Added to Ollama as {}", r.name.unwrap_or_default()))),
                    Ok(r) => toast::show(cx, Toast::error(r.error.unwrap_or_else(|| "Couldn't add the model to Ollama".into()))),
                    Err(e) => toast::show(cx, Toast::error(e.to_string())),
                }
                t.refresh_local(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Points the assistant at an Ollama model and says so.
    fn use_in_chat(&mut self, name: String, cx: &mut Context<Self>) {
        if crate::set_assistant_model("ollama", &name, cx) {
            toast::show(cx, Toast::success(format!("The assistant now uses {name}")).icon(glyphs::ASSISTANT));
        }
        cx.notify();
    }

    fn delete_local(&mut self, path: String, name: String, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::blocking(move || hub.delete_local_model(std::path::Path::new(&path))).await;
            let _ = this.update(cx, |t, cx| {
                match r {
                    Ok(true) => toast::show(cx, Toast::success(format!("Deleted {name}")).icon(glyphs::DELETE)),
                    Ok(false) => toast::show(cx, Toast::error(format!("Couldn't delete {name}. It may be open in another app."))),
                    Err(e) => toast::show(cx, Toast::error(format!("Couldn't delete {name}: {e}"))),
                }
                t.refresh_local(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn save_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = value_of(&self.token, cx).trim().to_string();
        let res = self.hub.set_hub_config(&HubConfigPatch { hf_token: Some(t), ..Default::default() });
        set_value(&self.token, "", window, cx);
        self.has_token = self.hub.hub_config_public().has_token;
        match res {
            Ok(_) => toast::show(cx, Toast::success(if self.has_token { "Hugging Face token saved" } else { "Hugging Face token removed" })),
            Err(e) => toast::show(cx, Toast::error(format!("Couldn't save the token: {e}"))),
        }
        cx.notify();
    }

    fn passes(&self, repo: &str, tags: &[String], pipeline: Option<&str>, extra: &[Filter]) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        let mut have = infer_tags(repo, tags, pipeline);
        have.extend_from_slice(extra);
        self.filters.iter().all(|f| match f {
            // ~0.6 GB per billion parameters at a Q4-ish quant.
            Filter::Fits => params_from_name(repo).map(|p| fit_for((p * 0.6 * 1024f64.powi(3)) as u64, self.system.as_ref()) != Fit::No).unwrap_or(true),
            other => have.contains(other),
        })
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn row(&self, id: SharedString, repo: &str, title: &str, blurb: Option<&str>, author: &str, downloads: u64, active: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (r, t) = (repo.to_string(), title.to_string());
        // The seal claims a trusted quantizer, so it follows the author: a pick that fell back to
        // another uploader doesn't get it, a search hit from one does.
        let verified = is_trusted(author);
        list_row(id, active)
            .items_start()
            .flex_none()
            .gap(px(10.))
            .py(px(8.))
            .child(family_logo(repo))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(title.to_string()))
                            .when(verified, |d| d.child(div().id("verified").flex_none().tooltip(tip("Built by a trusted quantizer")).child(ph_icon(ph::SEAL_CHECK, Weight::Fill, Tokens::accent(), icon_px::SM)))),
                    )
                    .when_some(blurb.map(String::from), |d, b| d.child(div().text_size(px(size::XS)).line_height(px(15.)).text_color(Tokens::text_3()).line_clamp(2).child(b)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(size::XS))
                            .text_color(Tokens::text_3())
                            .child(div().truncate().child(author.to_string()))
                            .child(count_stat(ph::DOWNLOAD_SIMPLE, downloads)),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(r.clone(), Some(t.clone()), cx)))
    }

    /// Closes the filter popover; the keyboard goes back where it was if the popover still had it.
    fn close_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filters_open = false;
        if let Some(prev) = self.filters_restore.take().filter(|_| self.filters_focus.contains_focused(window, cx)) {
            prev.focus(window, cx);
        }
        cx.notify();
    }

    fn filters_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.filters.len();
        div()
            .relative()
            .flex_none()
            .child(
                btn("hub-filters", if n > 0 { format!("Filters · {n}") } else { "Filters".to_string() }, ButtonKind::Ghost)
                    .small()
                    .icon(glyphs::FILTER)
                    .selected(n > 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.filters_closed_at.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
                            return;
                        }
                        if this.filters_open {
                            this.close_filters(window, cx);
                        } else {
                            this.filters_open = true;
                            this.filters_restore = window.focused(cx);
                            window.focus(&this.filters_focus, cx);
                            cx.notify();
                        }
                    })),
            )
            .when(self.filters_open, |d| {
                let chips = div().flex().flex_wrap().gap(px(6.)).children(Filter::ALL.into_iter().map(|f| {
                    filter_chip(SharedString::from(format!("flt-{}", f.label())), f.label(), self.filters.contains(&f)).icon(filter_icon(f)).on_click(cx.listener(move |this, _, _, cx| {
                        if !this.filters.remove(&f) {
                            this.filters.insert(f);
                        }
                        cx.notify();
                    }))
                }));
                let pop = popup_surface()
                    .w(px(300.))
                    .p(px(12.))
                    .gap(px(10.))
                    .track_focus(&self.filters_focus)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            this.close_filters(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        this.close_filters(window, cx);
                        this.filters_closed_at = Some(Instant::now());
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Filter models"))
                            .when(n > 0, |d| {
                                d.child(btn("hub-filters-clear", "Clear", ButtonKind::Ghost).small().on_click(cx.listener(|this, _, _, cx| {
                                    this.filters.clear();
                                    cx.notify();
                                })))
                            }),
                    )
                    .child(chips);
                d.child(div().absolute().bottom_0().right_0().child(deferred(anchored().anchor(Anchor::TopRight).offset(point(px(0.), px(6.))).snap_to_window_with_margin(px(8.)).child(appear("hub-filters-pop", pop))).with_priority(2)))
            })
    }

    fn left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_ix = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let tabs = segmented("hub-tabs", [(Some(ph::SPARKLE), "Picks"), (Some(glyphs::SEARCH), "Search"), (Some(glyphs::STORAGE), "Downloaded")], tab_ix).small().full_width().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let t = TABS[(*ix).min(TABS.len() - 1)];
            this.set_tab(t, cx);
        }));
        // The search field stays put above the results instead of scrolling away with them.
        let toolbar = match self.tab {
            Tab::Hf => Some(div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().child(field(&self.query).small().cleanable(true).prefix(search_prefix()))).child(self.filters_button(cx))),
            Tab::Picks => Some(div().flex().items_center().gap(px(6.)).child(div().flex_1().child(muted("Hand-picked GGUF builds"))).child(self.filters_button(cx))),
            Tab::Local => None,
        };
        let sel = self.selected.clone();
        let mut list = div().id("hub-list").flex().flex_col().gap(px(2.)).flex_1().min_h_0().overflow_y_scroll();
        match self.tab {
            Tab::Picks => {
                if !self.picks_loaded {
                    list = list.children((0..6).map(|_| skeleton_row()));
                } else if self.picks.iter().all(Option::is_none) {
                    list = list.child(
                        empty_view(ph::WIFI_SLASH, "Couldn't reach Hugging Face", "Check your connection, then try again.")
                            .compact()
                            .action(btn("picks-retry", "Try again", ButtonKind::Secondary).small().icon(glyphs::RETRY).on_click(cx.listener(|this, _, _, cx| this.load_picks(cx)))),
                    );
                } else {
                    let mut shown = 0;
                    for (i, p) in PICKS.iter().enumerate() {
                        let Some(m) = &self.picks[i] else { continue };
                        if !self.passes(&m.repo, &m.tags, m.pipeline.as_deref(), p.tags) {
                            continue;
                        }
                        shown += 1;
                        list = list.child(self.row(SharedString::from(format!("pick-{i}")), &m.repo, p.title, Some(p.blurb), &m.author, m.downloads, sel.as_deref() == Some(m.repo.as_str()), cx));
                    }
                    if shown == 0 {
                        list = list.child(self.no_filter_matches(cx));
                    }
                }
            }
            Tab::Hf => {
                if self.hf_loading {
                    list = list.children((0..6).map(|_| skeleton_row()));
                } else if let Some(e) = &self.hf_error {
                    list = list.child(error_view(e.clone()).title("Search failed").compact().retry(cx.listener(|this, _, _, cx| this.search(cx))));
                } else {
                    let mut shown = 0;
                    for (i, m) in self.hf.iter().enumerate() {
                        if !self.passes(&m.repo, &m.tags, m.pipeline.as_deref(), &[]) {
                            continue;
                        }
                        shown += 1;
                        list = list.child(self.row(SharedString::from(format!("hf-{i}")), &m.repo, &m.name, None, &m.author, m.downloads, sel.as_deref() == Some(m.repo.as_str()), cx));
                    }
                    if self.hf_loaded && self.hf.is_empty() {
                        list = list.child(empty_view(glyphs::SEARCH, "No GGUF models found", "Try a shorter name, or add “GGUF” to the search.").compact());
                    } else if shown == 0 && !self.hf.is_empty() {
                        list = list.child(self.no_filter_matches(cx));
                    }
                }
            }
            Tab::Local => list = list.child(self.local_view(cx)),
        }
        div().flex().flex_col().gap(px(10.)).w(px(340.)).flex_none().h_full().child(tabs).children(toolbar).child(list)
    }

    fn no_filter_matches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        empty_view(glyphs::FILTER, "Nothing matches these filters", "Turn some filters off to see more models.").compact().action(btn("filters-reset", "Clear filters", ButtonKind::Secondary).small().on_click(cx.listener(|this, _, _, cx| {
            this.filters.clear();
            cx.notify();
        })))
    }

    fn local_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active: Vec<&HubDownload> = self.downloads.iter().filter(|d| d.state.is_active()).collect();
        let (ollama, current) = {
            let s = self.shared.read(cx);
            let cur = (s.store.prefs.provider == AiProviderId::Ollama).then(|| s.store.prefs.model.clone());
            (s.status(AiProviderId::Ollama).cloned(), cur)
        };
        let ollama_ready = ollama.as_ref().map(|s| s.state == AiProviderState::Ready).unwrap_or(false);
        let same_model = |a: &str, b: &str| a == b || a.trim_end_matches(":latest") == b.trim_end_matches(":latest");
        let mut col = div().flex().flex_col().gap(px(10.)).pb(px(8.));
        for d in &active {
            col = col.child(download_row(d, cx));
        }
        if self.local.is_empty() && active.is_empty() {
            col = col.child(
                empty_view(glyphs::STORAGE, "No downloaded models yet", "Pick a model under Picks or Search, then download the size that fits this PC.")
                    .compact()
                    .action(btn("go-picks", "See picks", ButtonKind::Secondary).small().icon(ph::SPARKLE).on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Picks, cx)))),
            );
        }
        // A file nothing can run is a dead end: say what Ollama needs (installed but off or not
        // running is not "install it"), with the way to fix it.
        let ollama_state = ollama.as_ref().map(|s| s.state);
        if !self.local.is_empty() && !ollama_ready && self.local.iter().any(|m| m.imported_as.is_none()) {
            let alert = match ollama_state {
                Some(AiProviderState::Disabled) => inline_alert(AlertKind::Warning, "Turn on Ollama to chat with these models", "Ollama is installed but turned off in AI providers.")
                    .action("Open AI providers", |_, w, cx| mv_ui::navigate(Route::Settings { tab: Some("providers".into()) }, w, cx)),
                Some(AiProviderState::Offline | AiProviderState::Error) => inline_alert(AlertKind::Warning, "Start Ollama to chat with these models", "Ollama is installed but isn't running. Open it, then re-check."),
                _ => inline_alert(AlertKind::Warning, "Install Ollama to chat with these models", "MangaVox runs downloaded GGUF files through Ollama.").action("Get Ollama", |_, _, cx| cx.open_url(OLLAMA_URL)),
            };
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(alert)
                    .child(div().flex().justify_end().child(btn("ollama-recheck", "Re-check", ButtonKind::Ghost).small().icon(glyphs::REFRESH).on_click(cx.listener(|this, _, _, cx| this.shared.update(cx, |s, cx| s.refresh_providers(true, cx)))))),
            );
        }
        let import_blocked = match ollama_state {
            Some(AiProviderState::Disabled) => "Turn on Ollama in AI providers first",
            Some(AiProviderState::Offline | AiProviderState::Error) => "Start Ollama first",
            _ => "Install and start Ollama first",
        };
        if !self.local.is_empty() {
            let total: u64 = self.local.iter().map(|m| m.size).sum();
            col = col.child(section_label(format!("{} on this PC · {}", self.local.len(), fmt_bytes(total))).pt(px(4.)));
        }
        for (i, m) in self.local.iter().enumerate() {
            let confirming = self.confirm_delete.as_deref() == Some(m.path.as_str());
            let importing = self.importing.as_deref() == Some(m.path.as_str());
            let (p_imp, p_del, p_ask, name) = (m.path.clone(), m.path.clone(), m.path.clone(), m.name.clone());
            let actions: AnyElement = match &m.imported_as {
                Some(n) => {
                    let in_use = current.as_deref().is_some_and(|c| same_model(c, n));
                    let n2 = n.clone();
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.))
                        .child(icon_tag(ph::CHECK_CIRCLE, format!("In Ollama as {n}"), Tone::Success))
                        .child(div().flex_1())
                        .child(if in_use {
                            icon_tag(ph::CHECK, "In use", Tone::Accent).into_any_element()
                        } else {
                            btn(("use", i), "Use in chat", ButtonKind::Primary).small().icon(glyphs::ASSISTANT).on_click(cx.listener(move |this, _, _, cx| this.use_in_chat(n2.clone(), cx))).into_any_element()
                        })
                        .into_any_element()
                }
                None => {
                    let b = btn(("imp", i), "Add to Ollama", ButtonKind::Secondary).small().icon("brand/ollama.svg").loading(importing);
                    let b = if ollama_ready { b.on_click(cx.listener(move |this, _, _, cx| this.import(p_imp.clone(), cx))) } else { b.disabled(import_blocked) };
                    div().flex().items_center().gap(px(6.)).child(muted("Not added to Ollama yet")).child(div().flex_1()).child(b).into_any_element()
                }
            };
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(radius::LG))
                    .bg(Tokens::surface())
                    .border_1()
                    .border_color(if confirming { Tokens::danger_border() } else { Tokens::border() })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(family_logo(m.repo.as_deref().unwrap_or(&m.name)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(m.name.clone()))
                                    .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(match &m.repo {
                                        Some(r) => format!("{} · {r}", fmt_bytes(m.size)),
                                        None => fmt_bytes(m.size),
                                    })),
                            )
                            .child(icon_button(("rm", i), glyphs::DELETE, "Delete file").small().danger().on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_delete = Some(p_ask.clone());
                                cx.notify();
                            }))),
                    )
                    .child(if confirming {
                        // Multi-gigabyte and not undoable: confirm in place, buttons named by outcome.
                        confirm_inline(format!("Delete this file ({})? It can't be undone.", fmt_bytes(m.size)), "Delete file", true)
                            .cancel_label("Keep")
                            .flex_wrap()
                            .on_cancel(cx.listener(|this, _, _, cx| {
                                this.confirm_delete = None;
                                cx.notify();
                            }))
                            .on_confirm(cx.listener(move |this, _, _, cx| this.delete_local(p_del.clone(), name.clone(), cx)))
                            .into_any_element()
                    } else {
                        actions
                    }),
            );
        }
        // Secondary setup lives below the models it serves.
        col.child(
            div().pt(px(6.)).child(
                disclosure("hub-advanced", "Add from a link or token").content(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .pr(px(4.))
                        .child(div().flex().items_center().gap(px(6.)).child(div().flex_1().min_w_0().child(field(&self.url).small())).child(btn("dl-url", "Download", ButtonKind::Secondary).small().icon(ph::LINK).on_click(cx.listener(|this, _, w, cx| this.download_url(w, cx)))))
                        .child(muted(if self.has_token { "A Hugging Face token is saved." } else { "A token lets you download gated repos." }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().flex_1().min_w_0().child(field(&self.token).small().mask_toggle()))
                                .child(btn("tok-save", if self.has_token { "Replace" } else { "Save" }, ButtonKind::Secondary).small().icon(ph::KEY).on_click(cx.listener(|this, _, w, cx| this.save_token(w, cx)))),
                        ),
                ),
            ),
        )
    }

    fn hardware_line(&self) -> String {
        match &self.system {
            Some(s) => {
                let mut parts = vec![];
                if let Some(g) = &s.gpu_name {
                    parts.push(g.clone());
                }
                if s.vram_gb > 0.0 {
                    parts.push(format!("{:.0} GB graphics memory", s.vram_gb));
                }
                parts.push(format!("{:.0} GB RAM", s.ram_gb));
                format!("This PC: {}. Each option is rated for it.", parts.join(" · "))
            }
            None => "Checking this PC's graphics card and memory…".into(),
        }
    }

    fn right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = div().id("hub-detail").flex_1().min_w_0().h_full().overflow_y_scroll().rounded(px(radius::LG)).border_1().border_color(Tokens::border()).bg(Tokens::surface()).p(px(18.));
        if self.selected.is_none() {
            return shell.child(div().size_full().flex().items_center().justify_center().child(empty_view(ph::CLOUD_ARROW_DOWN, "Pick a model to see download options", self.hardware_line())));
        }
        let d = match &self.detail {
            None => {
                return shell.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(skeleton_line(px(220.)).h(px(22.)))
                        .child(skeleton_line(px(160.)))
                        .child(div().flex().flex_wrap().gap(px(8.)).children((0..5).map(|_| skeleton_block(px(88.), px(44.)))))
                        .child(skeleton_text(3))
                        .children((0..3).map(|_| skeleton(relative(1.), px(46.), radius::LG))),
                )
            }
            Some(Err(e)) => {
                let (repo, title) = (self.selected.clone().unwrap_or_default(), self.selected_title.clone());
                return shell.child(error_view(e.clone()).title("Couldn't load this model").retry(cx.listener(move |this, _, _, cx| this.select(repo.clone(), title.clone(), cx))));
            }
            Some(Ok(d)) => d,
        };
        let s = &d.summary;
        let tags = infer_tags(&s.repo, &s.tags, s.pipeline.as_deref());
        let recommended = pick_recommended(&d.files, self.system.as_ref());
        let common = ["Q3_K_M", "Q4_K_M", "Q5_K_M", "Q6_K", "Q8_0", "MXFP4", "Q4_K_XL"];
        let mut files: Vec<&HubModelFile> = d.files.iter().filter(|f| self.show_all || Some(&f.path) == recommended.as_ref() || common.contains(&f.quant.as_deref().unwrap_or(""))).collect();
        if files.is_empty() {
            files = d.files.iter().collect();
        }
        files.sort_by_key(|f| f.size);
        let params = d.params_b.or_else(|| params_from_name(&s.repo)).map(|p| format!("{p}B")).unwrap_or_else(|| "—".into());
        let stats = [
            ("Params", params),
            ("Arch", d.arch.clone().unwrap_or_else(|| "—".into())),
            ("Context", d.context.map(|c| format!("{}K", c / 1024)).unwrap_or_else(|| "—".into())),
            ("Format", "GGUF".into()),
            ("License", d.license.clone().unwrap_or_else(|| "—".into())),
        ];
        let repo = s.repo.clone();
        let hf_url = format!("https://huggingface.co/{repo}");
        let mut col = div().flex().flex_col().gap(px(12.));
        col = col
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(family_logo(&repo))
                    .child(div().flex_1().min_w_0().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).truncate().child(self.selected_title.clone().unwrap_or_else(|| s.name.clone())))
                    .child(external_link("Hugging Face", hf_url)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(size::XS))
                    .text_color(Tokens::text_3())
                    .child(tag(repo.clone(), Tone::Neutral))
                    .child(count_stat(ph::DOWNLOAD_SIMPLE, s.downloads))
                    .child(count_stat(ph::HEART, s.likes))
                    .when(s.gated, |d| d.child(icon_tag(ph::LOCK, "Gated", Tone::Warning))),
            )
            .child(div().flex().flex_wrap().gap(px(8.)).children(stats.into_iter().map(|(k, v)| {
                div().flex().flex_col().min_w(px(80.)).px(px(10.)).py(px(6.)).rounded(px(radius::MD)).bg(Tokens::surface_2()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(k)).child(div().truncate().max_w(px(160.)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(v))
            })))
            .when(!tags.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(6.)).children(tags.iter().map(|t| icon_tag(filter_icon(*t), t.label(), Tone::Neutral)))))
            .when_some(d.description.clone().filter(|t| !t.trim().is_empty()), |d, t| d.child(div().text_size(px(size::SM)).line_height(px(19.)).text_color(Tokens::text_2()).line_clamp(6).child(t)))
            .child(div().mt(px(6.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Download options"))
            .child(muted(self.hardware_line()));
        if d.files.is_empty() {
            return shell.child(col.child(empty_view(glyphs::MODEL, "No GGUF files in this repo", "Search for the model name plus “GGUF” to find a build that runs on this PC.").compact()));
        }
        for group in ["Smaller", "Balanced", "Higher quality", "Best quality"] {
            let rows: Vec<&&HubModelFile> = files.iter().filter(|f| quant_group(f, recommended.as_deref()).0 == group).collect();
            if rows.is_empty() {
                continue;
            }
            col = col.child(section_label(group).pt(px(4.)));
            for (i, f) in rows.into_iter().enumerate() {
                col = col.child(self.quant_row(&repo, f, group, i, recommended.as_deref(), cx));
            }
        }
        col = col.child(
            div().child(
                btn("show-all", if self.show_all { "Show fewer options" } else { "Show all quantizations" }, ButtonKind::Ghost).small().trailing_icon(if self.show_all { ph::CARET_UP } else { ph::CARET_DOWN }).on_click(cx.listener(|this, _, _, cx| {
                    this.show_all = !this.show_all;
                    cx.notify();
                })),
            ),
        );
        shell.child(col)
    }

    fn quant_row(&self, repo: &str, f: &HubModelFile, group: &str, i: usize, recommended: Option<&str>, cx: &mut Context<Self>) -> impl IntoElement {
        let fit = fit_for(f.size, self.system.as_ref());
        let is_rec = quant_group(f, recommended).1;
        let fname = base_name(f.parts.as_ref().and_then(|p| p.first()).unwrap_or(&f.path)).to_string();
        let dl = self.downloads.iter().filter(|d| d.repo.as_deref() == Some(repo) && d.file == fname).max_by_key(|d| d.state.is_active() as u8);
        let (fit_icon, fit_color) = match fit {
            Fit::Great => (ph::CHECK_CIRCLE, Tokens::success()),
            Fit::Ok => (ph::GAUGE, Tokens::text_2()),
            Fit::Slow => (ph::HOURGLASS_MEDIUM, Tokens::warning()),
            Fit::No => (ph::X_CIRCLE, Tokens::danger()),
        };
        let (r, file) = (repo.to_string(), f.clone());
        let action: AnyElement = match dl {
            Some(d) if d.state.is_active() => {
                let id = d.id.clone();
                btn(SharedString::from(format!("cancel-{group}-{i}")), "Cancel", ButtonKind::Ghost)
                    .small()
                    .icon(glyphs::CLOSE)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.hub.cancel_download(&id);
                        this.start_polling(cx);
                    }))
                    .into_any_element()
            }
            Some(d) if d.state == HubDownloadState::Done => icon_tag(ph::CHECK_CIRCLE, "Downloaded", Tone::Success).into_any_element(),
            _ => btn(SharedString::from(format!("get-{group}-{i}")), "Download", if is_rec { ButtonKind::Primary } else { ButtonKind::Secondary })
                .small()
                .icon(glyphs::DOWNLOAD)
                .when(fit == Fit::No, |b| b.tooltip("Needs more memory than this PC has"))
                .on_click(cx.listener(move |this, _, _, cx| this.download_file(r.clone(), file.clone(), cx)))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if is_rec { Tokens::accent_border() } else { Tokens::border() })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().w(px(80.)).text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(f.quant.clone().unwrap_or_else(|| "GGUF".into())))
                    .child(div().w(px(64.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(fmt_bytes(f.size)))
                    .when(is_rec, |d| d.child(icon_tag(ph::SPARKLE, "Recommended", Tone::Accent)))
                    .child(
                        div()
                            .id(SharedString::from(format!("fit-{group}-{i}")))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(size::XS))
                            .text_color(fit_color)
                            .tooltip(tip(fit.detail()))
                            .child(ph_icon(fit_icon, Weight::Fill, fit_color, icon_px::XS))
                            .child(div().truncate().child(fit.text())),
                    )
                    .child(action),
            )
            .when_some(dl.filter(|d| matches!(d.state, HubDownloadState::Downloading | HubDownloadState::Queued | HubDownloadState::Error)), |d, dl| d.child(download_status(dl)))
    }
}

/// Download or like count with its Phosphor glyph (not a Unicode arrow or heart).
fn count_stat(icon: Ph, n: u64) -> Div {
    div().flex().flex_none().items_center().gap(px(3.)).child(ph_icon(icon, Weight::Bold, Tokens::text_3(), icon_px::XS)).child(fmt_count(n))
}

fn filter_icon(f: Filter) -> Ph {
    match f {
        Filter::Fits => ph::CPU,
        Filter::Moe => ph::STACK,
        Filter::Vision => ph::EYE,
        Filter::Coding => ph::CODE,
        Filter::Reasoning => ph::BRAIN,
        Filter::Distilled => ph::FEATHER,
        Filter::Uncensored => ph::LOCK_OPEN,
    }
}

/// Brand mark of the model family when `brand/` has one (OpenAI, DeepSeek, Mistral); otherwise a
/// model glyph tinted per family, so Qwen, Gemma and Llama rows still look different at a glance
/// (the uploader's initial said nothing: most builds come from the same two quantizers).
pub fn family_logo(repo: &str) -> AnyElement {
    let r = repo.to_lowercase();
    let tile = div().flex().flex_none().items_center().justify_center().size(px(32.)).rounded(px(radius::MD));
    let brand = if r.contains("gpt-oss") || r.contains("openai") {
        Some(("brand/openai.svg", true))
    } else if r.contains("deepseek") {
        Some(("brand/deepseek.svg", false))
    } else if ["mistral", "mixtral", "codestral", "devstral", "magistral", "ministral"].iter().any(|w| r.contains(w)) {
        Some(("brand/mistral.svg", false))
    } else {
        None
    };
    match brand {
        Some((path, true)) => tile.bg(Tokens::surface_2()).child(svg().path(path).size(px(icon_px::LG)).flex_none().text_color(Tokens::text_1())).into_any_element(),
        Some((path, false)) => tile.bg(Tokens::surface_2()).child(img(path).size(px(icon_px::LG)).flex_none()).into_any_element(),
        None => {
            let hue = family_hue(&r);
            tile.bg(Tokens::hue_soft(hue)).child(ph_icon(glyphs::MODEL, Weight::Duotone, Tokens::hue(hue), icon_px::LG)).into_any_element()
        }
    }
}

/// One stable colour per model family (by name, so a family always gets the same one).
fn family_hue(repo_lower: &str) -> Hue {
    let name = repo_lower.rsplit('/').next().unwrap_or(repo_lower);
    for (key, hue) in [("qwen", Hue::Violet), ("gemma", Hue::Blue), ("llama", Hue::Orange), ("glm", Hue::Green), ("phi", Hue::Cyan), ("dolphin", Hue::Cyan), ("granite", Hue::Slate)] {
        if name.contains(key) {
            return hue;
        }
    }
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    Hue::ALL[(h as usize) % Hue::ALL.len()]
}

fn download_status(d: &HubDownload) -> Div {
    let frac = if d.total > 0 { (d.received as f32 / d.total as f32).clamp(0., 1.) } else { 0. };
    let failed = d.state == HubDownloadState::Error;
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .when(!failed, |el| el.child(progress_bar(SharedString::from(format!("dl-bar-{}", d.id)), frac, d.state.is_active())))
        .child(div().flex().items_center().gap(px(5.)).text_size(px(size::XS)).text_color(if failed { Tokens::danger() } else { Tokens::text_3() }).when(failed, |el| el.child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::XS))).child(match d.state {
            HubDownloadState::Error => d.error.clone().unwrap_or_else(|| "Download failed".into()),
            HubDownloadState::Queued => "Queued…".into(),
            _ => format!("{} / {} · {}/s", fmt_bytes(d.received), fmt_bytes(d.total), fmt_bytes(d.bytes_per_sec)),
        }))
}

fn download_row(d: &HubDownload, cx: &mut Context<ModelHubPanel>) -> impl IntoElement {
    let id = d.id.clone();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(px(radius::LG))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ph_icon(glyphs::DOWNLOAD, Weight::Bold, Tokens::accent(), icon_px::SM))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(d.file.clone()))
                .child(btn(SharedString::from(format!("c-{}", d.id)), "Cancel", ButtonKind::Ghost).small().icon(glyphs::CLOSE).on_click(cx.listener(move |this, _, _, cx| {
                    this.hub.cancel_download(&id);
                    this.start_polling(cx);
                }))),
        )
        .child(download_status(d))
}

impl Render for ModelHubPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Embedded in Settings → Model Library, which shows the title and the hardware line.
        div().flex().gap(px(14.)).size_full().child(self.left(cx)).child(self.right(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(repo: &str) -> HubModelSummary {
        let (a, n) = repo.split_once('/').unwrap();
        HubModelSummary { repo: repo.into(), author: a.into(), name: n.into(), downloads: 1, likes: 0, updated_at: None, tags: vec![], pipeline: None, gated: false }
    }

    #[::core::prelude::v1::test]
    fn picks_prefer_trusted_quantizers() {
        let r = vec![m("random/Qwen3.5-4B-GGUF"), m("Unsloth/Qwen3.5-4B-GGUF"), m("bartowski/Qwen3.5-4B-GGUF")];
        assert_eq!(resolve_pick(&r).unwrap().repo, "Unsloth/Qwen3.5-4B-GGUF");
        let r = vec![m("random/x-GGUF")];
        assert_eq!(resolve_pick(&r).unwrap().repo, "random/x-GGUF");
        assert!(resolve_pick(&[]).is_none());
        // The seal follows the uploader, so a fallback pick doesn't claim it.
        assert!(is_trusted("Unsloth") && is_trusted("bartowski"));
        assert!(!is_trusted("random"));
    }

    #[::core::prelude::v1::test]
    fn basenames() {
        assert_eq!(base_name("a/b/c.gguf"), "c.gguf");
        assert_eq!(base_name("c.gguf"), "c.gguf");
    }

    #[::core::prelude::v1::test]
    fn families_keep_one_colour() {
        assert_eq!(family_hue("unsloth/qwen3.5-4b-gguf"), Hue::Violet);
        assert_eq!(family_hue("bartowski/qwen3-8b"), Hue::Violet);
        assert_eq!(family_hue("x/gemma-4-12b"), Hue::Blue);
        // Unknown families are stable across calls.
        assert_eq!(family_hue("x/zeta-7b"), family_hue("y/zeta-7b"));
    }
}
