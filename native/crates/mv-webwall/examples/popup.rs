//! Checks that a window the user opened may open a popup (a site's "Continue with Google").
//!
//!     cargo run -p mv-webwall --example popup
use std::time::Duration;

use mv_webwall::{WallConfig, WebWall};

#[tokio::main]
async fn main() {
    let profile = std::env::temp_dir().join(format!("mangavox-webwall-popup-{}", std::process::id()));
    let wall = WebWall::start(WallConfig { profile_dir: profile.clone(), ..Default::default() }).expect("WebView2");
    let url = "https://example.com/";
    wall.show(url, "MangaVox · popup check").await.expect("show");
    let opened = wall.eval_json_for(url, "(async () => { const w = window.open('https://example.org/', 'mvpop', 'width=500,height=600'); await new Promise(r => setTimeout(r, 2500)); const ok = !!w && !w.closed; if (w) w.close(); return ok; })()", Duration::from_secs(30)).await.expect("eval");
    println!("popup from the user's window opened: {opened}");
    drop(wall);
    std::thread::sleep(Duration::from_millis(800));
    let _ = std::fs::remove_dir_all(profile);
}
