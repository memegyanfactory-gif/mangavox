//! Checks the pieces a signed-in site is driven with: a window the user sees, a script that runs
//! for longer than the usual action timeout, and a large answer coming back. Uses a throwaway
//! browser profile and a public page.
//!
//!     cargo run -p mv-webwall --example show_eval
use std::time::{Duration, Instant};

use mv_webwall::{WallConfig, WebWall};

#[tokio::main]
async fn main() {
    let profile = std::env::temp_dir().join(format!("mangavox-webwall-example-{}", std::process::id()));
    let wall = WebWall::start(WallConfig { profile_dir: profile.clone(), ..Default::default() }).expect("WebView2");
    let url = "https://example.com/";
    wall.show(url, "MangaVox · example window").await.expect("show");
    println!("window shown");
    // 40 s of waiting (the usual limit is 30 s), then about 1.5 MB back.
    let t = Instant::now();
    let script = "(async () => { await new Promise(r => setTimeout(r, 40000)); return { title: document.title, visible: document.visibilityState, data: 'x'.repeat(1500000) }; })()";
    let v = wall.eval_json_for(url, script, Duration::from_secs(90)).await.expect("long eval");
    println!("long eval: {} s, title {:?}, page {:?}, {} bytes back", t.elapsed().as_secs(), v["title"].as_str(), v["visible"].as_str(), v["data"].as_str().map(str::len).unwrap_or(0));
    // A second script runs in the same window without reloading it.
    let again = wall.eval_json_for(url, "(async () => (window.__mv_seen = (window.__mv_seen || 0) + 1))()", Duration::from_secs(20)).await.expect("eval");
    let third = wall.eval_json_for(url, "(async () => (window.__mv_seen = (window.__mv_seen || 0) + 1))()", Duration::from_secs(20)).await.expect("eval");
    println!("same page kept between scripts: {again} then {third}");
    drop(wall);
    std::thread::sleep(Duration::from_millis(800));
    let _ = std::fs::remove_dir_all(profile);
}
