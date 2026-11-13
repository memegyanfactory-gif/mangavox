//! Pictures made on Magnific's website, in the app's own browser window where the user has
//! signed in (its unlimited offers cover the website, not the API). The app does what the user
//! would do by hand, one picture at a time: Google Nano Banana 2.1, one image, 1K, unlimited mode
//! on; the references dropped in, the prompt typed, Generate pressed, the picture waited for and
//! saved. `magnific_web.js` is that routine; it presses Generate only when the page itself shows
//! the picture as "Unlimited", and never touches a check meant for a person (the window is
//! brought up for the user instead).
//!
//! The user chose this knowing Magnific's terms restrict automated use of the site.
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use ui_studio::images::Reference;

/// The generator page (the app, not the landing page).
pub(crate) const GENERATOR: &str = "https://www.magnific.com/app/ai-image-generator";

/// The frames the page offers.
pub(crate) const ASPECTS: &[&str] = &["2:3", "3:4", "4:5", "1:1", "5:4", "4:3", "3:2", "16:9", "9:16", "21:9"];

/// Reference pictures sent with one picture (the page takes 14; fewer keeps each request light).
pub(crate) const MAX_REFS: usize = 8;

/// The longest a picture may take once Generate is pressed.
const WAIT: Duration = Duration::from_secs(300);

/// One picture at a time, whoever asks.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn browser() -> Result<std::sync::Arc<mv_webwall::WebWall>> {
    mv_webwall::global().ok_or_else(|| anyhow!("Magnific's website needs the app's built-in browser (WebView2), which is not available on this PC."))
}

/// Opens the window the user signs in to Magnific in. It keeps the sign-in between runs.
pub(crate) async fn open_window() -> Result<()> {
    browser()?.show(GENERATOR, "Magnific · sign in here (MangaVox)").await.map_err(|e| anyhow!("Could not open the Magnific window: {e}"))
}

/// The routine with what this picture needs filled in.
fn script(prompt: &str, aspect: &str, refs: &[(String, String)]) -> String {
    let params = json!({
        "prompt": prompt,
        "aspect": if ASPECTS.contains(&aspect) { aspect } else { "16:9" },
        "refs": refs.iter().map(|(name, b64)| json!({ "name": name, "b64": b64 })).collect::<Vec<_>>(),
        "wait_ms": WAIT.as_millis() as u64,
    });
    include_str!("magnific_web.js").replace("__PARAMS__", &params.to_string())
}

/// What the routine answered: the picture as a data URL, or why there is none (and whether the
/// user has something to do in the window).
fn outcome(v: &Value) -> Result<&str> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return v.get("data").and_then(Value::as_str).ok_or_else(|| anyhow!("Magnific's website returned no picture."));
    }
    let why = v.get("why").and_then(Value::as_str).unwrap_or("it did not say why");
    match v.get("need").and_then(Value::as_str).unwrap_or("") {
        "signin" => bail!("Sign in to Magnific's website in the window that just opened, then paint again."),
        "human" => bail!("Magnific's website wants to confirm you are a person. Do that in the window that just opened, then paint again."),
        "paid" => bail!("Magnific's website: {why}"),
        _ => bail!("Magnific's website could not make the picture: {why}"),
    }
}

/// Makes one picture on Magnific's website and saves it as PNG at `out`.
pub(crate) async fn generate(prompt: &str, refs: &[Reference], aspect: &str, out: &Path, on_status: impl Fn(&str)) -> Result<()> {
    let wall = browser()?;
    on_status("Waiting for the picture before it");
    let _turn = TURN.lock().await;
    // The first picture of a run opens the window (a page nobody sees runs slowly, and a sign-in
    // or a check for a person has to be seen). Closed by the user later, the work goes on hidden.
    static OPENED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !OPENED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        on_status("Opening the Magnific window");
        let _ = open_window().await;
    }
    on_status("Preparing the references");
    let files: Vec<_> = refs.iter().take(MAX_REFS).map(|r| r.file.clone()).collect();
    let small = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String)>> { files.iter().enumerate().map(|(i, f)| Ok((format!("mangavox-ref-{}.jpg", i + 1), mv_create::small_jpeg_b64(f, 1280)?))).collect() }).await??;
    on_status("On Magnific's website: typing the prompt and pressing Generate");
    let js = script(prompt, aspect, &small);
    let answer = wall.eval_json_for(GENERATOR, &js, WAIT + Duration::from_secs(150)).await.map_err(|e| anyhow!("Magnific's website did not answer: {e}"))?;
    let data = match outcome(&answer) {
        Ok(d) => d.to_string(),
        Err(e) => {
            // Signing in and proving to be a person are the user's to do: bring the window up.
            if matches!(answer.get("need").and_then(Value::as_str), Some("signin" | "human")) {
                let _ = open_window().await;
            }
            return Err(e);
        }
    };
    on_status("Saving the picture");
    let target = out.to_path_buf();
    tokio::task::spawn_blocking(move || mv_create::save_data_url(&data, &target)).await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routine_gets_this_pictures_needs_and_guards_the_free_offer() {
        let js = script("A \"quoted\" prompt\nwith a second line", "2:3", &[("mangavox-ref-1.jpg".into(), "QUJD".into())]);
        assert!(!js.contains("__PARAMS__"));
        assert!(js.contains(r#""aspect":"2:3""#) && js.contains(r#"A \"quoted\" prompt\nwith a second line"#) && js.contains(r#""b64":"QUJD""#), "the needs are filled in as JSON");
        // A frame the page does not offer falls back to one it does.
        assert!(script("x", "7:5", &[]).contains(r#""aspect":"16:9""#));
        // Generate is pressed only after the page shows Unlimited, at 1K, one image, unlimited mode on.
        let (guard, press) = (js.find(r#"return fail("paid""#).unwrap(), js.find("go.click()").unwrap());
        assert!(guard < press, "the guard comes before the click");
        assert!(js.contains(r#"!== "1K""#) && js.contains(r#"!== "ON""#) && js.contains("/unlimited/i.test(label)"));
        // A check for a person is reported, never worked around.
        assert!(js.contains(r#"fail("human""#) && !js.to_lowercase().contains("solve"));
    }

    #[test]
    fn answers_become_pictures_or_plain_reasons() {
        assert_eq!(outcome(&json!({ "ok": true, "data": "data:image/png;base64,AAAA" })).unwrap(), "data:image/png;base64,AAAA");
        let why = |v: Value| outcome(&v).unwrap_err().to_string();
        assert!(why(json!({ "ok": false, "need": "signin", "why": "x" })).starts_with("Sign in to Magnific's website"));
        assert!(why(json!({ "ok": false, "need": "human", "why": "x" })).contains("confirm you are a person"));
        assert!(why(json!({ "ok": false, "need": "paid", "why": "Magnific does not show this picture as Unlimited (size 2K)" })).contains("not show this picture as Unlimited"));
        assert!(why(json!({ "ok": false, "need": "site", "why": "Magnific said: error" })).contains("could not make the picture"));
        assert!(why(json!({ "ok": true })).contains("no picture"));
    }
}
