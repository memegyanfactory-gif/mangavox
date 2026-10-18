use mv_plugins::repo;
use mv_plugins::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;

/// Mock fetch: first route whose key is a substring of the URL wins. A "HANG" route never resolves.
#[derive(Default)]
struct Mock {
    routes: Vec<(String, u16, String)>,
    seen: Mutex<Vec<FetchRequest>>,
}
impl Mock {
    fn route(mut self, key: &str, status: u16, body: &str) -> Self {
        self.routes.push((key.into(), status, body.into()));
        self
    }
}
impl FetchBackend for Mock {
    fn fetch(&self, _id: &str, req: FetchRequest) -> BoxFut<FetchResponse> {
        self.seen.lock().unwrap().push(req.clone());
        let hit = self.routes.iter().find(|(k, ..)| req.url.contains(k.as_str())).cloned();
        Box::pin(async move {
            match hit {
                Some((k, ..)) if k == "HANG" => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    unreachable!()
                }
                Some((_, st, body)) => Ok(FetchResponse::from_text(st, body, req.url, BTreeMap::new())),
                None => Err(format!("no mock route for {}", req.url)),
            }
        })
    }
}

fn manifest(id: &str) -> PluginManifest {
    PluginManifest::parse(&json!({"id": id, "name": id}).to_string()).unwrap()
}

async fn inline(code: &str, mock: Mock) -> Plugin {
    Plugin::from_source(manifest("t"), code, PluginOptions::default(), Arc::new(mock)).await.unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mv-plugins-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn async_methods_fetch_and_polyfills() {
    let code = r#"
      var BASE = 'https://x.test'
      registerSource({
        searchManga: async function (q, page) {
          var u = BASE + '/s?' + new URLSearchParams({ q: q, page: page }).toString()
          var r = await __fetch(u, { headers: { A: 'b' } })
          console.log('got', r.status)
          return r.data.items.map(function (i) { return { source_id: i, title: i.toUpperCase() } })
        },
        getPageUrls: function (id) {
          var abs = new URL('../img/1.png?x=1', 'https://h.test/a/b/c').href
          return Promise.resolve([abs, atob(btoa('hello')), new URL('https://H.test:443/p').origin])
        },
        getTagList: async function () { return undefined }
      })"#;
    let mock = Mock::default().route("x.test/s", 200, r#"{"items":["ab","cd"]}"#);
    let p = inline(code, mock).await;
    assert!(p.has_method("searchManga") && !p.has_method("getLatestManga"));
    let r = p.search_manga("a b", 2).await.unwrap();
    assert_eq!(r, json!([{"source_id":"ab","title":"AB"},{"source_id":"cd","title":"CD"}]));
    let u = p.get_page_urls("c").await.unwrap();
    assert_eq!(u, json!(["https://h.test/a/img/1.png?x=1", "hello", "https://h.test"]));
    assert_eq!(p.get_tag_list().await.unwrap(), Value::Null);
    assert!(p.get_latest_manga(1).await.is_err());
}

struct Shared(Arc<Mock>);
impl FetchBackend for Shared {
    fn fetch(&self, id: &str, r: FetchRequest) -> BoxFut<FetchResponse> {
        self.0.fetch(id, r)
    }
}

#[tokio::test]
async fn fetch_options_and_errors() {
    let code = r#"registerSource({
      searchManga: async function () {
        var r = await __fetch('https://x.test/post', { method: 'POST', body: { a: 1 }, referer: 'https://r/' })
        try { await __fetch('https://nowhere.test/') } catch (e) { return [r.status, 'caught:' + e.message] }
      },
      getMangaDetails: async function () { throw new Error('boom') }
    })"#;
    let mock = Arc::new(Mock::default().route("x.test/post", 201, "{}"));
    let p = Plugin::from_source(manifest("t"), code, PluginOptions::default(), Arc::new(Shared(mock.clone())))
        .await
        .unwrap();
    let r = p.search_manga("", 1).await.unwrap();
    assert_eq!(r[0], 201);
    assert!(r[1].as_str().unwrap().starts_with("caught:no mock route"));
    let seen = mock.seen.lock().unwrap();
    assert_eq!(seen[0].method.as_deref(), Some("POST"));
    assert_eq!(seen[0].body, Some(json!({"a": 1})));
    assert_eq!(seen[0].referer.as_deref(), Some("https://r/"));
    drop(seen);
    let e = p.get_manga_details("x").await.unwrap_err().to_string();
    assert!(e.contains("boom"), "{e}");
}

#[tokio::test]
async fn missing_register_source_fails() {
    let r = Plugin::from_source(manifest("t"), "var a = 1", PluginOptions::default(), Arc::new(Mock::default())).await;
    assert!(r.err().unwrap().to_string().contains("registerSource"));
    let r = Plugin::from_source(manifest("t"), "this is not js", PluginOptions::default(), Arc::new(Mock::default())).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn timeouts_and_memory_limit() {
    let opts = PluginOptions { call_timeout: Duration::from_millis(400), memory_limit: 16 * 1024 * 1024, ..Default::default() };
    let code = r#"registerSource({
      searchManga: async function () { for (;;) {} },
      getMangaDetails: async function () { return await __fetch('https://hang.test/HANG') },
      getChapterList: async function () { var a = []; for (var i = 0; i < 1e8; i++) a.push('x'.repeat(1000) + i); return a.length },
      getPopularManga: async function () { return 'ok' }
    })"#;
    let p = Plugin::from_source(manifest("t"), code, opts, Arc::new(Mock::default().route("HANG", 200, ""))).await.unwrap();
    let t = std::time::Instant::now();
    assert!(p.search_manga("", 1).await.is_err());
    assert!(t.elapsed() < Duration::from_secs(5));
    let e = p.get_manga_details("").await.unwrap_err().to_string();
    assert!(e.contains("timed out"), "{e}");
    assert!(p.get_chapter_list("").await.is_err());
    // still usable afterwards
    assert_eq!(p.get_popular_manga(1).await.unwrap(), json!("ok"));
}

#[tokio::test]
async fn bundled_manifests_load() {
    for (id, name) in [("webtoons", "WEBTOON"), ("mangadex", "MangaDex")] {
        let p = load_plugin_with(fixture(id), PluginOptions::default(), Arc::new(Mock::default())).await.unwrap();
        let m = p.manifest();
        assert_eq!((m.id.as_str(), m.name.as_str(), m.lang.as_str(), m.entry_point.as_str()), (id, name, "en", "index.js"));
        assert_eq!(m.version, "1.0.0");
        for f in ["searchManga", "getMangaDetails", "getChapterList", "getPageUrls", "getPopularManga", "getLatestManga"] {
            assert!(p.has_method(f), "{id} lacks {f}");
        }
    }
    assert!(load_plugin_with(fixture("nope"), PluginOptions::default(), Arc::new(Mock::default())).await.is_err());
}

#[tokio::test]
async fn webtoons_search_with_mock_html() {
    let html = r#"<ul><li><a class="_card_item" href="https://www.webtoons.com/en/fantasy/tower-of-god/list?title_no=95">
      <img src="https://cdn/x.jpg"><strong class="title">Tower of God</strong><div class="author">SIU</div></a></li></ul>"#;
    let mock = Mock::default().route("webtoons.com/en/search", 200, html);
    let p = load_plugin_with(fixture("webtoons"), PluginOptions::default(), Arc::new(mock)).await.unwrap();
    let r = p.search_manga("tower", 1).await.unwrap();
    assert_eq!(r[0]["source_id"], "fantasy|tower-of-god|95");
    assert_eq!(r[0]["title"], "Tower of God");
    assert_eq!(r[0]["author"], "SIU");
}

#[tokio::test]
async fn mangadex_search_with_mock_json() {
    let mock = Mock::default().route("api.mangadex.org/manga", 200, r#"{"data":[]}"#);
    let p = load_plugin_with(fixture("mangadex"), PluginOptions::default(), Arc::new(mock)).await.unwrap();
    assert_eq!(p.search_manga("x", 1).await.unwrap(), json!([]));
}

#[test]
fn repo_url_normalisation() {
    use repo::{normalize_repo_url as n, resolve_plugin_url as r};
    assert_eq!(n("https://github.com/u/r"), "https://raw.githubusercontent.com/u/r/repo/index.min.json");
    assert_eq!(n("https://github.com/u/r.git/"), "https://raw.githubusercontent.com/u/r/repo/index.min.json");
    assert_eq!(n("https://github.com/u/r/tree/dev/sub/dir"), "https://raw.githubusercontent.com/u/r/dev/sub/dir/index.min.json");
    assert_eq!(n("https://raw.githubusercontent.com/u/r/repo"), "https://raw.githubusercontent.com/u/r/repo/index.min.json");
    assert_eq!(n("https://e.com/index.json"), "https://e.com/index.json");
    assert_eq!(n("https://e.com/x"), "https://e.com/x/index.min.json");
    assert_eq!(r("https://github.com/u/r").unwrap().1, "https://raw.githubusercontent.com/u/r/main/");
    assert_eq!(r("https://e.com/p/manifest.json").unwrap(), ("https://e.com/p/manifest.json".into(), "https://e.com/p/".into()));
    assert_eq!(r("https://e.com/p").unwrap().0, "https://e.com/p/manifest.json");
}

#[tokio::test]
async fn repo_index_formats() {
    let classic = r#"[{"name":"Foo","pkg":"eu.kanade.tachiyomi.extension.en.foo","apk":"foo.apk","lang":"en","code":3,"version":"1.4.3","nsfw":0,"sources":[{"name":"Foo","lang":"en","id":"123","baseUrl":"https://foo.test"}]}]"#;
    let stub = r#"[{"name":"Tachiyomi: Outdated app","pkg":"extension.all.keiyoushi","apk":"","lang":"all","code":1,"version":"1","nsfw":0,"sources":[]}]"#;
    let modern = r#"{"extensionList":{"extensions":[{"name":"Bar","packageName":"x.extension.fr.bar","versionName":"2.0","versionCode":7,"contentWarning":"CONTENT_WARNING_NSFW","resources":{"apkUrl":"https://c/d/bar.apk"},"sources":[{"name":"Bar","id":5,"homeUrl":"https://bar.test"}]}]}}"#;
    assert_eq!(repo::parse_repo_index(classic).unwrap()[0].sources[0].base_url, "https://foo.test");
    assert!(repo::parse_repo_index(stub).is_none());
    let m = &repo::parse_repo_index(modern).unwrap()[0];
    assert_eq!((m.lang.as_str(), m.apk.as_str(), m.nsfw, m.code), ("fr", "bar.apk", 1, 7));

    // stub at index.min.json -> falls back to index.json
    let mock = Mock::default().route("index.min.json", 200, stub).route("index.json", 200, modern);
    let exts = repo::fetch_repo_extensions("https://e.com/r", &mock).await.unwrap();
    assert_eq!(exts[0].name, "Bar");
}

#[tokio::test]
async fn install_list_uninstall() {
    let dir = tmpdir("install");
    let code = std::fs::read_to_string(fixture("webtoons").join("index.js")).unwrap();
    let man = std::fs::read_to_string(fixture("webtoons").join("manifest.json")).unwrap();
    let mock = Arc::new(Mock::default().route("manifest.json", 200, &man).route("index.js", 200, &code));
    let m = repo::install_plugin_from_url("https://e.com/p/", &dir, Arc::new(Shared(mock.clone()))).await.unwrap();
    assert_eq!(m.id, "webtoons");
    assert!(dir.join("webtoons/index.js").exists() && dir.join("webtoons/.source_url").exists());
    assert_eq!(repo::list_installed(&dir).len(), 1);
    assert_eq!(repo::check_plugin_update(&dir, "webtoons", mock.as_ref()).await.unwrap(), None);
    let p = load_plugin_with(dir.join("webtoons"), PluginOptions::default(), Arc::new(Shared(mock))).await.unwrap();
    assert!(p.has_method("searchManga"));
    assert!(repo::uninstall_plugin(&dir, "webtoons").unwrap());
    assert!(!repo::uninstall_plugin(&dir, "webtoons").unwrap());
    assert!(repo::uninstall_plugin(&dir, "../x").is_err());

    // invalid plugin code is rejected and nothing is written
    let bad = Mock::default().route("manifest.json", 200, &man).route("index.js", 200, "var nothing = 12345");
    assert!(repo::install_plugin_from_url("https://e.com/p/", &dir, Arc::new(bad)).await.is_err());
    assert!(repo::list_installed(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "live network"]
async fn live_webtoons_search() {
    let p = load_plugin(fixture("webtoons")).await.unwrap();
    let r = p.search_manga("tower of god", 1).await.unwrap();
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
    assert!(r.as_array().is_some_and(|a| !a.is_empty()));
}
