//! Settings → Model Hub: curated picks, Hugging Face search, quant picker with a fit rating,
//! resumable downloads, Ollama import and a downloaded-models manager.
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_ai::{HubConfigPatch, HubDownload, HubDownloadState, HubModelDetail, HubModelFile, HubModelSummary, HubSystemFit, LocalModelFile, ModelHub, SearchOpts};
use mv_ui::{button, chip, icon, size, ButtonKind, Tokens};

use crate::fit::{detect_system, fit_for, infer_tags, params_from_name, pick_recommended, quant_group, Filter, Fit};
use crate::shared::shared;
use crate::ui::{field, fmt_bytes, fmt_count, muted, password_state, set_value, tag, text_state, value_of};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Picks,
    Hf,
    Local,
}

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

/// Choose the repo from a trusted quantizer, else the most downloaded.
pub fn resolve_pick(results: &[HubModelSummary]) -> Option<&HubModelSummary> {
    results.iter().find(|m| TRUSTED.iter().any(|t| t.eq_ignore_ascii_case(&m.author))).or_else(|| results.first())
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

pub struct ModelHubPanel {
    hub: ModelHub,
    tab: Tab,
    filters: HashSet<Filter>,
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
    note: Option<String>,
    has_token: bool,
    _subs: Vec<Subscription>,
}

pub fn create(window: &mut Window, cx: &mut App) -> AnyView {
    let hub = shared(cx).read(cx).hub.clone();
    cx.new(|cx| {
        let query = text_state("Search Hugging Face (GGUF models)", window, cx);
        let url = text_state("Paste a direct .gguf link", window, cx);
        let token = password_state("Hugging Face token (for gated repos)", window, cx);
        let sub = cx.subscribe_in(&query, window, |this: &mut ModelHubPanel, _, ev: &InputEvent, _w, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.search(cx);
            }
        });
        let mut p = ModelHubPanel {
            has_token: hub.hub_config_public().has_token,
            hub,
            tab: Tab::Picks,
            filters: HashSet::new(),
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
            note: None,
            _subs: vec![sub],
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
        // Resolve curated picks live.
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
        self.refresh_local(cx);
        self.start_polling(cx);
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
                    Ok(Err(e)) => t.note = Some(e),
                    Err(e) => t.note = Some(e.to_string()),
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
                    Ok(Ok(_)) => t.tab = Tab::Local,
                    Ok(Err(e)) => t.note = Some(e),
                    Err(e) => t.note = Some(e.to_string()),
                }
                t.start_polling(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn import(&mut self, path: String, cx: &mut Context<Self>) {
        self.note = Some("Adding to Ollama…".into());
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let r = mv_ui::bg::run(async move { hub.import_into_ollama(std::path::Path::new(&path), None).await }).await;
            let _ = this.update(cx, |t, cx| {
                t.note = Some(match r {
                    Ok(r) if r.ok => format!("Added to Ollama as {}.", r.name.unwrap_or_default()),
                    Ok(r) => r.error.unwrap_or_else(|| "Import failed".into()),
                    Err(e) => e.to_string(),
                });
                t.refresh_local(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn delete_local(&mut self, path: String, cx: &mut Context<Self>) {
        let hub = self.hub.clone();
        cx.spawn(async move |this, cx| {
            let _ = mv_ui::bg::blocking(move || hub.delete_local_model(std::path::Path::new(&path))).await;
            let _ = this.update(cx, |t, cx| t.refresh_local(cx));
        })
        .detach();
    }

    fn save_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = value_of(&self.token, cx).trim().to_string();
        let _ = self.hub.set_hub_config(&HubConfigPatch { hf_token: Some(t), ..Default::default() });
        set_value(&self.token, "", window, cx);
        self.has_token = self.hub.hub_config_public().has_token;
        self.note = Some(if self.has_token { "Token saved.".into() } else { "Token removed.".into() });
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

    fn row(&self, id: SharedString, repo: &str, title: &str, blurb: Option<&str>, author: &str, downloads: u64, active: bool, verified: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (r, t) = (repo.to_string(), title.to_string());
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(10.))
            .cursor_pointer()
            .when(active, |d| d.bg(Tokens::active()))
            .hover(|s| s.bg(Tokens::hover()))
            .child(div().flex().items_center().justify_center().flex_none().size(px(30.)).rounded(px(8.)).bg(Tokens::surface_2()).text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(author.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex().items_center().gap(px(5.)).child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(title.to_string())).when(verified, |d| d.child(icon("icons/badge-check.svg", Tokens::accent(), 13.))))
                    .when_some(blurb.map(String::from), |d, b| d.child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(b)))
                    .child(div().flex().gap(px(8.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(div().truncate().child(author.to_string())).child(format!("↓ {}", fmt_count(downloads)))),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select(r.clone(), Some(t.clone()), cx)))
    }

    fn left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [(Tab::Picks, "Picks"), (Tab::Hf, "Hugging Face"), (Tab::Local, "Downloaded")];
        let mut head = div().flex().gap(px(6.)).children(tabs.into_iter().map(|(t, label)| chip(label, label, self.tab == t).on_click(cx.listener(move |this, _, _, cx| {
            this.tab = t;
            if t == Tab::Hf && !this.hf_loaded && !this.hf_loading {
                this.search(cx);
            }
            if t == Tab::Local {
                this.refresh_local(cx);
            }
            cx.notify();
        }))));
        head = head.pb(px(2.));
        let filters = div().flex().flex_wrap().gap(px(6.)).children(Filter::ALL.into_iter().map(|f| {
            chip(SharedString::from(format!("flt-{}", f.label())), f.label(), self.filters.contains(&f)).h(px(24.)).text_size(px(size::XS)).on_click(cx.listener(move |this, _, _, cx| {
                if !this.filters.remove(&f) {
                    this.filters.insert(f);
                }
                cx.notify();
            }))
        }));
        let sel = self.selected.clone();
        let mut list = div().id("hub-list").flex().flex_col().gap(px(2.)).flex_1().min_h_0().overflow_y_scroll();
        match self.tab {
            Tab::Picks => {
                if !self.picks_loaded {
                    list = list.child(div().p(px(10.)).child(muted("Finding the best builds…")));
                }
                for (i, p) in PICKS.iter().enumerate() {
                    let Some(m) = &self.picks[i] else { continue };
                    if !self.passes(&m.repo, &m.tags, m.pipeline.as_deref(), p.tags) {
                        continue;
                    }
                    list = list.child(self.row(SharedString::from(format!("pick-{i}")), &m.repo, p.title, Some(p.blurb), &m.author, m.downloads, sel.as_deref() == Some(m.repo.as_str()), true, cx));
                }
            }
            Tab::Hf => {
                list = list.child(div().pb(px(6.)).child(field(&self.query).small().cleanable(true)));
                if self.hf_loading {
                    list = list.child(div().p(px(10.)).child(muted("Searching…")));
                }
                if let Some(e) = &self.hf_error {
                    list = list.child(div().p(px(10.)).text_size(px(size::SM)).text_color(Tokens::danger()).child(e.clone()));
                }
                for (i, m) in self.hf.iter().enumerate() {
                    if !self.passes(&m.repo, &m.tags, m.pipeline.as_deref(), &[]) {
                        continue;
                    }
                    list = list.child(self.row(SharedString::from(format!("hf-{i}")), &m.repo, &m.name, None, &m.author, m.downloads, sel.as_deref() == Some(m.repo.as_str()), false, cx));
                }
                if !self.hf_loading && self.hf_loaded && self.hf.is_empty() && self.hf_error.is_none() {
                    list = list.child(div().p(px(10.)).child(muted("No GGUF models found.")));
                }
            }
            Tab::Local => list = list.child(self.local_view(cx)),
        }
        div().flex().flex_col().gap(px(10.)).w(px(340.)).flex_none().h_full().child(head).when(self.tab != Tab::Local, |d| d.child(filters)).child(list)
    }

    fn local_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active: Vec<&HubDownload> = self.downloads.iter().filter(|d| d.state.is_active()).collect();
        let mut col = div().flex().flex_col().gap(px(10.));
        col = col.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(10.))
                .rounded(px(10.))
                .border_1()
                .border_color(Tokens::border())
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child("Download from a link"))
                .child(div().flex().gap(px(6.)).child(div().flex_1().child(field(&self.url).small())).child(button("dl-url", "Download", ButtonKind::Secondary).h(px(28.)).px(px(10.)).on_click(cx.listener(|this, _, w, cx| this.download_url(w, cx)))))
                .child(div().text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(if self.has_token { "Hugging Face token (saved)" } else { "Hugging Face token" }))
                .child(div().flex().gap(px(6.)).child(div().flex_1().child(field(&self.token).small().mask_toggle())).child(button("tok-save", if self.has_token { "Replace" } else { "Save" }, ButtonKind::Secondary).h(px(28.)).px(px(10.)).on_click(cx.listener(|this, _, w, cx| this.save_token(w, cx))))),
        );
        for d in active {
            col = col.child(download_row(d, cx));
        }
        if self.local.is_empty() {
            col = col.child(div().p(px(6.)).child(muted("No downloaded models yet.")));
        }
        for (i, m) in self.local.iter().enumerate() {
            let (p_imp, p_del) = (m.path.clone(), m.path.clone());
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .bg(Tokens::surface())
                    .child(div().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(m.name.clone()))
                    .child(div().flex().gap(px(8.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(fmt_bytes(m.size)).when_some(m.repo.clone(), |d, r| d.child(div().truncate().child(r))))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .map(|d| match &m.imported_as {
                                Some(n) => d.child(tag(format!("In Ollama: {n}"), Tokens::accent())),
                                None => d.child(button(("imp", i), "Add to Ollama", ButtonKind::Secondary).h(px(26.)).px(px(10.)).on_click(cx.listener(move |this, _, _, cx| this.import(p_imp.clone(), cx)))),
                            })
                            .child(div().flex_1())
                            .child(button(("rm", i), "Delete", ButtonKind::Ghost).h(px(26.)).px(px(10.)).on_click(cx.listener(move |this, _, _, cx| this.delete_local(p_del.clone(), cx)))),
                    ),
            );
        }
        col
    }

    fn right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = div().id("hub-detail").flex_1().min_w_0().h_full().overflow_y_scroll().rounded(px(12.)).border_1().border_color(Tokens::border()).bg(Tokens::surface()).p(px(18.));
        if self.selected.is_none() {
            return shell.child(div().size_full().flex().items_center().justify_center().child(muted("Pick a model to see its download options.")));
        }
        let d = match &self.detail {
            None => return shell.child(muted("Loading…")),
            Some(Err(e)) => return shell.child(div().text_size(px(size::SM)).text_color(Tokens::danger()).child(e.clone())),
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
        files.sort_by(|a, b| a.size.cmp(&b.size));
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
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(self.selected_title.clone().unwrap_or_else(|| s.name.clone())))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(size::XS))
                    .text_color(Tokens::text_3())
                    .child(tag(repo.clone(), Tokens::text_2()))
                    .child(div().id("hf-open").flex().items_center().gap(px(3.)).cursor_pointer().text_color(Tokens::accent()).child("Hugging Face").child(icon("icons/external-link.svg", Tokens::accent(), 11.)).on_click(move |_, _, cx| cx.open_url(&hf_url)))
                    .child(format!("↓ {}", fmt_count(s.downloads)))
                    .child(format!("♥ {}", fmt_count(s.likes)))
                    .when(s.gated, |d| d.child(tag("Gated", Tokens::warning()))),
            )
            .child(div().flex().flex_wrap().gap(px(8.)).children(stats.into_iter().map(|(k, v)| div().flex().flex_col().min_w(px(80.)).px(px(10.)).py(px(6.)).rounded(px(8.)).bg(Tokens::surface_2()).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(k)).child(div().truncate().max_w(px(160.)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(v)))))
            .when(!tags.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(6.)).children(tags.iter().map(|t| tag(t.label(), Tokens::text_2())))))
            .when_some(d.description.clone().filter(|t| !t.trim().is_empty()), |d, t| d.child(div().text_size(px(size::SM)).text_color(Tokens::text_2()).line_clamp(6).child(t)))
            .child(div().mt(px(6.)).text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Download options"));
        if d.files.is_empty() {
            return shell.child(col.child(muted("This repo has no GGUF files. Try a GGUF build (search the model name plus “GGUF”).")));
        }
        for group in ["Smaller", "Balanced", "Higher quality", "Best quality"] {
            let rows: Vec<&&HubModelFile> = files.iter().filter(|f| quant_group(f, recommended.as_deref()).0 == group).collect();
            if rows.is_empty() {
                continue;
            }
            col = col.child(div().mt(px(4.)).text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child(group.to_uppercase()));
            for (i, f) in rows.into_iter().enumerate() {
                col = col.child(self.quant_row(&repo, f, group, i, recommended.as_deref(), cx));
            }
        }
        col = col.child(
            div()
                .id("show-all")
                .text_size(px(size::SM))
                .text_color(Tokens::accent())
                .cursor_pointer()
                .child(if self.show_all { "Show fewer options" } else { "Show all quantizations" })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_all = !this.show_all;
                    cx.notify();
                })),
        );
        shell.child(col)
    }

    fn quant_row(&self, repo: &str, f: &HubModelFile, group: &str, i: usize, recommended: Option<&str>, cx: &mut Context<Self>) -> impl IntoElement {
        let fit = fit_for(f.size, self.system.as_ref());
        let is_rec = quant_group(f, recommended).1;
        let fname = base_name(f.parts.as_ref().and_then(|p| p.first()).unwrap_or(&f.path)).to_string();
        let dl = self.downloads.iter().filter(|d| d.repo.as_deref() == Some(repo) && d.file == fname).max_by_key(|d| d.state.is_active() as u8);
        let fit_color = match fit {
            Fit::Great => Tokens::accent(),
            Fit::Ok => Tokens::text_2(),
            Fit::Slow => Tokens::warning(),
            Fit::No => Tokens::danger(),
        };
        let (r, file) = (repo.to_string(), f.clone());
        let action: AnyElement = match dl {
            Some(d) if d.state.is_active() => {
                let id = d.id.clone();
                button(SharedString::from(format!("cancel-{group}-{i}")), "Cancel", ButtonKind::Ghost).h(px(26.)).px(px(10.)).on_click(cx.listener(move |this, _, _, cx| {
                    this.hub.cancel_download(&id);
                    this.start_polling(cx);
                })).into_any_element()
            }
            Some(d) if d.state == HubDownloadState::Done => div().flex().items_center().gap(px(4.)).text_size(px(size::SM)).text_color(Tokens::accent()).child(icon("icons/check.svg", Tokens::accent(), 13.)).child("Downloaded").into_any_element(),
            _ => button(SharedString::from(format!("get-{group}-{i}")), "Download", if is_rec { ButtonKind::Primary } else { ButtonKind::Secondary }).h(px(26.)).px(px(12.)).on_click(cx.listener(move |this, _, _, cx| this.download_file(r.clone(), file.clone(), cx))).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(10.))
            .border_1()
            .border_color(if is_rec { rgba(0x2bd49a55) } else { Tokens::border() })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().w(px(80.)).text_size(px(size::SM)).font_weight(FontWeight::MEDIUM).text_color(Tokens::text_1()).child(f.quant.clone().unwrap_or_else(|| "GGUF".into())))
                    .child(div().w(px(64.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(fmt_bytes(f.size)))
                    .when(is_rec, |d| d.child(tag("Recommended", Tokens::accent())))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(fit_color).child(format!("{} — {}", fit.text(), fit.detail())))
                    .child(action),
            )
            .when_some(dl.filter(|d| matches!(d.state, HubDownloadState::Downloading | HubDownloadState::Queued | HubDownloadState::Error)), |d, dl| d.child(download_status(dl)))
    }
}

fn download_status(d: &HubDownload) -> Div {
    let frac = if d.total > 0 { (d.received as f32 / d.total as f32).clamp(0., 1.) } else { 0. };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(div().h(px(4.)).w_full().rounded(px(2.)).bg(Tokens::active()).child(div().h_full().w(relative(frac)).rounded(px(2.)).bg(Tokens::accent())))
        .child(div().text_size(px(size::XS)).text_color(if d.state == HubDownloadState::Error { Tokens::danger() } else { Tokens::text_3() }).child(match d.state {
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
        .gap(px(6.))
        .p(px(10.))
        .rounded(px(10.))
        .bg(Tokens::surface())
        .child(div().flex().items_center().gap(px(8.)).child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(d.file.clone())).child(button(SharedString::from(format!("c-{}", d.id)), "Cancel", ButtonKind::Ghost).h(px(24.)).px(px(8.)).on_click(cx.listener(move |this, _, _, cx| {
            this.hub.cancel_download(&id);
            this.start_polling(cx);
        }))))
        .child(download_status(d))
}

impl Render for ModelHubPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Embedded in Settings → Model Library, which shows the title and the hardware line.
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .size_full()
            .when_some(self.note.clone(), |d, n| {
                d.child(
                    div()
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(8.))
                        .bg(Tokens::surface())
                        .border_1()
                        .border_color(Tokens::border())
                        .text_size(px(size::SM))
                        .text_color(Tokens::text_2())
                        .child(n),
                )
            })
            .child(div().flex().gap(px(14.)).flex_1().min_h_0().child(self.left(cx)).child(self.right(cx)))
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
    }

    #[::core::prelude::v1::test]
    fn basenames() {
        assert_eq!(base_name("a/b/c.gguf"), "c.gguf");
        assert_eq!(base_name("c.gguf"), "c.gguf");
    }
}
