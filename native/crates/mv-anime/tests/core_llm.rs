//! The LLM port with canned replies (design 3.4, 11.1): good JSON, cut-off JSON, junk, provider
//! errors, a usage limit, a text-only model and a cancel. No network: `Ai::fake` never calls
//! the registry.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mv_ai::{AiEffort, AiProviderId, AiRole, AiStepResult, CancellationToken};
use mv_anime::llm::{is_cancelled, Ai, BadReply, Canned, UNREADABLE};
use serde_json::{json, Value};

struct Run {
    ai: Ai,
    canned: Arc<Canned>,
}

fn canned(results: Vec<AiStepResult>) -> Run {
    let canned = Arc::new(Canned::from_results(results));
    Run { ai: Ai::fake(canned.clone()), canned }
}

fn text(t: &str) -> AiStepResult {
    AiStepResult { text: t.into(), ..AiStepResult::default() }
}

async fn ask(ai: &Ai, images: Vec<PathBuf>) -> (anyhow::Result<Value>, Vec<String>) {
    let log = Mutex::new(Vec::new());
    let progress = |s: String| {
        if let Ok(mut l) = log.lock() {
            l.push(s);
        }
    };
    let r = ai.ask_json("You are a test.", "Reply with JSON.".into(), images, AiEffort::High, &progress, &CancellationToken::new()).await;
    (r, log.into_inner().unwrap_or_default())
}

#[tokio::test]
async fn good_json_comes_back_as_is() {
    let run = canned(vec![text(r#"{"scenes":[{"title":"Gate"}]}"#)]);
    let (r, _) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"scenes":[{"title":"Gate"}]})));
    let seen = run.canned.seen();
    assert_eq!(seen.len(), 1);
    let req = &seen[0];
    assert!(req.request_id.starts_with("anime-"), "{}", req.request_id);
    assert!(req.lean && req.tools.is_none() && req.access.is_none() && req.mcp.is_none());
    assert_eq!(req.effort, Some(AiEffort::High));
    assert_eq!((req.messages[0].role, req.messages[1].role), (AiRole::System, AiRole::User));
    assert_eq!(req.messages[1].content, "Reply with JSON.");
}

#[tokio::test]
async fn json_inside_prose_and_a_fence_is_found() {
    let run = canned(vec![text("Here is the plan:\n```json\n{\"shots\": [1, 2]}\n```\nDone.")]);
    let (r, _) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"shots": [1, 2]})));
}

#[tokio::test]
async fn a_cut_off_reply_keeps_what_it_finished() {
    // Missing only its closing brackets: closed.
    let run = canned(vec![text(r#"{"scenes":[{"title":"A"},{"title":"B"}"#)]);
    let (r, _) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"scenes":[{"title":"A"},{"title":"B"}]})));
    assert_eq!(run.canned.seen().len(), 1, "no second call for a reply that could be closed");
    // Cut off inside a string: the finished scene is kept.
    let run = canned(vec![text(r#"{"scenes":[{"title":"A"},{"title":"B"#)]);
    let (r, _) = ask(&run.ai, vec![]).await;
    let v = r.expect("salvaged");
    assert_eq!(v["scenes"][0]["title"], "A");
}

#[tokio::test]
async fn junk_gets_one_nudge_then_reads() {
    let run = canned(vec![text("I think these scenes are lovely and very dramatic."), text(r#"{"ok":true}"#)]);
    let (r, log) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"ok": true})));
    let seen = run.canned.seen();
    assert_eq!(seen.len(), 2);
    let nudge = &seen[1];
    assert_eq!(nudge.messages.len(), 4, "system, user, the junk answer, the nudge");
    assert_eq!(nudge.messages[2].role, AiRole::Assistant);
    assert!(nudge.messages[3].content.contains("valid JSON"));
    assert!(log.iter().any(|l| l.contains("not valid JSON")), "{log:?}");
}

#[tokio::test]
async fn junk_twice_fails_cleanly_with_the_raw_reply() {
    let run = canned(vec![text("No JSON here, sorry."), text("Still none.")]);
    let (r, _) = ask(&run.ai, vec![]).await;
    let e = r.expect_err("unreadable");
    assert_eq!(e.to_string(), UNREADABLE);
    assert_eq!(e.downcast_ref::<BadReply>().map(|b| b.raw.as_str()), Some("Still none."));
    assert!(!is_cancelled(&e));
}

#[tokio::test]
async fn provider_errors_and_empty_answers_are_retried_twice() {
    let run = canned(vec![Canned::error("503 Service Unavailable"), text("   "), text(r#"{"n":3}"#)]);
    let (r, log) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"n": 3})));
    assert_eq!(run.canned.seen().len(), 3);
    assert_eq!(log.iter().filter(|l| l.contains("trying again")).count(), 2, "{log:?}");
    // Three failures in a row: the last one is the error.
    let run = canned(vec![Canned::error("first"), Canned::error("second"), Canned::error("third")]);
    let (r, _) = ask(&run.ai, vec![]).await;
    assert_eq!(r.expect_err("failed").to_string(), "third");
}

#[tokio::test]
async fn a_usage_limit_is_waited_out_and_the_same_request_sent_again() {
    let run = canned(vec![Canned::limit("You've hit your usage limit", None), text(r#"{"after":"wait"}"#)]);
    let (r, log) = ask(&run.ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"after": "wait"})));
    let seen = run.canned.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], seen[1], "the very same request");
    assert!(log.iter().any(|l| l.contains("usage limit") && l.contains("Waiting")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("answers again")), "{log:?}");
}

#[tokio::test]
async fn a_text_only_model_gets_no_pictures() {
    let canned = Arc::new(Canned::new([r#"{"ok":1}"#]));
    let mut ai = Ai::fake(canned.clone());
    ai.model.provider = AiProviderId::Antigravity;
    ai.model.model = format!("gpt-oss-120b/{}", ai.model.model);
    assert!(!ai.can_see());
    let (r, log) = ask(&ai, vec![PathBuf::from("page.jpg")]).await;
    assert!(r.is_ok());
    assert!(canned.seen()[0].images.is_empty());
    assert!(log.iter().any(|l| l.contains("can't see pictures")), "{log:?}");
}

#[tokio::test]
async fn a_model_that_says_it_cannot_see_is_asked_again_without_pictures() {
    let run = canned(vec![text("Sorry, I cannot see images in this conversation."), text(r#"{"ok":2}"#)]);
    assert!(run.ai.can_see());
    let (r, _) = ask(&run.ai, vec![PathBuf::from("page.jpg")]).await;
    assert_eq!(r.ok(), Some(json!({"ok": 2})));
    let seen = run.canned.seen();
    assert_eq!(seen[0].images, vec![PathBuf::from("page.jpg")]);
    assert!(seen[1].images.is_empty());
}

#[tokio::test]
async fn a_cancel_stops_the_call() {
    let run = canned(vec![text(r#"{"ok":true}"#)]);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let r = run.ai.ask_json("s", "u".into(), vec![], AiEffort::Low, &|_| {}, &cancel).await;
    assert!(is_cancelled(&r.expect_err("cancelled")));
}

#[tokio::test]
async fn canned_texts_shortcut() {
    let ai = Ai::canned([r#"{"list":[1,2,3]}"#]);
    let (r, _) = ask(&ai, vec![]).await;
    assert_eq!(r.ok(), Some(json!({"list": [1, 2, 3]})));
}
