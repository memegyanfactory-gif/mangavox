use mv_plugins::repo;
use mv_plugins::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;

/// Mock fetch: first route whose key is a substring of the URL (or equals it, for "=url" keys) wins.
/// A "HANG" route never resolves.
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
        // A key starting with '=' must equal the URL; any other key matches as a substring.
        let hit = self
            .routes
            .iter()
            .find(|(k, ..)| match k.strip_prefix('=') {
                Some(exact) => req.url == exact,
                None => req.url.contains(k.as_str()),
            })
            .cloned();
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
    {
        let seen = mock.seen.lock().unwrap();
        assert_eq!(seen[0].method.as_deref(), Some("POST"));
        assert_eq!(seen[0].body, Some(json!({"a": 1})));
        assert_eq!(seen[0].referer.as_deref(), Some("https://r/"));
    }
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

// ------------------------------------------------------------------------------------------------
// Repositories and repo-extension installs (generated template plugins), all against the mock backend.
// ------------------------------------------------------------------------------------------------

fn repo_ext(pkg: &str, name: &str, lang: &str, base: &str) -> repo::RepoExtension {
    repo::RepoExtension {
        name: name.into(),
        pkg: pkg.into(),
        apk: "x.apk".into(),
        lang: lang.into(),
        code: 1,
        version: "1.4.1".into(),
        nsfw: 0,
        sources: vec![repo::RepoSource { name: name.into(), lang: lang.into(), id: json!("1"), base_url: base.into() }],
        icon_url: None,
    }
}

/// `#mvref=<encodeURIComponent(base + "/")>`, the Referer hint the templates append to image URLs.
fn mvref(base: &str) -> String {
    format!("#mvref={}%2F", base.replace(':', "%3A").replace('/', "%2F"))
}

async fn install(mock: Mock, e: &repo::RepoExtension, tag: &str) -> (PathBuf, repo::RepoInstallResult, Arc<Mock>) {
    let dir = tmpdir(tag);
    let mock = Arc::new(mock);
    let r = repo::install_repo_extension(e, 0, &dir, Arc::new(Shared(mock.clone()))).await.unwrap();
    (dir, r, mock)
}

async fn load(dir: &Path, id: &str, mock: &Arc<Mock>) -> Plugin {
    load_plugin_with(dir.join(id), PluginOptions::default(), Arc::new(Shared(mock.clone()))).await.unwrap()
}

fn titles(v: &Value) -> Vec<&str> {
    v.as_array().unwrap().iter().map(|m| m["title"].as_str().unwrap()).collect()
}

#[test]
fn repos_json_roundtrip_and_electron_compat() {
    let root = tmpdir("repos");
    assert!(repo::list_repos(&root).is_empty());

    // The file the Electron app wrote.
    let kei = "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";
    std::fs::write(root.join("repos.json"), format!("{{\n  \"repos\": [\n    \"{kei}\"\n  ]\n}}")).unwrap();
    assert_eq!(repo::list_repos(&root), vec![kei.to_string()]);

    // Pasted forms are normalised, duplicates collapse.
    assert_eq!(repo::add_repo(&root, "https://github.com/u/r").unwrap(), "https://raw.githubusercontent.com/u/r/repo/index.min.json");
    repo::add_repo(&root, "https://github.com/u/r.git/").unwrap();
    assert_eq!(repo::list_repos(&root).len(), 2);
    // Mihon / Tachiyomi share links.
    let deep = "tachiyomi://add-repo?url=https%3A%2F%2Fe.com%2Frepo%2Findex.min.json";
    assert_eq!(repo::add_repo(&root, deep).unwrap(), "https://e.com/repo/index.min.json");
    assert_eq!(repo::normalize_repo_url("mihon://add-repo?url=https://e.com/x/index.json"), "https://e.com/x/index.json");
    // Links meant for newer apps: the protobuf index (and repo.json), also as GitHub raw/blob links.
    let kei = "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";
    assert_eq!(repo::normalize_repo_url("https://github.com/keiyoushi/extensions/raw/repo/index.pb"), kei);
    assert_eq!(repo::normalize_repo_url("https://github.com/keiyoushi/extensions/blob/repo/repo.json"), kei);
    assert_eq!(repo::normalize_repo_url("https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.pb"), kei);
    assert_eq!(repo::normalize_repo_url("https://github.com/keiyoushi/extensions/raw/repo/index.json"), "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.json");
    assert_eq!(repo::list_repos(&root).len(), 3);
    assert!(repo::add_repo(&root, "   ").is_err());
    assert!(repo::add_repo(&root, "not a url").is_err());

    // Written back in the Electron shape: {"repos": [...]} in order.
    let v: Value = serde_json::from_str(&std::fs::read_to_string(root.join("repos.json")).unwrap()).unwrap();
    assert_eq!(v["repos"][0], kei);
    assert_eq!(v["repos"].as_array().unwrap().len(), 3);

    // Remove by the pasted form or the stored one; unknown = false.
    assert!(repo::remove_repo(&root, "https://github.com/u/r").unwrap());
    assert!(!repo::remove_repo(&root, "https://github.com/u/r").unwrap());
    assert!(repo::remove_repo(&root, kei).unwrap());
    assert_eq!(repo::list_repos(&root), vec!["https://e.com/repo/index.min.json".to_string()]);

    // A corrupt file reads as empty and is replaced on the next add.
    std::fs::write(root.join("repos.json"), "{ not json").unwrap();
    assert!(repo::list_repos(&root).is_empty());
    repo::add_repo(&root, "https://github.com/u/r").unwrap();
    assert_eq!(repo::list_repos(&root).len(), 1);
    assert!(!root.join("repos.json.tmp").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn modern_index_string_version_code_and_icon() {
    let modern = r#"{"extensionList":{"extensions":[{"name":"AHottie","packageName":"eu.kanade.tachiyomi.extension.all.ahottie","resources":{"apkUrl":"https://github.com/k/e/releases/download/19c8e5f-0/tachiyomi-all.ahottie-v1.6.4.apk","iconUrl":"https://cdn.jsdelivr.net/gh/k/s@main/src/all/ahottie/res/ic_launcher.png"},"versionCode":"106004","versionName":"1.6.4","contentWarning":"CONTENT_WARNING_NSFW","sources":[{"id":"6289731484943315811","name":"AHottie","language":"all","homeUrl":"https://ahottie.top"}]}]}}"#;
    let e = &repo::parse_repo_index(modern).unwrap()[0];
    // The real Keiyoushi index writes versionCode as a string.
    assert_eq!((e.code, e.version.as_str(), e.lang.as_str(), e.apk.as_str()), (106004, "1.6.4", "all", "tachiyomi-all.ahottie-v1.6.4.apk"));
    assert_eq!(repo::extension_icon_url("https://x/index.json", e), "https://cdn.jsdelivr.net/gh/k/s@main/src/all/ahottie/res/ic_launcher.png");
    // Classic repos keep icons at <repo>/icon/<pkg>.png and never write iconUrl (Electron-compatible file).
    let mut classic = e.clone();
    classic.icon_url = None;
    assert_eq!(repo::extension_icon_url("https://x/r/index.min.json", &classic), "https://x/r/icon/eu.kanade.tachiyomi.extension.all.ahottie.png");
    assert!(!serde_json::to_string(&classic).unwrap().contains("iconUrl"));
    let back: repo::RepoExtension = serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap();
    assert_eq!(&back, e);
}

#[tokio::test]
async fn add_repo_verified_checks_the_index_first() {
    let root = tmpdir("repos-verified");
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.foo", "Foo", "en", "https://foo.test");
    // One malformed entry does not hide the rest.
    let index = format!(r#"[{}, {{"name": null, "pkg": 5}}]"#, serde_json::to_string(&e).unwrap());
    let mock = Mock::default().route("r.test/repo/index.min.json", 200, &index).route("empty.test/", 200, "[]");

    let (url, exts) = repo::add_repo_verified(&root, "https://r.test/repo", &mock).await.unwrap();
    assert_eq!(url, "https://r.test/repo/index.min.json");
    assert_eq!(exts, vec![e.clone()]);
    assert_eq!(repo::list_repos(&root), vec![url]);

    // Nothing is saved for an unreachable, invalid or empty repo.
    assert!(repo::add_repo_verified(&root, "https://nowhere.test/x", &mock).await.is_err());
    let err = repo::add_repo_verified(&root, "https://empty.test/", &mock).await.unwrap_err().to_string();
    assert!(err.contains("does not list any extensions"), "{err}");
    assert!(repo::add_repo_verified(&root, "", &mock).await.is_err());
    assert_eq!(repo::list_repos(&root).len(), 1);

    assert_eq!(repo::extension_icon_url("https://r.test/repo/index.min.json", &e), "https://r.test/repo/icon/eu.kanade.tachiyomi.extension.en.foo.png");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn engine_detection_matches_the_embedded_signatures() {
    use mv_plugins::templates::detect_engine_from_html as d;
    assert_eq!(d(r#"<div class="page-item-detail">"#), Engine::Madara);
    assert_eq!(d(r#"<div class="listupd"><div class="bsx">"#), Engine::MangaThemesia);
    assert_eq!(d(r#"<div class="panel-content-genres">"#), Engine::MangaBox);
    assert_eq!(d("Powered by FoOlSlide"), Engine::FoolSlide);
    assert_eq!(d(r#"<a href="/series/?genre=action"></a><img src="/assets/images/x.png">"#), Engine::KeyoApp);
    assert_eq!(d(r#"{"series_slug":"a"}"#), Engine::HeanCms);
    assert_eq!(d("<html>nothing</html>"), Engine::Generic);
    assert_eq!(d(""), Engine::Generic);
    // Search pages echo our own ?post_type=wp-manga query: that alone is not Madara.
    assert_eq!(d(r#"<a href="/?s=x&post_type=wp-manga">"#), Engine::Generic);
}

#[tokio::test]
async fn install_madara_extension_and_browse() {
    let b = "https://m.test";
    let list = r#"<div class="page-item-detail manga"><div class="item-thumb"><a href="https://m.test/manga/one-piece/" title="One Piece"><img data-src="https://m.test/c/op.jpg" src="https://m.test/blank.gif"></a></div>
        <div class="item-summary"><div class="post-title font-title"><h3 class="h5"><a href="https://m.test/manga/one-piece/">One Piece</a></h3></div></div></div>
      <div class="page-item-detail manga"><div class="item-summary"><div class="post-title"><h3><a href="https://m.test/manga/naruto/">Naruto</a></h3></div></div></div>"#;
    let details = r#"<div class="post-title"><h1><span class="manga-title-badges hot">HOT</span> One Piece</h1></div>
        <div class="summary_image"><a><img data-src="https://m.test/c/op.jpg"></a></div>
        <div class="author-content"><a href="/a/oda/">Oda Eiichiro</a></div>
        <div class="post-content_item"><div class="summary-heading"><h5>Status</h5></div><div class="summary-content">OnGoing</div></div>
        <div class="genres-content"><a href="/g/action/">Action</a>, <a href="/g/adventure/">Adventure</a></div>
        <div class="description-summary"><div class="summary__content"><p>Pirates.</p></div></div>
        <ul class="main version-chap">
          <li class="wp-manga-chapter"><a href="https://m.test/manga/one-piece/chapter-2/">Chapter 2</a><span class="chapter-release-date"><i>March 3, 2024</i></span></li>
          <li class="wp-manga-chapter"><a href="https://m.test/manga/one-piece/chapter-1/">Chapter 1</a><span class="chapter-release-date"><i>March 1, 2024</i></span></li>
        </ul>"#;
    let pages = r#"<div class="reading-content"><div class="page-break"><img data-src="https://cdn.test/1.jpg"></div><div class="page-break"><img data-src="https://cdn.test/2.jpg"></div></div>"#;
    let mock = Mock::default()
        .route("m.test/manga/one-piece/chapter-1/?style=list", 200, pages)
        .route("=https://m.test/manga/one-piece/", 200, details)
        .route("m.test/manga/?m_orderby=views", 200, list)
        .route("=https://m.test/", 200, list);
    let mut e = repo_ext("eu.kanade.tachiyomi.extension.en.onepiece", "One Piece Site", "en", b);
    e.nsfw = 1;

    // The install future must be Send so the UI can run it on its background runtime.
    fn is_send<T: Send>(_: &T) {}
    let dir = tmpdir("madara");
    let mock = Arc::new(mock);
    let fut = repo::install_repo_extension(&e, 0, &dir, Arc::new(Shared(mock.clone())));
    is_send(&fut);
    let r = fut.await.unwrap();

    assert_eq!(r.engine, Engine::Madara);
    assert_eq!(r.id, "repo-eu-kanade-tachiyomi-extension-en-onepiece-en");
    assert_eq!(r.id, repo::repo_plugin_id(&e, 0));
    assert_eq!(r.warning, None);

    // On disk: manifest (Electron-compatible fields + nsfw), generated code, .repo_source marker.
    let m = read_manifest(dir.join(&r.id)).unwrap();
    assert_eq!((m.name.as_str(), m.version.as_str(), m.lang.as_str(), m.base_url.as_str()), ("One Piece Site", "1.4.1", "en", b));
    assert!(m.nsfw);
    assert_eq!(m.extra["engine"], "madara");
    assert_eq!(m.extra["generator"], "mangavox-template");
    assert_eq!(m.extra["template_version"], json!(mv_plugins::templates::TEMPLATE_VERSION));
    assert_eq!(m.extra["repo_pkg"], "eu.kanade.tachiyomi.extension.en.onepiece");
    assert_eq!(m.extra["id_style"], "path");
    let code = std::fs::read_to_string(dir.join(&r.id).join("index.js")).unwrap();
    assert!(code.starts_with("// MangaVox template plugin v2 (engine: madara)"));
    assert_eq!(repo::read_repo_source(&dir, &r.id), Some(e.clone()));
    assert_eq!(repo::read_repo_source(&dir, "nope"), None);
    assert!(!dir.join(&r.id).join("index.js.tmp").exists());
    assert_eq!(repo::list_installed(&dir).len(), 1);
    let inst = repo::installed_repo_plugins(&dir);
    assert_eq!((inst.len(), inst[0].extension.clone(), inst[0].manifest.id.clone()), (1, e.clone(), r.id.clone()));

    // The generated plugin runs in the sandbox against the prelude globals.
    let p = load(&dir, &r.id, &mock).await;
    for f in ["searchManga", "getMangaDetails", "getChapterList", "getPageUrls", "getPopularManga", "getLatestManga", "getTagList", "getMangaByTag"] {
        assert!(p.has_method(f), "missing {f}");
    }
    let hint = mvref(b);
    let popular = p.get_popular_manga(1).await.unwrap();
    assert_eq!(titles(&popular), ["One Piece", "Naruto"]);
    assert_eq!(popular[0]["source_id"], "manga/one-piece/");
    assert_eq!(popular[0]["cover_url"], format!("https://m.test/c/op.jpg{hint}"));

    let d = p.get_manga_details("manga/one-piece/").await.unwrap();
    assert_eq!(d["title"], "One Piece");
    assert_eq!(d["author"], "Oda Eiichiro");
    assert_eq!(d["status"], "ongoing");
    assert_eq!(d["genres"], json!(["Action", "Adventure"]));
    assert_eq!(d["description"], "Pirates.");
    assert_eq!(d["cover_url"], format!("https://m.test/c/op.jpg{hint}"));

    let ch = p.get_chapter_list("manga/one-piece/").await.unwrap();
    assert_eq!(ch.as_array().unwrap().len(), 2);
    assert_eq!(ch[0]["source_id"], "manga/one-piece/chapter-1/");
    assert_eq!((ch[0]["number"].as_f64(), ch[1]["number"].as_f64()), (Some(1.0), Some(2.0)));
    assert_eq!(ch[0]["title"], "Chapter 1");
    assert_eq!(ch[0]["language"], "en");
    assert_eq!(ch[0]["published_at"], "2024-03-01T12:00:00.000Z");

    let pages = p.get_page_urls("manga/one-piece/chapter-1/").await.unwrap();
    assert_eq!(pages, json!([format!("https://cdn.test/1.jpg{hint}"), format!("https://cdn.test/2.jpg{hint}")]));

    // Removal works like any plugin and takes the marker with it.
    assert!(repo::uninstall_plugin(&dir, &r.id).unwrap());
    assert!(repo::installed_repo_plugins(&dir).is_empty() && repo::list_installed(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_mangathemesia_extension() {
    let list = r#"<div class="listupd"><div class="bs"><div class="bsx"><a href="https://t.test/manga/solo/" title="Solo Leveling"><div class="limit"><img src="https://t.test/c/solo.jpg"></div><div class="bigor"><div class="tt">Solo Leveling</div></div></a></div></div></div>"#;
    let details = r#"<div class="eplister" id="chapterlist"><ul>
        <li data-num="2"><div class="chbox"><div class="eph-num"><a href="https://t.test/solo-chapter-2/"><span class="chapternum">Chapter 2</span><span class="chapterdate">March 3, 2024</span></a></div></div></li>
        <li data-num="1"><div class="chbox"><div class="eph-num"><a href="https://t.test/solo-chapter-1/"><span class="chapternum">Chapter 1</span><span class="chapterdate">March 1, 2024</span></a></div></div></li></ul></div>"#;
    let reader = r#"<script>ts_reader.run({"post_id":1,"sources":[{"source":"Server 1","images":["https://t.test/p/1.jpg","https://t.test/p/2.jpg"]}]});</script>"#;
    let mock = Mock::default()
        .route("=https://t.test/solo-chapter-2/", 200, reader)
        .route("t.test/manga/solo/", 200, details)
        .route("t.test/manga/?page=1&order=popular", 200, list)
        .route("=https://t.test/", 200, list);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.solo", "Solo", "en", "https://t.test");
    let (dir, r, mock) = install(mock, &e, "mangathemesia").await;
    assert_eq!((r.engine, r.warning), (Engine::MangaThemesia, None));
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!((titles(&pop), pop[0]["source_id"].as_str()), (vec!["Solo Leveling"], Some("manga/solo/")));
    let ch = p.get_chapter_list("manga/solo/").await.unwrap();
    assert_eq!(ch[0]["source_id"], "solo-chapter-1/");
    assert_eq!((ch[0]["number"].as_f64(), ch[1]["number"].as_f64()), (Some(1.0), Some(2.0)));
    let pages = p.get_page_urls("solo-chapter-2/").await.unwrap();
    let hint = mvref("https://t.test");
    assert_eq!(pages, json!([format!("https://t.test/p/1.jpg{hint}"), format!("https://t.test/p/2.jpg{hint}")]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_mangabox_extension() {
    let list = r#"<div class="panel-content-genres"><div class="content-genres-item"><a class="genres-item-img" href="https://b.test/manga/abc"><img src="https://b.test/c/abc.jpg"></a><div class="genres-item-info"><h3><a class="genres-item-name" href="https://b.test/manga/abc" title="ABC Manga">ABC Manga</a></h3></div></div></div>"#;
    let details = r#"<ul class="row-content-chapter">
        <li class="a-h"><a class="chapter-name" href="https://b.test/manga/abc/chapter-2" title="ABC chapter 2">Chapter 2</a><span class="chapter-time" title="Mar 03, 2024 12:00">Mar 03,24</span></li>
        <li class="a-h"><a class="chapter-name" href="https://b.test/manga/abc/chapter-1">Chapter 1</a><span class="chapter-time" title="Mar 01, 2024 12:00">Mar 01,24</span></li></ul>"#;
    let reader = r#"<script>var cdns = ["https://cdn.b.test/"]; var chapterImages = ["a/1.jpg","a/2.jpg"];</script>"#;
    let mock = Mock::default()
        .route("=https://b.test/manga/abc/chapter-2", 200, reader)
        .route("=https://b.test/manga/abc", 200, details)
        .route("b.test/manga-list/hot-manga?page=1", 200, list)
        .route("=https://b.test/", 200, list);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.abc", "ABC", "en", "https://b.test");
    let (dir, r, mock) = install(mock, &e, "mangabox").await;
    assert_eq!((r.engine, r.warning), (Engine::MangaBox, None));
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!((titles(&pop), pop[0]["source_id"].as_str()), (vec!["ABC Manga"], Some("manga/abc")));
    let ch = p.get_chapter_list("manga/abc").await.unwrap();
    assert_eq!((ch[0]["source_id"].as_str(), ch[0]["number"].as_f64()), (Some("manga/abc/chapter-1"), Some(1.0)));
    let hint = mvref("https://b.test");
    assert_eq!(p.get_page_urls("manga/abc/chapter-2").await.unwrap(), json!([format!("https://cdn.b.test/a/1.jpg{hint}"), format!("https://cdn.b.test/a/2.jpg{hint}")]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_foolslide_extension() {
    let home = r#"<a href="/foolslide/">Powered by FoOlSlide</a><div class="group"><div class="title"><a href="https://f.test/series/one/" title="One">One</a></div><img class="preview" src="https://f.test/c/one.jpg"></div>"#;
    let details = r#"<div class="list"><div class="element"><div class="title"><a href="https://f.test/read/one/en/0/2/" title="Chapter 2">Chapter 2</a></div><div class="meta_r">by <a href="/team/">Team</a>, 2024.03.03</div></div>
        <div class="element"><div class="title"><a href="https://f.test/read/one/en/0/1/" title="Chapter 1">Chapter 1</a></div><div class="meta_r">by <a href="/team/">Team</a>, 2024.03.01</div></div></div>"#;
    let reader = r#"<script>var pages = [{"url":"https://f.test/p/1.jpg"},{"url":"https://f.test/p/2.jpg"}];</script>"#;
    let mock = Mock::default()
        .route("=https://f.test/read/one/en/0/2/page/1", 200, reader)
        .route("=https://f.test/series/one/", 200, details)
        .route("=https://f.test/directory/1/", 200, home)
        .route("=https://f.test/", 200, home);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.fool", "Fool", "en", "https://f.test");
    let (dir, r, mock) = install(mock, &e, "foolslide").await;
    assert_eq!((r.engine, r.warning), (Engine::FoolSlide, None));
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!((titles(&pop), pop[0]["source_id"].as_str()), (vec!["One"], Some("series/one/")));
    let ch = p.get_chapter_list("series/one/").await.unwrap();
    assert_eq!((ch[0]["source_id"].as_str(), ch[0]["number"].as_f64(), ch[0]["scanlation_group"].as_str()), (Some("read/one/en/0/1/"), Some(1.0), Some("Team")));
    assert_eq!(p.get_page_urls("read/one/en/0/2/").await.unwrap().as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_heancms_extension_discovers_the_api_host() {
    // The home page is a bare SPA shell: only the API probe can tell this is HeanCMS.
    let home = r#"<html><script src="/_next/static/chunk.js"></script></html>"#;
    let popular = r#"{"data":[{"series_slug":"hero","title":"Hero","thumbnail":"covers/hero.jpg","status":"Ongoing"}]}"#;
    let mock = Mock::default()
        .route("api.h.test/chapter/hero/chapter-1", 200, r#"{"chapter":{"chapter_data":{"images":["https://img.test/1.jpg","pages/2.jpg"]}}}"#)
        .route("api.h.test/chapter/query?page=1&perPage=9999&series_id=7", 200, r#"{"data":[{"chapter_slug":"chapter-1","chapter_name":"Chapter 1","index":"1","created_at":"2024-03-01T00:00:00Z","price":0},{"chapter_slug":"vip","chapter_name":"VIP","index":"2","price":5}]}"#)
        .route("api.h.test/series/hero", 200, r#"{"id":7,"title":"Hero"}"#)
        .route("api.h.test/query?query_string=", 200, popular)
        .route("api.h.test/query?page=1&perPage=1", 200, popular)
        .route("=https://h.test/", 200, home);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.hero", "Hero", "en", "https://h.test");
    let (dir, r, mock) = install(mock, &e, "heancms").await;
    assert_eq!((r.engine, r.warning), (Engine::HeanCms, None));
    let m = read_manifest(dir.join(&r.id)).unwrap();
    assert_eq!(m.extra["api_base"], "https://api.h.test");
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!((titles(&pop), pop[0]["source_id"].as_str()), (vec!["Hero"], Some("series/hero")));
    assert_eq!(pop[0]["cover_url"], format!("https://api.h.test/covers/hero.jpg{}", mvref("https://h.test")));
    let ch = p.get_chapter_list("series/hero").await.unwrap();
    assert_eq!(ch.as_array().unwrap().len(), 1, "paywalled chapters are skipped");
    assert_eq!(ch[0]["source_id"], "series/hero/chapter-1");
    let pages = p.get_page_urls("series/hero/chapter-1").await.unwrap();
    let hint = mvref("https://h.test");
    assert_eq!(pages, json!([format!("https://img.test/1.jpg{hint}"), format!("https://api.h.test/pages/2.jpg{hint}")]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_keyoapp_extension() {
    let home = r#"<a href="/series/?genre=action">Action</a><img src="/assets/images/logo.png">
        <a href="https://k.test/series/foo-123/" title="Foo"><img src="https://k.test/c/foo.jpg"></a>
        <a href="https://k.test/series/bar-456/" title="Bar"><img src="https://k.test/c/bar.jpg"></a>"#;
    let details = r#"<h1>Foo</h1><p>A story.</p><div id="chapters"><a href="https://k.test/chapter/abc123/" title="Chapter 2" d="March 3, 2024">Chapter 2</a><a href="https://k.test/chapter/abc122/" title="Chapter 1" d="March 1, 2024">Chapter 1</a></div>"#;
    let reader = r#"<script>var realUrl = `https://cdn.k.test/uploads/${uid}`;</script><div id="pages"><img uid="a1.jpg"><img uid="a2.jpg"></div>"#;
    let mock = Mock::default()
        .route("=https://k.test/chapter/abc123/", 200, reader)
        .route("=https://k.test/series/foo-123/", 200, details)
        .route("=https://k.test/series/", 200, home)
        .route("=https://k.test/", 200, home);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.keyo", "Keyo", "en", "https://k.test");
    let (dir, r, mock) = install(mock, &e, "keyoapp").await;
    assert_eq!((r.engine, r.warning), (Engine::KeyoApp, None));
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!(titles(&pop), ["Foo", "Bar"]);
    assert_eq!(pop[0]["source_id"], "series/foo-123/");
    let ch = p.get_chapter_list("series/foo-123/").await.unwrap();
    assert_eq!((ch.as_array().unwrap().len(), ch[0]["source_id"].as_str()), (2, Some("chapter/abc122/")));
    let hint = mvref("https://k.test");
    assert_eq!(p.get_page_urls("chapter/abc123/").await.unwrap(), json!([format!("https://cdn.k.test/uploads/a1.jpg{hint}"), format!("https://cdn.k.test/uploads/a2.jpg{hint}")]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_unknown_theme_uses_the_generic_engine() {
    let card = |n: u32| format!(r#"<a href="https://g.test/manga/title-{n}/" title="Title {n}"><img src="https://g.test/c/{n}.jpg"></a>"#);
    let home = format!("<html><body>{}</body></html>", (1..=4).map(card).collect::<String>());
    let details = r#"<h1>Title 1</h1><a href="https://g.test/manga/title-1/chapter-2/">Chapter 2</a><a href="https://g.test/manga/title-1/chapter-1/">Chapter 1</a>"#;
    let reader = r#"<div class="reading-content"><img src="https://g.test/p/1.jpg"><img src="https://g.test/p/2.jpg"></div>"#;
    let mock = Mock::default()
        .route("=https://g.test/manga/title-1/chapter-1/", 200, reader)
        .route("=https://g.test/manga/title-1/", 200, details)
        .route("=https://g.test/", 200, &home);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.plain", "Plain", "en", "g.test/");
    let (dir, r, mock) = install(mock, &e, "generic").await;
    assert_eq!((r.engine, r.warning), (Engine::Generic, None));
    // A bare host in the index is normalised to https.
    assert_eq!(read_manifest(dir.join(&r.id)).unwrap().base_url, "https://g.test");
    let p = load(&dir, &r.id, &mock).await;
    let pop = p.get_popular_manga(1).await.unwrap();
    assert_eq!(titles(&pop), ["Title 1", "Title 2", "Title 3", "Title 4"]);
    let ch = p.get_chapter_list("manga/title-1/").await.unwrap();
    assert_eq!(ch[0]["source_id"], "manga/title-1/chapter-1/");
    assert_eq!(p.get_page_urls("manga/title-1/chapter-1/").await.unwrap().as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn unreachable_site_installs_with_a_warning_and_detects_lazily() {
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.foo", "Foo", "en", "https://foo.test");

    // Declared multisrc theme (build.gradle.kts of the extension) is the fallback when the site is walled.
    let hint = Mock::default().route(
        "keiyoushi/extensions-source/main/src/en/foo/build.gradle.kts",
        200,
        "plugins { id(\"lib-multisrc\") }\nthemePkg = 'madara'\nversionCode = 3",
    );
    let (dir, r, _) = install(hint, &e, "hint").await;
    assert_eq!(r.engine, Engine::Madara);
    let w = r.warning.expect("an unreachable site must be reported");
    assert!(w.contains("Foo installed"), "{w}");
    assert!(dir.join(&r.id).join("index.js").exists(), "the plugin stays installed");
    let _ = std::fs::remove_dir_all(&dir);

    // Nothing known: the engine is `auto` and the plugin detects it on first use.
    let (dir, r, _) = install(Mock::default(), &e, "auto").await;
    assert_eq!(r.engine, Engine::Auto);
    assert!(r.warning.is_some());
    assert_eq!(read_manifest(dir.join(&r.id)).unwrap().extra["engine"], "auto");
    let list = r#"<div class="page-item-detail"><div class="post-title"><h3><a href="https://foo.test/manga/x/">X</a></h3></div></div>"#;
    let up = Arc::new(Mock::default().route("=https://foo.test/", 200, list).route("foo.test/manga/?m_orderby=views", 200, list));
    let p = load(&dir, &r.id, &up).await;
    assert_eq!(p.call("__engine", vec![]).await.unwrap(), json!("madara"));
    assert_eq!(titles(&p.get_popular_manga(1).await.unwrap()), ["X"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn install_rejects_bad_entries_and_cannot_escape_the_plugins_dir() {
    let dir = tmpdir("install-bad");
    let backend: Arc<dyn FetchBackend> = Arc::new(Mock::default());
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.foo", "Foo", "en", "https://foo.test");

    let err = repo::install_repo_extension(&e, 3, &dir, backend.clone()).await.unwrap_err().to_string();
    assert!(err.contains("No source found"), "{err}");
    let mut nourl = e.clone();
    nourl.sources[0].base_url = "".into();
    let err = repo::install_repo_extension(&nourl, 0, &dir, backend.clone()).await.unwrap_err().to_string();
    assert!(err.contains("no website address"), "{err}");

    // pkg and lang come from an untrusted index: the plugin id is always a plain folder name.
    let mut evil = e.clone();
    evil.pkg = "../../evil/x".into();
    evil.sources[0].lang = "e/n\\..".into();
    let id = repo::repo_plugin_id(&evil, 0);
    assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{id}");
    let r = repo::install_repo_extension(&evil, 0, &dir, backend.clone()).await.unwrap();
    assert!(dir.join(&r.id).join("manifest.json").exists());
    assert!(!dir.parent().unwrap().join("evil").exists());

    // Two sources of one extension in the same language get distinct plugin ids.
    let mut two = e.clone();
    two.sources.push(repo::RepoSource { name: "Foo 2".into(), lang: "en".into(), id: json!("2"), base_url: "https://foo2.test".into() });
    two.sources.push(repo::RepoSource { name: "Foo FR".into(), lang: "fr".into(), id: json!("3"), base_url: "https://foo3.test".into() });
    let ids: Vec<String> = (0..3).map(|i| repo::repo_plugin_id(&two, i)).collect();
    assert_eq!(ids, ["repo-eu-kanade-tachiyomi-extension-en-foo-en", "repo-eu-kanade-tachiyomi-extension-en-foo-en-1", "repo-eu-kanade-tachiyomi-extension-en-foo-fr"]);

    // An empty source name falls back to the extension name (a nameless manifest would be invisible).
    let mut nameless = e.clone();
    nameless.sources[0].name = " ".into();
    let r = repo::install_repo_extension(&nameless, 0, &dir, backend).await.unwrap();
    assert_eq!(r.name, "Foo");
    assert!(repo::list_installed(&dir).iter().any(|m| m.name == "Foo"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_check_and_in_place_update() {
    let list = r#"<div class="page-item-detail"><div class="post-title"><h3><a href="https://m.test/manga/a/">A</a></h3></div></div>"#;
    let site = |mock: Mock| mock.route("m.test/manga/?m_orderby=views", 200, list).route("=https://m.test/", 200, list);
    let e1 = repo_ext("eu.kanade.tachiyomi.extension.en.up", "Up", "en", "https://m.test");
    let (dir, r, mock) = install(site(Mock::default()), &e1, "update").await;
    assert_eq!(r.engine, Engine::Madara);

    let mut e2 = e1.clone();
    e2.version = "1.4.2".into();
    e2.code = 2;
    let other = repo_ext("eu.kanade.tachiyomi.extension.en.other", "Other", "en", "https://o.test");
    let index = serde_json::to_string(&vec![other, e2.clone()]).unwrap();
    let repos = vec!["https://r.test/nope/index.min.json".to_string(), "https://r.test/repo/index.min.json".to_string()];
    let remote = Mock::default().route("r.test/repo/index.min.json", 200, &index);

    // An unreachable repo is skipped; the other one reports the update.
    let ups = repo::check_repo_updates(&dir, &repos, &remote).await;
    assert_eq!(ups.len(), 1);
    let u = &ups[0];
    assert_eq!((u.plugin_id.as_str(), u.name.as_str(), u.installed_version.as_str(), u.available_version.as_str(), u.source_index), (r.id.as_str(), "Up", "1.4.1", "1.4.2", 0));
    assert_eq!(u.extension, e2);

    // No repo plugins = no network use at all.
    let empty = tmpdir("update-none");
    assert!(repo::check_repo_updates(&empty, &repos, &Mock::default()).await.is_empty());
    assert!(!ups.is_empty() && remote.seen.lock().unwrap().len() >= 2);

    // Installing the update replaces the plugin in place (same id) and stops the report.
    let r2 = repo::install_repo_extension(&u.extension, u.source_index, &dir, Arc::new(Shared(mock.clone()))).await.unwrap();
    assert_eq!(r2.id, r.id);
    assert_eq!(read_manifest(dir.join(&r.id)).unwrap().version, "1.4.2");
    assert_eq!(repo::read_repo_source(&dir, &r.id).unwrap().code, 2);
    assert_eq!(repo::list_installed(&dir).len(), 1);
    assert!(repo::check_repo_updates(&dir, &repos, &remote).await.is_empty());

    // Without version codes a different version string counts as an update.
    let (mut a, mut b) = (e1.clone(), e1.clone());
    (a.code, b.code, b.version) = (0, 0, "9".into());
    let idx = serde_json::to_string(&vec![b]).unwrap();
    std::fs::write(dir.join(&r.id).join(".repo_source"), serde_json::to_string(&a).unwrap()).unwrap();
    let ups = repo::check_repo_updates(&dir, &repos[1..], &Mock::default().route("index.min.json", 200, &idx)).await;
    assert_eq!(ups.len(), 1);
    let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&empty));
}

#[tokio::test]
async fn old_repo_plugins_are_regenerated_offline() {
    let dir = tmpdir("upgrade");
    let write = |id: &str, manifest: Value, code: &str| {
        let d = dir.join(id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("manifest.json"), manifest.to_string()).unwrap();
        std::fs::write(d.join("index.js"), code).unwrap();
        d
    };
    // Electron-era plugin from the pre-template generator.
    let legacy = write(
        "repo-a-en",
        json!({"id": "repo-a-en", "name": "A", "version": "1", "base_url": "https://a.test", "lang": "en", "entry_point": "index.js"}),
        "// Auto-generated MangaVox plugin\nregisterSource({});",
    );
    // Template plugin of an older version keeps its detected engine.
    let v1 = write(
        "repo-b-en",
        json!({"id": "repo-b-en", "name": "B", "base_url": "https://b.test", "lang": "en", "engine": "madara", "repo_pkg": "x.extension.en.b"}),
        "// MangaVox template plugin v1 (engine: madara)\nregisterSource({});",
    );
    // Current plugins and non-repo plugins are left alone.
    let current = write("repo-c-en", json!({"id": "repo-c-en", "name": "C", "base_url": "https://c.test"}), &format!("{} v{} (engine: auto)\n", mv_plugins::templates::TEMPLATE_HEADER, mv_plugins::templates::TEMPLATE_VERSION));
    let plain = write("webtoons", json!({"id": "webtoons", "name": "W"}), "// Auto-generated MangaVox plugin\n");

    assert_eq!(repo::upgrade_repo_plugins(&dir), 2);
    assert_eq!(repo::upgrade_repo_plugins(&dir), 0, "idempotent");

    let code = std::fs::read_to_string(legacy.join("index.js")).unwrap();
    assert!(code.starts_with("// MangaVox template plugin v2 (engine: auto)"));
    assert!(std::fs::read_to_string(legacy.join("index.js.bak")).unwrap().contains("Auto-generated"));
    let m = read_manifest(&legacy).unwrap();
    assert_eq!((m.extra["engine"].as_str(), m.extra["id_style"].as_str()), (Some("auto"), Some("legacy")));
    assert!(code.contains("\"legacyIds\":true"));

    let code = std::fs::read_to_string(v1.join("index.js")).unwrap();
    assert!(code.starts_with("// MangaVox template plugin v2 (engine: madara)"));
    let m = read_manifest(&v1).unwrap();
    assert_eq!((m.extra["id_style"].as_str(), m.extra["repo_pkg"].as_str(), m.extra["generator"].as_str()), (Some("path"), Some("x.extension.en.b"), Some("mangavox-template")));

    assert!(!current.join("index.js.bak").exists() && !plain.join("index.js.bak").exists());
    assert!(std::fs::read_to_string(plain.join("index.js")).unwrap().contains("Auto-generated"));
    // The upgraded plugins load in the sandbox.
    for d in [&legacy, &v1] {
        let p = load_plugin_with(d, PluginOptions::default(), Arc::new(Mock::default())).await.unwrap();
        assert!(p.has_method("getPopularManga"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "live network"]
async fn live_keiyoushi_index() {
    // index.min.json is a "update your app" stub, the real list is the modern index.json next to it.
    let exts = repo::fetch_repo_extensions("https://github.com/keiyoushi/extensions", &ReqwestBackend::new()).await.unwrap();
    let mut ids = std::collections::HashSet::new();
    let mut sources = 0;
    for e in &exts {
        for i in 0..e.sources.len() {
            sources += 1;
            assert!(ids.insert(repo::repo_plugin_id(e, i)), "duplicate plugin id for {} source {i}", e.pkg);
            assert!(mv_plugins::templates::normalize_base_url(&e.sources[i].base_url).len() > "https://".len(), "{} source {i}", e.pkg);
        }
    }
    println!("{} extensions, {sources} sources, e.g. {} -> {}", exts.len(), exts[0].name, repo::extension_icon_url("https://github.com/keiyoushi/extensions", &exts[0]));
    assert!(exts.len() > 300 && sources >= exts.len());
}

#[tokio::test]
async fn generated_plugins_cope_with_large_noisy_pages() {
    // Inline scripts full of markup, styles, comments and a huge data: image must not trip the HTML helpers.
    let noise = format!(
        "<!-- {} --><script>var tpl = \"{}\"; var a = '<a href=\"x\">';</script><style>{}</style><img src=\"data:image/png;base64,{}\">",
        "c ".repeat(50_000),
        "<div class='x'>&amp;</div>".repeat(20_000),
        ".a>b{color:red}".repeat(20_000),
        "QUJD".repeat(150_000),
    );
    let cards: String = (1..=200)
        .map(|n| format!(r#"<div class="page-item-detail"><div class="post-title"><h3><a href="https://n.test/manga/t-{n}/">Title {n}</a></h3></div><img data-src="https://n.test/c/{n}.jpg"></div>"#))
        .collect();
    let page = format!("<html><body>{noise}{cards}</body></html>");
    assert!(page.len() > 1_500_000);
    let mock = Mock::default().route("n.test/manga/?m_orderby=views", 200, &page).route("=https://n.test/", 200, &page);
    let e = repo_ext("eu.kanade.tachiyomi.extension.en.noisy", "Noisy", "en", "https://n.test");
    let t = std::time::Instant::now();
    let (dir, r, mock) = install(mock, &e, "noisy").await;
    assert_eq!((r.engine, r.warning), (Engine::Madara, None));
    let pop = load(&dir, &r.id, &mock).await.get_popular_manga(1).await.unwrap();
    assert_eq!(pop.as_array().unwrap().len(), 200);
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Network: the link newer apps take (Keiyoushi's protobuf `index.pb`) lists its extensions here
/// too. `cargo test -p mv-plugins keiyoushi -- --ignored`
#[tokio::test]
#[ignore]
async fn keiyoushi_index_pb_link_lists_extensions() {
    let backend = ReqwestBackend::new();
    let list = repo::fetch_repo_extensions("https://github.com/keiyoushi/extensions/raw/repo/index.pb", &backend).await.expect("fetch");
    println!("{} extensions", list.len());
    assert!(list.len() > 100, "only {} extensions", list.len());
}
