//! A headless PC end for testing cloud access against a relay (`cloud/`, `npm run dev`): connects,
//! prints a pairing link (a new one for every line on stdin) and allows every phone that asks.
//! Use a throwaway data folder, never your real one:
//!
//!     MANGAVOX_HOME=<temp dir> MANGAVOX_CLOUD_URL=http://localhost:8787/mangavox \
//!         cargo run -p ui-phone --example cloud_peer
use std::io::{BufRead as _, Write as _};
use std::time::Duration;

use ui_phone::{auth, cloud, state};

fn link() {
    let code = auth::new_cloud_code();
    match cloud::pair_link(&code, Some("http://192.168.1.5:8790/pair?code=test")) {
        Some(l) => println!("PAIR {l}"),
        None => println!("ERROR no cloud identity"),
    }
    let _ = std::io::stdout().flush();
}

fn main() {
    if std::env::var_os("MANGAVOX_HOME").is_none() {
        eprintln!("Set MANGAVOX_HOME to a throwaway folder first.");
        std::process::exit(2);
    }
    cloud::start();
    link();
    std::thread::spawn(|| {
        for _ in std::io::stdin().lock().lines() {
            link();
        }
    });
    let mut was = None;
    loop {
        for p in cloud::pending() {
            println!("ALLOW {}", p.name);
            cloud::answer(p.id, true);
        }
        let st = state::cloud_status();
        let now = (st.connected, st.error.clone());
        if was.as_ref() != Some(&now) {
            println!("STATUS connected={} error={:?}", now.0, now.1);
            was = Some(now);
        }
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_millis(200));
    }
}
