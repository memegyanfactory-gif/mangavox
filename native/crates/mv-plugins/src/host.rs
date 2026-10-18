use crate::backend::{FetchBackend, FetchRequest, ReqwestBackend};
use crate::manifest::{read_manifest, PluginManifest};
use anyhow::{anyhow, bail, Context, Result};
use rquickjs::context::EvalOptions;
use rquickjs::function::{Async, Func};
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Function, Promise};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PRELUDE: &str = include_str!("prelude.js");

/// Sandbox limits for one plugin.
#[derive(Debug, Clone)]
pub struct PluginOptions {
    /// QuickJS heap limit in bytes.
    pub memory_limit: usize,
    /// Max native stack for JS in bytes.
    pub stack_limit: usize,
    /// Wall-clock budget of one plugin method call (including its network awaits).
    pub call_timeout: Duration,
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            memory_limit: 128 * 1024 * 1024,
            stack_limit: 1024 * 1024,
            call_timeout: Duration::from_secs(120),
        }
    }
}

/// A loaded, isolated JS source plugin (own QuickJS runtime and context).
pub struct Plugin {
    manifest: PluginManifest,
    methods: Vec<String>,
    // Declared before `rt` so the context drops first.
    ctx: AsyncContext,
    _rt: AsyncRuntime,
    opts: PluginOptions,
    start: Instant,
    deadline_ms: Arc<AtomicU64>,
}

/// Load `<dir>/manifest.json` + entry point using the default reqwest backend.
pub async fn load_plugin(dir: impl AsRef<Path>) -> Result<Plugin> {
    load_plugin_with(dir, PluginOptions::default(), Arc::new(ReqwestBackend::new())).await
}

/// Load a plugin directory with explicit limits and fetch backend.
pub async fn load_plugin_with(
    dir: impl AsRef<Path>,
    opts: PluginOptions,
    backend: Arc<dyn FetchBackend>,
) -> Result<Plugin> {
    let dir = dir.as_ref();
    let manifest = read_manifest(dir)?;
    let entry = dir.join(&manifest.entry_point);
    let code = std::fs::read_to_string(&entry)
        .with_context(|| format!("Plugin {}: entry point not found at {}", manifest.id, entry.display()))?;
    Plugin::from_source(manifest, &code, opts, backend).await
}

fn to_js_err(e: String) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message("host", "error", e)
}

impl Plugin {
    /// Build a plugin from in-memory code.
    pub async fn from_source(
        manifest: PluginManifest,
        code: &str,
        opts: PluginOptions,
        backend: Arc<dyn FetchBackend>,
    ) -> Result<Plugin> {
        let rt = AsyncRuntime::new().map_err(|e| anyhow!("quickjs runtime: {e}"))?;
        rt.set_memory_limit(opts.memory_limit).await;
        rt.set_max_stack_size(opts.stack_limit).await;

        let start = Instant::now();
        let deadline_ms = Arc::new(AtomicU64::new(0));
        {
            let d = deadline_ms.clone();
            rt.set_interrupt_handler(Some(Box::new(move || {
                let dl = d.load(Ordering::Relaxed);
                dl != 0 && start.elapsed().as_millis() as u64 + 1 > dl
            })))
            .await;
        }

        let ctx = AsyncContext::full(&rt).await.map_err(|e| anyhow!("quickjs context: {e}"))?;
        let id = manifest.id.clone();
        let code = code.to_string();

        let methods: Result<Vec<String>> = ctx
            .async_with(async |ctx| -> Result<Vec<String>> {
                install_natives(&ctx, &id, backend).map_err(|e| anyhow!("{e}"))?;
                let sloppy = |name: &str| {
                    let mut o = EvalOptions::default();
                    o.strict = false;
                    o.filename = Some(name.to_string());
                    o
                };
                ctx.eval_with_options::<(), _>(PRELUDE, sloppy("prelude.js"))
                    .catch(&ctx)
                    .map_err(|e| anyhow!("prelude: {e}"))?;
                // Plugin code gets the same wall-clock guard while evaluating.
                ctx.eval_with_options::<(), _>(code, sloppy(&format!("plugin:{id}/index.js")))
                    .catch(&ctx)
                    .map_err(|e| anyhow!("{e}"))?;
                let has: Function = ctx.globals().get("__has_source").map_err(|e| anyhow!("{e}"))?;
                if !has.call::<_, bool>(()).map_err(|e| anyhow!("{e}"))? {
                    bail!("Plugin {id} did not call registerSource()");
                }
                let m: Function = ctx.globals().get("__methods").map_err(|e| anyhow!("{e}"))?;
                let s: String = m.call(()).map_err(|e| anyhow!("{e}"))?;
                Ok(serde_json::from_str(&s)?)
            })
            .await;

        Ok(Plugin { manifest, methods: methods?, ctx, _rt: rt, opts, start, deadline_ms })
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Names of the functions the plugin registered.
    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn has_method(&self, name: &str) -> bool {
        self.methods.iter().any(|m| m == name)
    }

    /// Call a registered plugin function with JSON args; awaits the returned promise.
    pub async fn call(&self, method: &str, args: Vec<Value>) -> Result<Value> {
        if !self.has_method(method) {
            bail!("Plugin {} does not implement {method}", self.manifest.id);
        }
        let args_json = serde_json::to_string(&args)?;
        let timeout = self.opts.call_timeout;
        self.deadline_ms.store(
            self.start.elapsed().as_millis() as u64 + timeout.as_millis() as u64,
            Ordering::Relaxed,
        );
        let method_owned = method.to_string();
        let fut = self.ctx.async_with(async move |ctx| -> Result<String, String> {
            let call: Function = ctx.globals().get("__call").map_err(|e| e.to_string())?;
            let promise: Promise = call
                .call((method_owned, args_json))
                .catch(&ctx)
                .map_err(|e| e.to_string())?;
            promise
                .into_future::<String>()
                .await
                .catch(&ctx)
                .map_err(|e| e.to_string())
        });
        let res = tokio::time::timeout(timeout, fut).await;
        self.deadline_ms.store(0, Ordering::Relaxed);
        match res {
            Err(_) => bail!("Plugin {}: {method} timed out after {}s", self.manifest.id, timeout.as_secs_f32()),
            Ok(Err(e)) => bail!("Plugin {}: {method} failed: {}", self.manifest.id, clean_err(&e)),
            Ok(Ok(s)) => Ok(serde_json::from_str(&s)?),
        }
    }

    pub async fn search_manga(&self, query: &str, page: u32) -> Result<Value> {
        self.call("searchManga", vec![json!(query), json!(page)]).await
    }
    pub async fn get_manga_details(&self, manga_id: &str) -> Result<Value> {
        self.call("getMangaDetails", vec![json!(manga_id)]).await
    }
    pub async fn get_chapter_list(&self, manga_id: &str) -> Result<Value> {
        self.call("getChapterList", vec![json!(manga_id)]).await
    }
    pub async fn get_page_urls(&self, chapter_id: &str) -> Result<Value> {
        self.call("getPageUrls", vec![json!(chapter_id)]).await
    }
    pub async fn get_popular_manga(&self, page: u32) -> Result<Value> {
        self.call("getPopularManga", vec![json!(page)]).await
    }
    pub async fn get_latest_manga(&self, page: u32) -> Result<Value> {
        self.call("getLatestManga", vec![json!(page)]).await
    }
    pub async fn get_tag_list(&self) -> Result<Value> {
        self.call("getTagList", vec![]).await
    }
    pub async fn get_manga_by_tag(&self, tag_id: &str, page: u32) -> Result<Value> {
        self.call("getMangaByTag", vec![json!(tag_id), json!(page)]).await
    }
}

fn clean_err(e: &str) -> String {
    e.trim().to_string()
}

fn install_natives(ctx: &Ctx<'_>, plugin_id: &str, backend: Arc<dyn FetchBackend>) -> rquickjs::Result<()> {
    let g = ctx.globals();

    let pid = plugin_id.to_string();
    g.set(
        "__log",
        Func::from(move |level: String, msg: String| {
            eprintln!("[plugin:{pid}] [{level}] {msg}");
        }),
    )?;

    let (b, pid) = (backend.clone(), plugin_id.to_string());
    g.set(
        "__fetch_raw",
        Function::new(
            ctx.clone(),
            Async(move |url: String, opts: String| {
                let (b, pid) = (b.clone(), pid.clone());
                async move {
                    let mut req: FetchRequest = serde_json::from_str(&opts).unwrap_or_default();
                    req.url = url;
                    let out = match b.fetch(&pid, req).await {
                        Ok(r) => serde_json::to_string(&r).map_err(|e| to_js_err(e.to_string()))?,
                        Err(e) => json!({ "error": e }).to_string(),
                    };
                    Ok::<String, rquickjs::Error>(out)
                }
            }),
        )?,
    )?;

    let (b, pid) = (backend.clone(), plugin_id.to_string());
    g.set(
        "__render_raw",
        Function::new(
            ctx.clone(),
            Async(move |url: String, opts: String| {
                let (b, pid) = (b.clone(), pid.clone());
                async move {
                    let opts: Value = serde_json::from_str(&opts).unwrap_or(Value::Null);
                    let out = match b.render(&pid, url, opts).await {
                        Ok(r) => serde_json::to_string(&r).map_err(|e| to_js_err(e.to_string()))?,
                        Err(e) => json!({ "error": e }).to_string(),
                    };
                    Ok::<String, rquickjs::Error>(out)
                }
            }),
        )?,
    )?;

    let (b, pid) = (backend, plugin_id.to_string());
    g.set(
        "__solve_raw",
        Function::new(
            ctx.clone(),
            Async(move |url: String| {
                let (b, pid) = (b.clone(), pid.clone());
                async move {
                    let out = match b.solve(&pid, url).await {
                        Ok(ok) => json!({ "ok": ok }).to_string(),
                        Err(e) => json!({ "error": e }).to_string(),
                    };
                    Ok::<String, rquickjs::Error>(out)
                }
            }),
        )?,
    )?;
    Ok(())
}
