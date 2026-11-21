//! What the generator page offers: its models with their free marks, and a read-only check that
//! the controls the picture routine uses are still there (Magnific renames them now and then).

use std::time::Duration;

use serde_json::{json, Value};

use crate::driver::{ensure_window, failure, hand_over, run, Fail, STILL_LOADING, TURN};
use crate::{log, SelfTest, SiteModel, WebError, DEFAULT_MODEL_CY, GENERATOR};

/// What the model menu shows when its list could not be read (the reason is logged).
pub(crate) const LIST_UNREADABLE: &str = "Magnific's model list could not be read just now.";

/// Controls a picture cannot be made without: exactly the ones the first, proven routine relied on
/// every time (it ran for months on these). Only a signed-in page is judged against them.
pub const REQUIRED: &[&str] = &[
    "image-generator-form",
    "tti-mode-selector-v3-trigger",
    "number-images-value",
    "image-references-input",
    "reference-add-button",
    "image-prompt-input",
    "image-generate-button",
];

/// Controls the routine uses when they are there and does without when not: the entries inside
/// the model and size menus (only needed when the page is not on them already), the count
/// buttons (one picture), the aspect menu, and the Style and Character slots (their names are
/// guesses; a reference then goes into the Add slot). Some only show with references or a prompt
/// in, so the read-only check cannot always see them. Never reported missing.
pub const OPTIONAL: &[&str] = &[
    // The unlimited switch only shows once a model that has it is picked (the page opens on "Auto"),
    // so the read-only check cannot require it. The picture routine still refuses to press Generate
    // unless this switch reads ON and the button shows the free label.
    "unlimited-mode-toggle-button",
    // The size menu depends on the model selected too (seen live: none with Seedream 4). The
    // routine sets a size only when the menu is there, and the free guard then wants it still
    // absent.
    "image-output-axes-trigger",
    "tti-model-selector-popover-model-imagen-nano-banana-2-1",
    "axis-option-resolution-1k",
    "decrease-number-images-button",
    "increase-number-images-button",
    "increment-number-images-button",
    "image-aspect-ratio-input",
    "image-generator-aspect-ratio-16:9",
    "smart-prompt-toggle",
    "reference-card-remove-button",
    "clear-prompt-button",
    "image-style-add-button",
    "style-reference-add-button",
    "image-style-reference-add-button",
    "style-add-button",
    "image-character-add-button",
    "character-reference-add-button",
    "image-character-reference-add-button",
    "character-add-button",
];

/// Words on the Generate button that say the picture would be free.
fn free_words(label: &str) -> bool {
    let l = label.to_lowercase();
    l.contains("unlimited") || l.contains("relax") || l.contains("credit-free")
}

/// The size a picture is made at from a row's size chips ("2K-4K", "4K"): 1K when offered, else
/// the smallest.
pub(crate) fn smallest_size(chips: &[String]) -> Option<String> {
    let mut all: Vec<f32> = chips
        .iter()
        .flat_map(|c| c.split(['-', '–']).map(str::to_string).collect::<Vec<_>>())
        .filter_map(|p| p.trim().trim_end_matches(['K', 'k']).parse::<f32>().ok())
        .collect();
    all.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pick = if all.contains(&1.0) { Some(1.0) } else { all.first().copied() };
    pick.map(|n| format!("{n}K"))
}

fn as_error(f: Fail) -> WebError {
    match f {
        Fail::Web(e) => e,
        Fail::Navigated => WebError::Site("The Magnific window is on another page. Press Continue and try again.".into()),
    }
}

/// The model list from the routine's answer.
pub(crate) fn parse_models(v: &Value) -> Result<Vec<SiteModel>, WebError> {
    if let Some(f) = failure(v) {
        // The model menu's own words: never "could not make the picture".
        return Err(match as_error(f) {
            WebError::Site(m) if m == STILL_LOADING => WebError::Site(m),
            WebError::Site(m) => {
                log(&format!("magnific model list: {m}"));
                WebError::Site(LIST_UNREADABLE.into())
            }
            other => other,
        });
    }
    let s = |m: &Value, k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let models: Vec<SiteModel> = v
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let cy = s(m, "cy");
            if cy.is_empty() {
                return None;
            }
            let raw = s(m, "raw");
            let name = Some(s(m, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| raw.clone());
            let sizes: Vec<String> = m.get("sizes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let default = cy == DEFAULT_MODEL_CY;
            // The default model's row shows its price until its Unlimited switch is on (and the
            // switch only shows once it is picked): its free offer at 1K is known.
            let free = default || (m.get("free").and_then(Value::as_bool).unwrap_or(false) && !m.get("priced").and_then(Value::as_bool).unwrap_or(false));
            let note = if default { "Free at 1K with Unlimited".to_string() } else { smallest_size(&sizes).map(|s| format!("Free at {s}")).unwrap_or_else(|| "Free".into()) };
            Some(SiteModel { cy, name, raw, free, group: s(m, "group"), note })
        })
        .collect();
    if models.is_empty() {
        return Err(WebError::Site("Magnific's model list was empty.".into()));
    }
    Ok(models)
}

/// The self-test from the routine's answer.
pub(crate) fn parse_self_test(v: &Value) -> Result<SelfTest, WebError> {
    if let Some(f) = failure(v) {
        return Err(as_error(f));
    }
    let seen: Vec<&str> = v.get("found").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let has = |n: &str| seen.iter().any(|s| *s == n);
    let label = v.get("label").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let signed_in = v.get("signed_in").and_then(Value::as_bool).unwrap_or(false);
    Ok(SelfTest {
        signed_in,
        found: REQUIRED.iter().chain(OPTIONAL).copied().filter(|n| has(n)).collect(),
        // A signed-out page lacks most controls: that is the sign-in, not a changed site.
        missing: if signed_in { REQUIRED.iter().copied().filter(|n| !has(n)).collect() } else { Vec::new() },
        free_label: Some(label).filter(|l| free_words(l)),
    })
}

pub(crate) fn models_script() -> String {
    include_str!("magnific_models.js").replace("__PARAMS__", &json!({ "generator": GENERATOR }).to_string())
}

pub(crate) fn self_test_script() -> String {
    let names: Vec<&str> = REQUIRED.iter().chain(OPTIONAL).copied().collect();
    let params = json!({ "generator": GENERATOR, "names": names, "required": REQUIRED, "model_cy": DEFAULT_MODEL_CY });
    include_str!("magnific_selftest.js").replace("__PARAMS__", &params.to_string())
}

/// Both read in the one Magnific window (opened behind the app the first time; a window the
/// user closed stays closed until they press Continue).
pub(crate) async fn models() -> Result<Vec<SiteModel>, WebError> {
    crate::driver::browser()?;
    let _turn = TURN.lock().await;
    let quiet = |_: &str| {};
    let r = match ensure_window(&quiet).await {
        Ok(()) => match run(&models_script(), Duration::from_secs(90)).await {
            Ok(v) => parse_models(&v),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    // A read-only look never pops the window up for a sign-in: the panel asks the user first.
    if let Err(e @ WebError::Human) = &r {
        hand_over(e).await;
    }
    r
}

pub(crate) async fn self_test() -> Result<SelfTest, WebError> {
    crate::driver::browser()?;
    let _turn = TURN.lock().await;
    let quiet = |_: &str| {};
    let r = match ensure_window(&quiet).await {
        Ok(()) => match run(&self_test_script(), Duration::from_secs(120)).await {
            Ok(v) => parse_self_test(&v),
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    if let Err(e) = &r {
        hand_over(e).await;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_come_back_with_their_free_marks() {
        let v = json!({ "ok": true, "models": [
            { "cy": "imagen-nano-banana-2-1", "name": "Google Nano Banana 2.1", "raw": "Google Nano Banana 2.1 Refs 25 - 140", "group": "Google", "free": false, "priced": true },
            { "cy": "seedream-5", "name": "", "raw": "Seedream 5.0 65 credits", "free": false },
            { "cy": "seedream-4-5", "name": "Seedream 4.5", "raw": "Seedream 4.5 Refs ~39s 2K-4K ∞ Free gens", "group": "Seedream", "free": true, "priced": false, "sizes": ["2K-4K"] },
            { "cy": "seedream-5-5", "name": "Seedream 5.5", "raw": "Seedream 5.5 ∞ 25 - 140", "free": true, "priced": true },
            { "cy": "", "name": "junk" }
        ]});
        let m = parse_models(&v).unwrap();
        assert_eq!(m.len(), 4);
        // The default model is free at 1K with its Unlimited switch, whatever its row showed
        // before the switch was there (the live "Credits" tag on Nano Banana 2.1).
        assert!(m[0].free && m[0].name == "Google Nano Banana 2.1" && m[0].note.contains("1K") && m[0].group == "Google");
        assert!(!m[1].free && m[1].name == "Seedream 5.0 65 credits", "no name: the raw words");
        assert!(m[2].free && m[2].note == "Free at 2K" && m[2].group == "Seedream", "free at its smallest size");
        assert!(!m[3].free, "a free mark next to a price is not free");
        assert_eq!(smallest_size(&["2K-4K".into()]).as_deref(), Some("2K"));
        assert_eq!(smallest_size(&["1K".into(), "4K".into()]).as_deref(), Some("1K"));
        assert_eq!(smallest_size(&[]), None);
        assert_eq!(parse_models(&json!({ "ok": false, "need": "signin", "why": "x" })), Err(WebError::SignIn));
        assert!(parse_models(&json!({ "ok": true, "models": [] })).is_err());
        // The model menu says so in its own words, never "could not make the picture".
        assert_eq!(parse_models(&json!({ "ok": false, "need": "site", "why": "The model list did not open." })), Err(WebError::Site(LIST_UNREADABLE.into())));
        assert_eq!(parse_models(&json!({ "ok": false, "need": "site", "why": "loading" })), Err(WebError::Site(STILL_LOADING.into())));
        let js = models_script();
        assert!(js.contains(r#"if (!trigger) return fail("site", "loading");"#) && !js.contains("The model menu was not found"));
        assert!(js.find("(q(\"tti-mode-selector-v3-trigger\") && q(\"image-generate-button\")), 20000").unwrap() < js.find("if (signedOut(/^(log ?in|sign ?in)$/i)) return fail(\"signin\"").unwrap(), "the page is waited for first");
    }

    #[test]
    fn the_model_routine_reads_free_marks_from_the_page() {
        let js = models_script();
        assert!(!js.contains("__PARAMS__") && js.contains(GENERATOR));
        assert!(js.contains(r#"[data-cy^="' + PREFIX + '"]"#) && js.contains(r#""tti-model-selector-popover-model-""#));
        // The free marking against a price: a coin icon or a number that is not a chip.
        assert!(js.contains(r"/∞|free gens?|\bunlimited\b|credit-free/i.test(raw) || !!e.querySelector(INFINITY)"));
        assert!(js.contains(r"const priced = /\d/.test(rest) || /\bcredits?\b(?!-free)/i.test(raw) || !!e.querySelector(COIN);"));
        assert!(js.contains("free: mark && !priced") && js.contains(r"/\b(?:refs?|new)\b/gi") && js.contains(r"/\d+\s*%\s*off\b/gi"));
        // Every closed provider group is opened (never a model row or a buy link), its rows read.
        assert!(js.contains(r#"'[aria-expanded="false"]'"#) && js.contains("!/upgrade|buy|purchase|pricing|subscribe/i.test(text(h))") && js.contains("h.click();"));
        // 1K and unlimited mode are set before the list is read; the menu is closed after.
        let (one_k, on, open) = (js.find(r#"!== "1K""#).unwrap(), js.find(r#"!== "ON""#).unwrap(), js.find("trigger.click()").unwrap());
        assert!(one_k < open && on < open);
        assert!(js.rfind("await escape()").unwrap() > open);
        // Generate is only looked for (the page has finished building), never pressed.
        assert_eq!(js.matches("image-generate-button").count(), 1);
        assert!(js.contains(r#"&& q("image-generate-button")), 20000"#));
        assert!(!js.contains("go.click") && !js.contains("DragEvent") && !js.contains("insertText"), "nothing is generated, uploaded or typed");
    }

    #[test]
    fn the_self_test_only_looks() {
        let js = self_test_script();
        assert!(!js.contains("__PARAMS__"));
        assert!(!js.contains("go.click()") && !js.contains("DragEvent") && !js.contains("insertText") && !js.contains("toggle-button\").click"));
        for n in REQUIRED {
            assert!(js.contains(&format!("\"{n}\"")), "{n} is looked for");
        }
        assert!(js.contains("image-style-add-button") && js.contains("image-character-add-button"), "the slot guesses are listed");
        let v = json!({ "ok": true, "signed_in": true, "found": ["image-generator-form", "image-generate-button", "image-style-add-button", "made-up"], "label": "Generate Unlimited" });
        let t = parse_self_test(&v).unwrap();
        assert!(t.signed_in);
        assert_eq!(t.found, vec!["image-generator-form", "image-generate-button", "image-style-add-button"]);
        assert_eq!(t.missing.len(), REQUIRED.len() - 2);
        assert!(!t.missing.contains(&"image-style-add-button"), "an optional slot is never reported missing");
        assert_eq!(t.free_label.as_deref(), Some("Generate Unlimited"));
        let paid = parse_self_test(&json!({ "ok": true, "signed_in": true, "found": [], "label": "Generate 65 credits" })).unwrap();
        assert_eq!(paid.free_label, None);
        assert_eq!(parse_self_test(&json!({ "ok": false, "need": "human", "why": "x" })), Err(WebError::Human));
    }

    #[test]
    fn a_signed_out_page_is_never_reported_as_changed() {
        // Signed out, the page lacks nearly every control: no missing list, only the sign-in.
        let out = parse_self_test(&json!({ "ok": true, "signed_in": false, "found": [], "label": "" })).unwrap();
        assert!(!out.signed_in);
        assert!(out.missing.is_empty(), "signed out says nothing about the site changing");
        let out = parse_self_test(&json!({ "ok": true, "signed_in": false, "found": ["image-generator-form"] })).unwrap();
        assert!(out.missing.is_empty());
        // Signed in with a control gone: that one is missing.
        let all: Vec<&str> = REQUIRED.iter().copied().filter(|n| *n != "reference-add-button").collect();
        let t = parse_self_test(&json!({ "ok": true, "signed_in": true, "found": all, "label": "Generate" })).unwrap();
        assert_eq!(t.missing, vec!["reference-add-button"]);
    }

    #[test]
    fn the_self_test_checks_the_sign_in_before_any_control() {
        let js = self_test_script();
        let at = |s: &str| js.find(s).unwrap_or_else(|| panic!("{s} is in the routine"));
        // A check for a person first, then the site's sign-in page, before the window is moved.
        assert!(at(r#"if (human()) return fail("human""#) < at("if (signInPage()) return SIGNED_OUT"));
        assert!(at("if (signInPage()) return SIGNED_OUT") < at("location.assign(P.generator)"));
        // The signed-out generator answers before any control is looked for or any menu opened.
        let signed_out = at("if (signedOut(/^(log ?in|sign ?in)$/i)) return SIGNED_OUT");
        assert!(signed_out < at("const look = () =>") && signed_out < at("look();") && signed_out < at("t.click()"));
        assert!(at("if (!form) return signedOut(") < at("look();"), "no form: signed out or not loaded, never 'changed'");
        assert!(js.contains(r#"const SIGNED_OUT = { ok: true, signed_in: false, found: [], label: "" }"#));
        // Plain reasons only: no page paths or control names in what the user may read.
        assert!(!js.contains("\"(\" + location.pathname"));
    }

    #[test]
    fn a_slow_page_reads_as_loading_and_the_sign_in_marks_need_no_language() {
        for js in [self_test_script(), models_script(), crate::driver::script(&crate::Job { model_name: "m".into(), model_cy: "c".into(), prompt: "p".into(), refs: vec![], aspect: "16:9".into(), res: "1K".into(), count: 1, wait_s: 0 }, &[])] {
            for cy in ["header-login-button", "header-signup-button", "login-button-modal", "signup-button-modal"] {
                assert!(js.contains(&format!("\"{cy}\"")), "{cy} marks signed out");
            }
        }
        let js = self_test_script();
        assert!(js.contains(r#""required":["image-generator-form""#), "the routine knows the proven controls");
        assert!(js.contains(r#"return fail("site", "loading")"#));
        // Hydration is waited for before the sign-in and the controls are judged.
        assert!(js.find("(q(\"tti-mode-selector-v3-trigger\") && q(\"image-generate-button\")), 20000").unwrap() < js.find("if (signedOut(/^(log ?in|sign ?in)$/i)) return SIGNED_OUT").unwrap());
        match parse_self_test(&json!({ "ok": false, "need": "site", "why": "loading" })) {
            Err(WebError::Site(m)) => assert!(m.contains("did not finish loading") && m.contains("Try again") && !m.contains("changed")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_the_proven_controls_are_required() {
        // What the first routine relied on every time.
        let proven = [
            "image-generator-form",
            "tti-mode-selector-v3-trigger",
            "number-images-value",
            "image-references-input",
            "reference-add-button",
            "image-prompt-input",
            "image-generate-button",
        ];
        assert_eq!(REQUIRED, &proven[..]);
        // The size menu depends on the model selected (none with Seedream 4): never required.
        assert!(OPTIONAL.contains(&"image-output-axes-trigger") && !REQUIRED.contains(&"image-output-axes-trigger"));
        // Newer controls and menu entries degrade quietly: optional, never required. The unlimited
        // switch appears only after a model is picked, so the read-only check cannot require it.
        for n in [
            "unlimited-mode-toggle-button",
            "tti-model-selector-popover-model-imagen-nano-banana-2-1",
            "axis-option-resolution-1k",
            "increase-number-images-button",
            "increment-number-images-button",
            "decrease-number-images-button",
            "image-aspect-ratio-input",
            "image-style-add-button",
            "style-reference-add-button",
            "image-character-add-button",
            "character-reference-add-button",
        ] {
            assert!(OPTIONAL.contains(&n), "{n} is optional");
            assert!(!REQUIRED.contains(&n), "{n} is not required");
        }
        let both: Vec<&&str> = REQUIRED.iter().filter(|n| OPTIONAL.contains(n)).collect();
        assert!(both.is_empty(), "{both:?}");
    }

    #[test]
    fn the_model_list_checks_the_sign_in_first() {
        let js = models_script();
        let at = |s: &str| js.find(s).unwrap_or_else(|| panic!("{s} is in the routine"));
        assert!(at(r#"if (human()) return fail("human""#) < at("if (signInPage()) return fail(\"signin\""));
        assert!(at("if (signInPage()) return fail(\"signin\"") < at("location.assign(P.generator)"));
        assert!(at("if (signedOut(/^(log ?in|sign ?in)$/i)) return fail(\"signin\"") < at("size.click()"));
        assert!(!js.contains("\"(\" + location.pathname"));
    }
}
