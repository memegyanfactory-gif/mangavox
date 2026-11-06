//! The Story step survives a hiccup: a failed call is sent again, a cut-off answer is rescued, and
//! a part the AI cannot answer is told in two halves instead of failing the chapter.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// Answers in order; an `Err` is a failed call.
struct Script {
    steps: Mutex<VecDeque<std::result::Result<String, String>>>,
    seen: Mutex<Vec<AiStepRequest>>,
    calls: AtomicUsize,
}

impl Script {
    fn new(steps: Vec<std::result::Result<&str, &str>>) -> Self {
        let steps = steps.into_iter().map(|s| s.map(str::to_string).map_err(str::to_string)).collect();
        Script { steps: Mutex::new(steps), seen: Mutex::new(vec![]), calls: AtomicUsize::new(0) }
    }
}

impl Llm for Script {
    fn step<'a>(&'a self, req: AiStepRequest, _c: CancellationToken) -> BoxFuture<'a, Result<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(req);
        let a = self.steps.lock().unwrap().pop_front();
        Box::pin(async move {
            match a {
                Some(Ok(t)) => Ok(t),
                Some(Err(e)) => Err(anyhow!(e)),
                None => Err(anyhow!("script ran out")),
            }
        })
    }
}

const BOTH: &str = r#"{"title":"Duel","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."},{"page":2,"panel":null,"text":"Kaito speaks."}]}"#;
const FIRST: &str = r#"{"title":"Duel","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."}]}"#;
const SECOND: &str = r#"{"summary_so_far":"s2","segments":[{"page":2,"panel":null,"text":"Kaito speaks."}]}"#;

/// A two-page chapter that is read, ready for its story.
async fn read_two(dir: &Path) -> (ChapterRef, Mutex<Brain>) {
    let ch = chapter(dir, 2);
    let brain = Mutex::new(Brain::load_in(dir.join("brain"), "m1", "Test Manga"));
    let llm = Fake::new(&[PAGE1, PAGE2]);
    read_chapter_with(&llm, &model(), &ch, &brain, &ReadOptions::default(), &silent(), &CancellationToken::new()).await.unwrap();
    (ch, brain)
}

async fn story_with(llm: &Script, ch: &ChapterRef, brain: &Mutex<Brain>) -> Result<Vec<(String, Story)>> {
    write_story_with(llm, &model(), std::slice::from_ref(ch), &[], &[], brain, &silent(), &CancellationToken::new()).await
}

fn texts(s: &Story) -> Vec<&str> {
    s.segments.iter().map(|x| x.text.as_str()).collect()
}

#[tokio::test]
async fn a_failed_call_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Ok(CAST_OK), Err("the AI program exited with code 1"), Err("timed out"), Ok(BOTH)]);
    let out = story_with(&llm, &ch, &brain).await.unwrap();
    assert_eq!(texts(&out[0].1), ["Ruko draws.", "Kaito speaks."]);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 4, "the cast check, a call that failed twice, then the story");
}

#[tokio::test]
async fn a_call_that_keeps_failing_stops_after_three_tries() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Ok(CAST_OK), Err("boom"), Err("boom"), Err("boom"), Ok(BOTH)]);
    let e = story_with(&llm, &ch, &brain).await.unwrap_err();
    assert!(e.to_string().contains("boom"));
    assert_eq!(llm.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_provider_message_or_an_empty_reply_is_a_failed_call_not_text() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Ok(CAST_OK), Ok("You've hit your session limit · resets 8am (Asia/Kolkata)"), Ok("  "), Ok(BOTH)]);
    let out = story_with(&llm, &ch, &brain).await.unwrap();
    assert_eq!(out[0].1.segments.len(), 2);
    // Nothing of the provider's message went into a nudge.
    assert!(llm.seen.lock().unwrap().iter().all(|r| r.messages.iter().all(|m| !m.content.contains("session limit"))));
}

#[tokio::test]
async fn a_cut_off_answer_is_kept_and_the_rest_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let cut = r#"{"title":"Duel","summary_so_far":"s","segments":[{"page":1,"panel":1,"text":"Ruko draws."},{"page":2,"panel":null,"text":"Kait"#;
    // The cut answer, the nudge answered with the same cut, then the follow-up for page 2.
    let llm = Script::new(vec![Ok(CAST_OK), Ok(cut), Ok(cut), Ok(SECOND)]);
    let out = story_with(&llm, &ch, &brain).await.unwrap();
    assert_eq!(texts(&out[0].1), ["Ruko draws.", "Kaito speaks."], "the clipped sentence is never spoken");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_part_the_ai_cannot_answer_is_told_in_two_halves() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Ok(CAST_OK), Ok("I cannot do that."), Ok("Still no JSON."), Ok(FIRST), Ok(SECOND)]);
    let out = story_with(&llm, &ch, &brain).await.unwrap();
    assert_eq!(out[0].1.title, "Duel");
    assert_eq!(texts(&out[0].1), ["Ruko draws.", "Kaito speaks."]);
    let seen = llm.seen.lock().unwrap();
    assert!(seen[3].messages.iter().any(|m| m.content.contains("pages 1-1")), "the first half asks for page 1 only");
    assert!(seen[4].messages.iter().any(|m| m.content.contains("pages 2-2")), "the second half asks for page 2 only");
}

#[tokio::test]
async fn a_part_with_no_usable_segments_is_told_in_two_halves() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Ok(CAST_OK), Ok(r#"{"title":"Duel","segments":[]}"#), Ok(FIRST), Ok(SECOND)]);
    let out = story_with(&llm, &ch, &brain).await.unwrap();
    assert_eq!(texts(&out[0].1), ["Ruko draws.", "Kaito speaks."]);
}

#[tokio::test]
async fn one_page_that_still_fails_fails_the_chapter_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    // The whole part, its first half, then the first page on its own: never a story.
    let llm = Script::new(vec![Ok(CAST_OK), Ok("no"), Ok("no"), Ok("no"), Ok("no")]);
    let e = story_with(&llm, &ch, &brain).await.unwrap_err();
    assert!(e.to_string().contains("page 1 even on its own"), "{e}");
}

#[tokio::test]
async fn cancelling_stops_at_once_without_a_retry() {
    let dir = tempfile::tempdir().unwrap();
    let (ch, brain) = read_two(dir.path()).await;
    let llm = Script::new(vec![Err("cancelled"), Ok(CAST_OK), Ok(BOTH)]);
    let e = story_with(&llm, &ch, &brain).await.unwrap_err();
    assert_eq!(e.to_string(), "cancelled");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
}
