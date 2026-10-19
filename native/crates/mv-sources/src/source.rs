//! The `Source` trait (the Rust form of `MangaSourcePlugin` in `src/shared/plugin-interface.ts`),
//! the error type, the shared fetch context and the Cloudflare `WallSolver` hook.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use mv_net::{Http, NetError, Opts, Resp};
use serde::de::DeserializeOwned;

use crate::types::{ChapterInfo, MangaDetails, MangaSearchResult, SourceInfo, Tag};

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error(transparent)]
    Net(#[from] NetError),
    /// The page / JSON did not look like what the parser expects (site layout changed, interstitial, ...).
    #[error("{0}")]
    Parse(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The source does not offer this optional capability (popular / latest / tags).
    #[error("{0} is not supported by this source")]
    Unsupported(&'static str),
    /// An anti-bot wall answered and no [`WallSolver`] is attached (or it failed).
    #[error("{source_id}: blocked by an anti-bot wall ({kind}) at {url}; a browser verification is required")]
    Wall { source_id: String, kind: String, url: String },
    /// This source can only run with a [`WallSolver`].
    #[error("{0} needs a browser-backed WallSolver (hidden WebView) which is not attached")]
    SolverRequired(&'static str),
    /// The WallSolver itself failed.
    #[error("wall solver: {0}")]
    Solver(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SourceError>;

/// Provides what a hidden browser window gives the Electron app: clearance cookies, in-browser
/// fetches and rendered DOM for Cloudflare-walled sites. Implemented by `mv-webwall`.
///
/// Note: Cloudflare ties `cf_clearance` to the User-Agent, so build the shared [`Http`] with
/// `NetConfig::user_agent` set to [`WallSolver::user_agent`] when it returns `Some`.
#[async_trait]
pub trait WallSolver: Send + Sync {
    /// UA string the solver's browser uses (cookies it returns only work with the same UA).
    fn user_agent(&self) -> Option<String> {
        None
    }
    /// Load `url`, clear any challenge (hidden first, then a visible "verify" window), and return
    /// the site's cookies as `(name, value)` pairs so reqwest can replay them.
    async fn cookies(&self, url: &str) -> Result<Vec<(String, String)>>;
    /// Same-origin `fetch(url)` from inside the page context; returns the response body text.
    async fn fetch_text(&self, url: &str, headers: &[(String, String)]) -> Result<String>;
    /// Navigate to `url`, optionally wait for a CSS selector to appear, return the rendered DOM
    /// (`document.documentElement.outerHTML`).
    async fn fetch_html(&self, url: &str, wait_for: Option<&str>) -> Result<String>;
    /// Navigate to `page_url` (if not already there) and evaluate an async JS expression; the
    /// result must be JSON-serializable.
    async fn eval_json(&self, page_url: &str, script: &str) -> Result<serde_json::Value>;
}

/// Everything a source needs: the shared HTTP client and the optional wall solver.
#[derive(Clone)]
pub struct SourceCtx {
    pub http: Http,
    pub solver: Option<Arc<dyn WallSolver>>,
}

impl SourceCtx {
    pub fn new(http: Http) -> Self {
        Self { http, solver: None }
    }

    pub fn with_solver(mut self, solver: Arc<dyn WallSolver>) -> Self {
        self.solver = Some(solver);
        self
    }

    /// GET through reqwest; when the answer is an anti-bot wall / 403 / 503 and a solver is
    /// attached, borrow its cookies and retry, then fall back to an in-browser fetch.
    /// The response is returned for any other status (callers decide what a 404 means).
    pub async fn fetch(&self, source_id: &str, url: &str, opts: &Opts) -> Result<Resp> {
        let first = match self.http.get(url, opts).await {
            Ok(r) => r,
            Err(e) => {
                if let (Some(_), NetError::Request { .. }) = (&self.solver, &e) {
                    return self.via_solver(source_id, url, opts).await;
                }
                return Err(e.into());
            }
        };
        let wall = first.wall();
        if wall.is_none() && first.status != 403 && first.status != 503 {
            return Ok(first);
        }
        if self.solver.is_none() {
            return match wall {
                Some(k) => Err(SourceError::Wall { source_id: source_id.into(), kind: k.as_str().into(), url: url.into() }),
                None => Ok(first),
            };
        }
        self.via_solver(source_id, url, opts).await
    }

    async fn via_solver(&self, source_id: &str, url: &str, opts: &Opts) -> Result<Resp> {
        let solver = self.solver.as_ref().expect("checked by caller");
        if let Ok(cookies) = solver.cookies(url).await {
            for (n, v) in &cookies {
                let _ = self.http.add_cookie(url, &format!("{n}={v}"));
            }
            if let Ok(r) = self.http.get(url, opts).await {
                if r.wall().is_none() && r.status != 403 && r.status != 503 {
                    return Ok(r);
                }
            }
        }
        let text = solver.fetch_text(url, &opts.headers).await?;
        let r = Resp { status: 200, url: url.into(), headers: HashMap::new(), body: text.into_bytes().into() };
        if let Some(k) = r.wall() {
            return Err(SourceError::Wall { source_id: source_id.into(), kind: k.as_str().into(), url: url.into() });
        }
        Ok(r)
    }

    /// [`fetch`](Self::fetch) as text; any status >= 400 becomes an error.
    pub async fn fetch_text(&self, source_id: &str, url: &str, opts: &Opts) -> Result<String> {
        Ok(self.fetch(source_id, url, opts).await?.error_for_status()?.text())
    }

    /// [`fetch`](Self::fetch) as JSON; any status >= 400 becomes an error.
    pub async fn fetch_json<T: DeserializeOwned>(&self, source_id: &str, url: &str, opts: &Opts) -> Result<T> {
        Ok(self.fetch(source_id, url, opts).await?.error_for_status()?.json()?)
    }

    pub fn solver(&self, what: &'static str) -> Result<&Arc<dyn WallSolver>> {
        self.solver.as_ref().ok_or(SourceError::SolverRequired(what))
    }
}

/// A manga source. Page / chapter ids are opaque strings produced by the source itself.
/// `page` arguments are 1-based.
#[async_trait]
pub trait Source: Send + Sync {
    fn info(&self) -> SourceInfo;

    async fn search_manga(&self, query: &str, page: u32) -> Result<Vec<MangaSearchResult>>;
    async fn get_manga_details(&self, manga_id: &str) -> Result<MangaDetails>;
    /// Ascending (chapter 1 first).
    async fn get_chapter_list(&self, manga_id: &str) -> Result<Vec<ChapterInfo>>;
    /// Image URLs in reading order. URLs may carry a `#mvref=<referer>` fragment
    /// (see [`crate::util::split_ref_hint`]).
    async fn get_page_urls(&self, chapter_id: &str) -> Result<Vec<String>>;

    async fn get_popular_manga(&self, _page: u32) -> Result<Vec<MangaSearchResult>> {
        Err(SourceError::Unsupported("popular"))
    }
    async fn get_latest_manga(&self, _page: u32) -> Result<Vec<MangaSearchResult>> {
        Err(SourceError::Unsupported("latest"))
    }
    async fn get_tag_list(&self) -> Result<Vec<Tag>> {
        Ok(Vec::new())
    }
    async fn get_manga_by_tag(&self, _tag_id: &str, _page: u32) -> Result<Vec<MangaSearchResult>> {
        Err(SourceError::Unsupported("tag browsing"))
    }
    /// Titles carrying *all* of `tag_ids` (AND). Sources that cannot combine tags only accept one
    /// (see [`supports_multi_tag`](Self::supports_multi_tag)).
    async fn get_manga_by_tags(&self, tag_ids: &[String], page: u32) -> Result<Vec<MangaSearchResult>> {
        match tag_ids {
            [] => self.get_popular_manga(page).await,
            [one] => self.get_manga_by_tag(one, page).await,
            _ => Err(SourceError::Unsupported("combining tags")),
        }
    }
    /// Whether [`get_manga_by_tags`](Self::get_manga_by_tags) accepts more than one tag.
    fn supports_multi_tag(&self) -> bool {
        false
    }

    /// Headers (Referer, User-Agent, ...) to use when downloading page images.
    fn download_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}
