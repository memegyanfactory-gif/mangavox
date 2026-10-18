//! Live tests (need WebView2 + network): `cargo test -p mv-webwall -- --ignored --nocapture`.

use std::sync::{Arc, OnceLock};

use mv_sources::{comick, hentai2read, WallSolver};
use mv_webwall::{WallConfig, WebWall};

/// One shared instance per process (like the app): WebView2 does not like a second environment
/// on the same profile right after the first one shut down.
fn start() -> Arc<WebWall> {
    static W: OnceLock<Arc<WebWall>> = OnceLock::new();
    W.get_or_init(|| {
        let dir = std::env::temp_dir().join("mv-webwall-test-profile");
        WebWall::start(WallConfig { profile_dir: dir, interactive: false, ..Default::default() }).expect("start")
    })
    .clone()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn hentai2read_html_parses() {
    let w = start();
    println!("UA: {:?}", w.user_agent());
    let html = w.fetch_html("https://hentai2read.com/browse/most-viewed", None).await.expect("fetch_html");
    println!("html len {}", html.len());
    let props = hentai2read::parse_props(&html).expect("parse_props");
    let books = hentai2read::map_books(&props);
    println!("books: {}", books.len());
    assert!(!books.is_empty());
    let cookies = w.cookies("https://hentai2read.com/browse/most-viewed").await.expect("cookies");
    println!("cookies: {:?}", cookies.iter().map(|c| c.0.as_str()).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn comick_api_via_page_fetch() {
    let w = start();
    let body = w
        .fetch_text("https://api.comick.dev/v1.0/search?q=one%20piece&limit=5", &[("Accept".into(), "application/json".into())])
        .await
        .expect("fetch_text");
    println!("body head: {}", &body[..body.len().min(200)]);
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    let list = comick::parse_comic_list(&json);
    println!("results: {}", list.len());
    assert!(!list.is_empty());
    let v = w.eval_json("https://comick.live/", "document.location.origin").await.expect("eval_json");
    println!("origin: {v}");
}
