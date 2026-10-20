//! Studio's issues: every failure and missing prerequisite as one sentence a person can act on,
//! plus the one-click fixes ("Chapter 4 video failed: it has no narration audio → Generate audio,
//! Retry video"). The raw error stays one click away as detail, never on its own.
//! NN/g error messages: <https://www.nngroup.com/articles/error-message-guidelines/>
use std::collections::HashMap;

use mv_recap::{PhaseStatus, Step};
use mv_ui::AlertKind;

use crate::pipeline::{self, ItemState, Queue};
use crate::run::{RunInfo, RunKind, RunState};
use crate::service::{JobInfo, JobView};

/// What a fix button does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fix {
    /// Queue chapters in Generate up to `until` (prerequisites included), making `force` again.
    Make { chapters: Vec<String>, until: Step, force: Option<Step> },
    /// Retry a failed Generate chapter from the step that failed.
    Retry(u64),
    /// Open a chapter at a step.
    Open { chapter: String, step: Step },
    OpenSetup,
    AudioSettings,
    /// The AI steps need a model: open the chapter's Read step, where the model button is.
    PickModel { chapter: Option<String> },
    ResumeQueue,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IssueAction {
    pub label: String,
    pub fix: Fix,
}

fn act(label: impl Into<String>, fix: Fix) -> IssueAction {
    IssueAction { label: label.into(), fix }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Issue {
    /// Stable id (dismissing hides it until it changes).
    pub key: String,
    pub kind: AlertKind,
    pub chapter: Option<String>,
    pub step: Option<Step>,
    /// The sentence: what is wrong, for which chapter.
    pub title: String,
    /// The raw error, for "Details".
    pub detail: Option<String>,
    pub actions: Vec<IssueAction>,
    /// A failed Generate chapter (it can be dismissed from the queue).
    pub queue_item: Option<u64>,
}

/// A chapter as issues name it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Named {
    /// "Chapter 4".
    pub label: String,
}

/// Why a step failed, as the end of a sentence, and the fixes that make sense for it.
pub(crate) fn explain(step: Step, raw: &str, chapter: &str, item: Option<u64>) -> (String, Vec<IssueAction>) {
    let low = raw.to_lowercase();
    let ch = || vec![chapter.to_string()];
    let make = |until: Step, force: Option<Step>| Fix::Make { chapters: ch(), until, force };
    let retry_fix = || match item {
        Some(id) => Fix::Retry(id),
        None => make(step, Some(step)),
    };
    let retry = || act(format!("Retry {}", step.label().to_lowercase()), retry_fix());
    let has = |needles: &[&str]| needles.iter().any(|n| low.contains(n));
    if has(&["no audio files", "no narration", &pipeline::NO_AUDIO.to_lowercase()]) {
        return ("it has no narration audio".into(), vec![act("Generate audio", make(Step::Audio, None)), act("Retry video", make(Step::Video, Some(Step::Video)))]);
    }
    if has(&["no script", "no story", "script_edited", &pipeline::NO_STORY.to_lowercase()]) {
        return ("it has no story to narrate yet".into(), vec![act("Write story", make(Step::Story, None)), act("Generate audio", make(Step::Audio, None))]);
    }
    if low.contains(&pipeline::NO_READ.to_lowercase()) {
        return ("the chapter hasn't been read yet".into(), vec![act("Read chapter", make(Step::Read, None)), retry()]);
    }
    if low.contains(&pipeline::NO_MODEL.to_lowercase()) || has(&["no model", "pick an ai model"]) {
        return ("no AI model is picked".into(), vec![act("Pick a model", Fix::PickModel { chapter: Some(chapter.to_string()) }), retry()]);
    }
    if low.contains(&pipeline::NO_FFMPEG.to_lowercase()) || low.contains("ffmpeg") {
        return ("FFmpeg isn't installed, and videos are rendered with it".into(), vec![act("Open Setup", Fix::OpenSetup), retry()]);
    }
    if low.contains(&pipeline::NO_SETUP.to_lowercase()) || has(&["python and the pipeline", "no module named", "python was not found"]) {
        return ("Python or the pipeline scripts are missing".into(), vec![act("Open Setup", Fix::OpenSetup), retry()]);
    }
    if low.contains(&pipeline::VOICE_NOT_READY.to_lowercase()) || (low.contains("voice") && has(&["not found", "missing", "download", "not installed"])) {
        let who = raw.split_once(": ").map(|(_, v)| v.trim()).filter(|v| !v.is_empty() && v.len() < 40);
        let what = match who {
            Some(v) => format!("the narrator voice ({v}) isn't downloaded or set up"),
            None => "the narrator voice isn't downloaded or set up".into(),
        };
        return (what, vec![act("Open Audio settings", Fix::AudioSettings), retry()]);
    }
    if has(&["cuda", "out of memory", "oom"]) {
        return ("the voice engine ran out of graphics memory".into(), vec![retry(), act("Open Audio settings", Fix::AudioSettings)]);
    }
    if has(&["rate limit", "429", "quota", "too many requests"]) {
        return ("the AI provider is limiting requests right now".into(), vec![retry()]);
    }
    if has(&["connection", "connect", "timed out", "timeout", "network", "unreachable", "dns"]) {
        return ("Studio couldn't reach the AI model or a service it needs".into(), vec![retry()]);
    }
    if has(&["401", "403", "unauthorized", "api key", "invalid key"]) {
        return ("the AI provider refused the key (check it in Settings)".into(), vec![act("Pick a model", Fix::PickModel { chapter: Some(chapter.to_string()) }), retry()]);
    }
    if has(&["no pages", "page folder is empty", "no images"]) {
        return ("its downloaded pages are missing; download the chapter again".into(), vec![retry()]);
    }
    let first = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("it stopped with an error");
    let short: String = first.chars().take(140).collect();
    let short = short.trim_end_matches('.').to_string();
    (if short.is_empty() { "it stopped with an error".into() } else { short }, vec![retry()])
}

/// "Chapter 4 video failed: it has no narration audio."
fn failed_sentence(label: &str, step: Step, why: &str) -> String {
    format!("{label} {} failed: {why}.", step.label().to_lowercase())
}

/// What Studio knows right now, for [`collect`].
pub(crate) struct Inputs<'a> {
    pub queue: &'a Queue,
    pub jobs: &'a [JobInfo],
    pub run: Option<&'a RunInfo>,
    pub statuses: &'a HashMap<String, PhaseStatus>,
    pub names: &'a HashMap<String, Named>,
    /// The checked setup: (can run jobs, has FFmpeg, default voice usable + its name).
    pub setup: Option<(bool, bool, Option<String>)>,
    /// Chapters the open step works on, and that step (prerequisite gaps).
    pub targets: &'a [String],
    pub step: Option<Step>,
}

/// Every issue, worst first.
pub(crate) fn collect(i: &Inputs) -> Vec<Issue> {
    let mut out: Vec<Issue> = vec![];
    let label = |id: &str| i.names.get(id).map(|n| n.label.clone()).unwrap_or_else(|| "A chapter".into());

    // Setup problems block every chapter, so they come first.
    if let Some((runnable, ffmpeg, voice)) = &i.setup {
        if !runnable {
            out.push(Issue {
                key: "setup-python".into(),
                kind: AlertKind::Error,
                chapter: None,
                step: None,
                title: "Studio can't make audio or video yet: Python or the pipeline scripts are missing.".into(),
                detail: None,
                actions: vec![act("Open Setup", Fix::OpenSetup)],
                queue_item: None,
            });
        }
        if !ffmpeg {
            out.push(Issue {
                key: "setup-ffmpeg".into(),
                kind: AlertKind::Warning,
                chapter: None,
                step: Some(Step::Video),
                title: "FFmpeg isn't installed, so videos can't be rendered.".into(),
                detail: None,
                actions: vec![act("Open Setup", Fix::OpenSetup)],
                queue_item: None,
            });
        }
        if let Some(v) = voice {
            out.push(Issue {
                key: format!("setup-voice-{v}"),
                kind: AlertKind::Warning,
                chapter: None,
                step: Some(Step::Audio),
                title: format!("Your narrator voice ({v}) isn't ready, so chapters can't be narrated with it."),
                detail: None,
                actions: vec![act("Open Audio settings", Fix::AudioSettings)],
                queue_item: None,
            });
        }
    }

    // Generate: chapters that failed, and a queue held since MangaVox closed.
    let mut covered: Vec<&str> = vec![];
    for it in i.queue.items.iter().filter(|it| it.state == ItemState::Failed) {
        let step = it.current_step().unwrap_or(Step::Read);
        let raw = it.error.clone().unwrap_or_default();
        let name = i.names.get(&it.chapter.id).map(|n| n.label.clone()).unwrap_or_else(|| it.chapter.label());
        let (why, mut actions) = explain(step, &raw, &it.chapter.id, Some(it.id));
        actions.push(act(format!("Open {}", step.label()), Fix::Open { chapter: it.chapter.id.clone(), step }));
        out.push(Issue {
            key: format!("q-{}-{}", it.id, step.index()),
            kind: AlertKind::Error,
            chapter: Some(it.chapter.id.clone()),
            step: Some(step),
            title: failed_sentence(&name, step, &why),
            detail: (!raw.is_empty()).then_some(raw),
            actions,
            queue_item: Some(it.id),
        });
        covered.push(&it.chapter.id);
    }
    if i.queue.interrupted && i.queue.paused {
        let n = i.queue.items.iter().filter(|it| it.state.active()).count();
        if n > 0 {
            out.push(Issue {
                key: "queue-interrupted".into(),
                kind: AlertKind::Info,
                chapter: None,
                step: None,
                title: format!("Generate was paused when MangaVox closed. {n} chapter{} still to make.", crate::page::plural(n)),
                detail: None,
                actions: vec![act("Resume", Fix::ResumeQueue)],
                queue_item: None,
            });
        }
    }

    // Audio and video jobs started by hand: the latest per chapter and kind, while it is the
    // newest word on that chapter (a later success or a running retry clears it).
    let mut seen: Vec<(String, bool)> = vec![];
    for j in i.jobs.iter() {
        // Newest first.
        let render = j.stages.iter().any(|s| s == "video");
        if seen.contains(&(j.chapter_id.clone(), render)) {
            continue;
        }
        seen.push((j.chapter_id.clone(), render));
        let JobView::Failed(raw) = &j.state else { continue };
        if covered.contains(&j.chapter_id.as_str()) || i.queue.items.iter().any(|it| it.chapter.id == j.chapter_id && it.state.active()) {
            continue;
        }
        let step = if render { Step::Video } else { Step::Audio };
        let (why, mut actions) = explain(step, raw, &j.chapter_id, None);
        actions.push(act(format!("Open {}", step.label()), Fix::Open { chapter: j.chapter_id.clone(), step }));
        let name = i.names.get(&j.chapter_id).map(|n| n.label.clone()).unwrap_or_else(|| j.chapter_label.clone());
        out.push(Issue {
            key: format!("job-{}", j.id),
            kind: AlertKind::Error,
            chapter: Some(j.chapter_id.clone()),
            step: Some(step),
            title: failed_sentence(&name, step, &why),
            detail: Some(raw.clone()),
            actions,
            queue_item: None,
        });
        covered.push(&j.chapter_id);
    }

    // The AI run (Read / Story), when it failed.
    if let Some(r) = i.run.filter(|r| matches!(r.state, RunState::Failed(_))) {
        let RunState::Failed(raw) = &r.state else { unreachable!() };
        let step = if r.kind == RunKind::Read { Step::Read } else { Step::Story };
        let chapter = r.chapter_id.clone();
        if !chapter.as_deref().is_some_and(|c| covered.contains(&c)) {
            let name = chapter.as_deref().map(label).or(r.chapter.clone()).unwrap_or_else(|| "A chapter".into());
            let (why, mut actions) = match &chapter {
                Some(c) => explain(step, raw, c, None),
                None => explain(step, raw, "", None),
            };
            if chapter.is_none() {
                actions.retain(|a| !matches!(a.fix, Fix::Make { .. }));
            }
            if let Some(c) = &chapter {
                actions.push(act(format!("Open {}", step.label()), Fix::Open { chapter: c.clone(), step }));
            }
            out.push(Issue {
                key: format!("run-{}-{}", step.index(), chapter.clone().unwrap_or_default()),
                kind: AlertKind::Error,
                chapter,
                step: Some(step),
                title: failed_sentence(&name, step, &why),
                detail: Some(raw.clone()),
                actions,
                queue_item: None,
            });
        }
    }

    // Missing prerequisites of the open step, for the chapters it works on.
    if let Some(step) = i.step {
        let gap = |id: &String| -> Option<(Step, &'static str)> {
            let st = i.statuses.get(id)?;
            match step {
                Step::Story if st.read_pages == 0 => Some((Step::Read, "hasn't been read yet, so its story can't be written")),
                Step::Audio if !st.story => Some((Step::Story, "has no story yet, so it can't be narrated")),
                Step::Music | Step::Video if st.audio_files == 0 => Some((Step::Audio, "has no narration audio")),
                _ => None,
            }
        };
        let gaps: Vec<(&String, Step, &'static str)> = i.targets.iter().filter(|c| !covered.contains(&c.as_str())).filter_map(|c| gap(c).map(|(s, w)| (c, s, w))).collect();
        let needs_label = |s: Step| match s {
            Step::Read => "Read chapter",
            Step::Story => "Write story",
            _ => "Generate audio",
        };
        if gaps.len() > 3 {
            // One line for many chapters (the same fix for all).
            let missing = gaps[0].1;
            let names: Vec<String> = gaps.iter().map(|(c, _, _)| label(c)).collect();
            let all: Vec<String> = gaps.iter().map(|(c, _, _)| (*c).clone()).collect();
            out.push(Issue {
                key: format!("gap-{}-{}", step.index(), all.join(",")),
                kind: AlertKind::Warning,
                chapter: None,
                step: Some(missing),
                title: format!("{} chapters aren't ready for {}: {}.", gaps.len(), step.label(), shorten(&names)),
                detail: None,
                actions: vec![act(format!("{} for all {}", needs_label(missing), gaps.len()), Fix::Make { chapters: all.clone(), until: missing, force: None }), act(format!("Make {} for all", step.label().to_lowercase()), Fix::Make { chapters: all, until: step, force: None })],
                queue_item: None,
            });
        } else {
            for (c, missing, why) in gaps {
                out.push(Issue {
                    key: format!("gap-{}-{c}", step.index()),
                    kind: AlertKind::Warning,
                    chapter: Some(c.clone()),
                    step: Some(missing),
                    title: format!("{} {why}.", label(c)),
                    detail: None,
                    actions: vec![act(needs_label(missing), Fix::Make { chapters: vec![c.clone()], until: missing, force: None }), act(format!("Open {}", missing.label()), Fix::Open { chapter: c.clone(), step: missing })],
                    queue_item: None,
                });
            }
        }
    }
    out
}

/// "Chapter 1, Chapter 2, Chapter 3 and 4 more".
fn shorten(names: &[String]) -> String {
    match names.len() {
        0..=3 => crate::page::join_and(&names.iter().map(String::as_str).collect::<Vec<_>>()),
        n => format!("{} and {} more", names[..3].join(", "), n - 3),
    }
}

#[cfg(test)]
mod tests {
    use super::{collect, explain, Fix, Inputs, Named};
    use crate::pipeline::{ItemState, QChapter, Queue};
    use crate::service::{JobInfo, JobView};
    use mv_recap::{PhaseStatus, Step};
    use std::collections::{HashMap, VecDeque};

    fn job(id: u64, ch: &str, video: bool, state: JobView) -> JobInfo {
        JobInfo {
            id,
            chapter_id: ch.into(),
            manga_title: "M".into(),
            chapter_label: "Chapter 4".into(),
            output_dir: "/o".into(),
            voice: String::new(),
            stages: if video { vec!["sync_map".into(), "video".into()] } else { vec!["tts".into()] },
            state,
            stage: None,
            stage_percent: 0.0,
            status: String::new(),
            logs: VecDeque::new(),
            narrator: None,
            regen_pages: vec![],
        }
    }

    fn names() -> HashMap<String, Named> {
        ["c3", "c4"].iter().enumerate().map(|(i, c)| (c.to_string(), Named { label: format!("Chapter {}", i + 3) })).collect()
    }

    #[test]
    fn raw_errors_become_sentences_with_fixes() {
        let (why, acts) = explain(Step::Video, "No audio files found — generate audio in the Audio tab first", "c4", None);
        assert_eq!(why, "it has no narration audio");
        assert_eq!(acts.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), vec!["Generate audio", "Retry video"]);
        assert_eq!(acts[1].fix, Fix::Make { chapters: vec!["c4".into()], until: Step::Video, force: Some(Step::Video) });
        let (why, acts) = explain(Step::Audio, "CUDA out of memory", "c4", Some(9));
        assert!(why.contains("graphics memory"));
        assert_eq!(acts[0].fix, Fix::Retry(9), "a queued chapter retries in the queue");
        let (why, _) = explain(Step::Read, "No AI model is picked", "c4", None);
        assert_eq!(why, "no AI model is picked");
        let (why, acts) = explain(Step::Story, "something odd\nTraceback ...", "c4", None);
        assert_eq!(why, "something odd");
        assert_eq!(acts[0].label, "Retry story");
    }

    #[test]
    fn a_failed_render_without_audio_names_the_fix() {
        let jobs = vec![job(2, "c4", true, JobView::Failed("No audio files found — generate audio in the Audio tab first".into()))];
        let statuses = HashMap::new();
        let names = names();
        let q = Queue::default();
        let issues = collect(&Inputs { queue: &q, jobs: &jobs, run: None, statuses: &statuses, names: &names, setup: None, targets: &[], step: None });
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].title, "Chapter 4 video failed: it has no narration audio.");
        assert_eq!(issues[0].actions.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), vec!["Generate audio", "Retry video", "Open Video"]);
        assert!(issues[0].detail.is_some(), "the raw error stays as detail");
    }

    #[test]
    fn a_later_success_clears_a_failure() {
        // Newest first: the render succeeded after it failed.
        let jobs = vec![job(3, "c4", true, JobView::Done), job(2, "c4", true, JobView::Failed("boom".into()))];
        let (statuses, names, q) = (HashMap::new(), names(), Queue::default());
        let issues = collect(&Inputs { queue: &q, jobs: &jobs, run: None, statuses: &statuses, names: &names, setup: None, targets: &[], step: None });
        assert!(issues.is_empty());
    }

    #[test]
    fn missing_prerequisites_of_the_open_step() {
        let mut statuses = HashMap::new();
        statuses.insert("c3".to_string(), PhaseStatus { read_pages: 5, total_pages: 5, story: true, ..Default::default() });
        statuses.insert("c4".to_string(), PhaseStatus { read_pages: 5, total_pages: 5, story: false, ..Default::default() });
        let (names, q) = (names(), Queue::default());
        let targets = vec!["c3".to_string(), "c4".to_string()];
        let issues = collect(&Inputs { queue: &q, jobs: &[], run: None, statuses: &statuses, names: &names, setup: None, targets: &targets, step: Some(Step::Audio) });
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].title, "Chapter 4 has no story yet, so it can't be narrated.");
        assert_eq!(issues[0].actions[0].label, "Write story");
        let video = collect(&Inputs { queue: &q, jobs: &[], run: None, statuses: &statuses, names: &names, setup: None, targets: &targets, step: Some(Step::Video) });
        assert_eq!(video.len(), 2);
        assert!(video.iter().all(|i| i.title.ends_with("has no narration audio.")));
    }

    #[test]
    fn failed_generate_chapters_and_setup_come_first() {
        let mut q = Queue::default();
        let ch = QChapter { id: "c3".into(), manga_id: "m".into(), manga_title: "M".into(), number: Some(3.0), title: None, pages_dir: "/p".into(), output_dir: "/o".into() };
        q.add(ch, Step::Video, None, "landscape", None);
        q.items[0].state = ItemState::Failed;
        q.items[0].current = Some(Step::Audio.index());
        q.items[0].error = Some("The narrator voice is not ready: Kokoro Heart".into());
        let (statuses, names) = (HashMap::new(), names());
        let issues = collect(&Inputs { queue: &q, jobs: &[], run: None, statuses: &statuses, names: &names, setup: Some((true, false, None)), targets: &[], step: None });
        assert_eq!(issues[0].key, "setup-ffmpeg");
        assert_eq!(issues[1].title, "Chapter 3 audio failed: the narrator voice (Kokoro Heart) isn't downloaded or set up.");
        assert_eq!(issues[1].queue_item, Some(q.items[0].id));
    }
}
