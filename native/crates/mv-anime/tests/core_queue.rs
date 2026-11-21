//! The job queue (design 5.7): persistence, a simulated restart (a running job comes back
//! queued and held, never lost, never run twice), lanes paused on their own, pause and resume
//! of one job, de-duplication, cancel, automatic retry of a passing failure, and the Web lane
//! stopping on a sign-in. This test binary points `MANGAVOX_HOME` at its own temp folder.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use futures::future::BoxFuture;
use mv_anime::backend::{ImageError, NoImages};
use mv_anime::queue::{self, Job, JobCtx, JobKind, JobState, Lane, Queue, RunCtx};

fn home() -> &'static Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp folder");
        std::env::set_var("MANGAVOX_HOME", dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        dir
    })
    .path()
}

fn story(ep: &str) -> JobKind {
    JobKind::Story { episode: ep.into() }
}

fn job(id: u64, kind: JobKind, state: JobState) -> Job {
    Job { id, project: "p".into(), lane: kind.lane(), kind, state, label: format!("job {id}"), ..Job::default() }
}

// ------------------------------------------------------------------ the journal on its own

#[test]
fn a_restart_requeues_the_running_job_and_holds_the_queue() {
    home();
    let dir = tempfile::tempdir().expect("temp folder");
    let path = dir.path().join("queue.json");
    let q = Queue {
        next_id: 4,
        jobs: vec![job(1, story("ep01"), JobState::Running), job(2, story("ep02"), JobState::Done), job(3, JobKind::Crops { character: "jin".into() }, JobState::Paused)],
        ..Queue::default()
    };
    q.save(&path).expect("saved");
    let back = Queue::load(&path);
    assert_eq!(back.jobs.len(), 3, "nothing lost");
    assert_eq!(back.jobs[0].state, JobState::Queued);
    assert_eq!(back.jobs[0].started_at, None);
    assert_eq!(back.jobs[1].state, JobState::Done, "a finished job stays finished");
    assert_eq!(back.jobs[2].state, JobState::Paused);
    assert!(back.interrupted && back.held());
    assert_eq!(back.next_id, 4);
    // Nothing was waiting: not held.
    let idle = Queue { next_id: 9, jobs: vec![job(1, story("x"), JobState::Done)], ..Queue::default() };
    idle.save(&path).expect("saved");
    assert!(!Queue::load(&path).held());
}

#[test]
fn a_missing_or_corrupt_journal_starts_empty() {
    home();
    let dir = tempfile::tempdir().expect("temp folder");
    let path = dir.path().join("queue.json");
    let fresh = Queue::load(&path);
    assert!(fresh.jobs.is_empty() && !fresh.held());
    assert_eq!(fresh.next_id, 1);
    std::fs::write(&path, b"{\"jobs\": [ {\"id\": 1, ").expect("written");
    let q = Queue::load(&path);
    assert!(q.jobs.is_empty());
    let aside: Vec<_> = std::fs::read_dir(dir.path()).expect("listing").filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(aside.iter().any(|n| n.starts_with("queue.json.corrupt-")), "{aside:?}");
}

#[test]
fn identical_waiting_jobs_are_not_added_twice() {
    let mut q = Queue::default();
    let a = q.add(story("ep01"), "p", "Story".into());
    assert_eq!(q.add(story("ep01"), "p", "Story again".into()), a);
    assert_ne!(q.add(story("ep02"), "p", "Story".into()), a);
    assert_ne!(q.add(story("ep01"), "other", "Story".into()), a);
    assert_eq!(q.jobs.len(), 3);
    assert_eq!(q.jobs[0].lane, Lane::Llm);
    // Once it ran, the same work can be queued again (a new id, never a reused one).
    q.jobs[0].state = JobState::Done;
    let b = q.add(story("ep01"), "p", "Story".into());
    assert!(b > a && q.jobs.len() == 4);
    let f = q.add(JobKind::Frame { episode: "ep01".into(), shot: "sh0001".into(), variants: 2 }, "p", "Frame".into());
    assert_eq!(q.job(f).map(|j| j.lane), Some(Lane::Web));
}

#[test]
fn a_running_job_gets_one_follow_up_and_a_paused_one_runs_again() {
    let mut q = Queue::default();
    let cut = JobKind::Animatic { episode: "ep01".into() };
    let a = q.add(cut.clone(), "p", "Make the animatic".into());
    q.jobs[0].state = JobState::Running;
    // It read its inputs when it started: a new request queues one copy behind it, never two.
    let b = q.add(cut.clone(), "p", "Make the animatic".into());
    assert_ne!(b, a);
    assert_eq!(q.add(cut.clone(), "p", "Make the animatic".into()), b);
    assert_eq!(q.jobs.len(), 2);
    // A paused request asked for again waits for its turn again.
    q.jobs[1].state = JobState::Paused;
    assert_eq!(q.add(cut.clone(), "p", "Make the animatic".into()), b);
    assert_eq!(q.job(b).map(|j| j.state), Some(JobState::Queued));
    q.jobs[0].state = JobState::Done;
    q.jobs[1].state = JobState::Done;
    assert!(q.add(cut, "p", "Make the animatic".into()) > b);
    // A picture being drawn is never asked for twice (a second Generate on the user's account).
    let frame = JobKind::Frame { episode: "ep01".into(), shot: "sh0001".into(), variants: 1 };
    let f = q.add(frame.clone(), "p", "Frame".into());
    if let Some(j) = q.jobs.iter_mut().find(|j| j.id == f) {
        j.state = JobState::Running;
    }
    assert_eq!(q.add(frame, "p", "Frame".into()), f);
}

#[test]
fn only_the_newest_finished_jobs_are_kept() {
    let mut q = Queue::default();
    for id in 1..=250u64 {
        let state = if id % 50 == 0 { JobState::Queued } else if id % 7 == 0 { JobState::Failed } else { JobState::Done };
        q.jobs.push(Job { finished_at: Some(id as i64), ..job(id, story(&format!("ep{id}")), state) });
    }
    let waiting = q.jobs.iter().filter(|j| j.state == JobState::Queued).count();
    let gone = q.prune_finished(queue::KEEP_FINISHED);
    assert_eq!(gone, 250 - waiting - queue::KEEP_FINISHED);
    assert_eq!(q.jobs.iter().filter(|j| j.state == JobState::Queued).count(), waiting, "waiting jobs are never dropped");
    assert!(q.job(1).is_none() && q.job(249).is_some(), "the oldest finished go first");
    assert_eq!(q.prune_finished(queue::KEEP_FINISHED), 0);
}

#[test]
fn cancelling_the_waiting_jobs_leaves_the_rest() {
    let mut q = Queue { interrupted: true, ..Queue::default() };
    q.jobs = vec![
        job(1, story("ep01"), JobState::Running),
        job(2, story("ep02"), JobState::Queued),
        job(3, story("ep03"), JobState::Paused),
        job(4, story("ep04"), JobState::Done),
        Job { project: "other".into(), ..job(5, story("ep05"), JobState::Queued) },
    ];
    assert_eq!(q.drop_waiting(Some("p")), vec![2, 3]);
    assert_eq!(q.jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![1, 4, 5]);
    assert!(q.interrupted, "the other project's job still waits");
    assert_eq!(q.drop_waiting(None), vec![5]);
    assert!(!q.interrupted, "nothing waits, so the restart hold is gone");
    // A removed project takes its unfinished jobs with it.
    q.jobs.push(job(6, story("ep06"), JobState::Failed));
    assert_eq!(q.drop_project("p"), vec![1, 6]);
    assert_eq!(q.jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![4]);
}

#[test]
fn unknown_job_kinds_survive_a_round_trip() {
    let raw = r#"{"next_id":3,"jobs":[{"id":2,"project":"p","kind":{"kind":"lipsync","shot":"sh1"},"state":"done"}]}"#;
    let q: Queue = serde_json::from_str(raw).expect("reads");
    assert_eq!(q.jobs[0].kind, JobKind::Unknown);
    assert_eq!(q.jobs[0].state, JobState::Done);
}

// ------------------------------------------------------------------ the process-wide queue

/// How many times each job ran, and which were stopped by their cancel token.
fn runs() -> &'static Mutex<HashMap<u64, u32>> {
    static R: OnceLock<Mutex<HashMap<u64, u32>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stopped() -> &'static Mutex<Vec<u64>> {
    static S: OnceLock<Mutex<Vec<u64>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

fn ran(id: u64) -> u32 {
    runs().lock().map(|r| r.get(&id).copied().unwrap_or(0)).unwrap_or(0)
}

/// The test's step: behaves by the job's label.
fn exec<'a>(cx: &'a JobCtx<'a>) -> BoxFuture<'a, anyhow::Result<()>> {
    Box::pin(async move {
        let id = cx.job.id;
        let n = {
            let mut r = runs().lock().map_err(|_| anyhow!("poisoned"))?;
            let n = r.entry(id).or_insert(0);
            *n += 1;
            *n
        };
        (cx.progress)(0.5, "half way");
        match cx.job.label.as_str() {
            "block" => {
                cx.cancel.cancelled().await;
                if let Ok(mut s) = stopped().lock() {
                    s.push(id);
                }
                Err(anyhow!("cancelled"))
            }
            "ok-after-stop" => {
                cx.cancel.cancelled().await;
                Ok(())
            }
            "busy-once" if n == 1 => Err(anyhow!("The site is busy right now")),
            "signin" => Err(anyhow::Error::new(ImageError::NeedSignIn)),
            "broken" => Err(anyhow!("The step found nothing to do")),
            _ => {
                tokio::time::sleep(Duration::from_millis(30)).await;
                Ok(())
            }
        }
    })
}

fn wait_for(what: &str, secs: u64, cond: impl Fn(&Queue) -> bool) {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if cond(&queue::snapshot()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for: {what}\n{:#?}", queue::snapshot());
}

fn state_of(q: &Queue, id: u64) -> Option<JobState> {
    q.job(id).map(|j| j.state)
}

#[test]
fn the_runner_end_to_end() {
    let root = home();
    // The journal MangaVox left behind: job 1 was running when it closed, job 2 had finished.
    let journal = root.join("anime").join("queue.json");
    Queue { next_id: 3, jobs: vec![job(1, story("ep01"), JobState::Running), job(2, story("ep02"), JobState::Done)], ..Queue::default() }.save(&journal).expect("journal");

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    let ctx = Arc::new(RunCtx { ai: Box::new(|_| None), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow!("No voice Python in tests."))) });
    // Loaded once from a background thread, as the app does at start-up.
    std::thread::spawn(queue::warm).join().expect("warm");
    queue::start_runner_with(rt.handle(), ctx, Arc::new(exec));
    let v0 = queue::version();

    // Restart: job 1 is queued again and held; nothing runs before Resume.
    let q = queue::snapshot();
    assert_eq!(state_of(&q, 1), Some(JobState::Queued));
    assert_eq!(state_of(&q, 2), Some(JobState::Done));
    assert!(q.interrupted);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(ran(1), 0, "held after a restart");
    queue::resume_all();
    wait_for("job 1 done", 10, |q| state_of(q, 1) == Some(JobState::Done));
    assert_eq!((ran(1), ran(2)), (1, 0), "the interrupted job ran once, the finished one never");
    assert!(queue::version() > v0);

    // De-duplication and a lane paused on its own.
    queue::pause_lane(Lane::Web);
    let frame = JobKind::Frame { episode: "ep01".into(), shot: "sh0001".into(), variants: 1 };
    let a = queue::enqueue(frame.clone(), "p", "frame".into());
    assert_eq!(queue::enqueue(frame, "p", "frame".into()), a, "an identical queued job is not added twice");
    let c = queue::enqueue(story("ep03"), "p", "story".into());
    wait_for("the Llm lane runs while Web is paused", 10, |q| state_of(q, c) == Some(JobState::Done));
    assert_eq!(state_of(&queue::snapshot(), a), Some(JobState::Queued));
    assert_eq!(ran(a), 0);
    queue::resume_lane(Lane::Web);
    wait_for("the frame runs after the lane resumes", 10, |q| state_of(q, a) == Some(JobState::Done));
    assert_eq!(ran(a), 1);

    // Pause and resume one job.
    queue::pause_lane(Lane::Render);
    let d = queue::enqueue(JobKind::Crops { character: "jin".into() }, "p", "crops".into());
    queue::pause(d);
    queue::resume_lane(Lane::Render);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(state_of(&queue::snapshot(), d), Some(JobState::Paused));
    assert_eq!(ran(d), 0);
    queue::resume(d);
    wait_for("the resumed job runs", 10, |q| state_of(q, d) == Some(JobState::Done));

    // Cancel a queued job: gone, never run.
    queue::pause_lane(Lane::Voice);
    let e = queue::enqueue(JobKind::Voice { episode: "ep01".into(), line: "ln0001".into() }, "p", "voice".into());
    queue::cancel(e);
    assert_eq!(state_of(&queue::snapshot(), e), None);
    queue::resume_lane(Lane::Voice);

    // A running job: the journal on disk says Running; a restart now would queue it again.
    let f = queue::enqueue(JobKind::Animatic { episode: "ep01".into() }, "p", "block".into());
    wait_for("the blocking job runs", 10, |q| state_of(q, f) == Some(JobState::Running));
    assert!(queue::flush_journal(Duration::from_secs(5)), "the writer lands the newest state");
    let after_crash = Queue::load(&journal);
    assert_eq!(state_of(&after_crash, f), Some(JobState::Queued));
    assert!(after_crash.interrupted);
    assert_eq!(state_of(&after_crash, 1), Some(JobState::Done));
    assert_eq!(after_crash.jobs.len(), queue::snapshot().jobs.len(), "every job is in the journal");
    // Cancel it while it runs: its step is stopped and the lane is free again.
    queue::cancel(f);
    assert_eq!(state_of(&queue::snapshot(), f), None);
    let until = Instant::now() + Duration::from_secs(5);
    while !stopped().lock().map(|s| s.contains(&f)).unwrap_or(false) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(stopped().lock().map(|s| s.contains(&f)).unwrap_or(false), "the running step saw the cancel");
    let g = queue::enqueue(JobKind::Animatic { episode: "ep02".into() }, "p", "render".into());
    wait_for("the Render lane is free after a cancel", 10, |q| state_of(q, g) == Some(JobState::Done));

    // A passing failure is retried by itself; a plain failure is not.
    let h = queue::enqueue(story("ep04"), "p", "busy-once".into());
    wait_for("the busy job succeeds on its retry", 20, |q| state_of(q, h) == Some(JobState::Done));
    assert_eq!(ran(h), 2);
    assert_eq!(queue::snapshot().job(h).map(|j| j.tries), Some(2));
    let i = queue::enqueue(story("ep05"), "p", "broken".into());
    wait_for("the broken job fails", 10, |q| state_of(q, i) == Some(JobState::Failed));
    assert_eq!(ran(i), 1);
    assert_eq!(queue::snapshot().job(i).and_then(|j| j.error.clone()).as_deref(), Some("The step found nothing to do"));

    // A sign-in stop fails the job without retrying and pauses the Web lane.
    let j = queue::enqueue(JobKind::Sheet { target: mv_anime::Target::StyleFrame, variants: 1 }, "p", "signin".into());
    wait_for("the sign-in stop", 10, |q| state_of(q, j) == Some(JobState::Failed));
    assert_eq!(ran(j), 1);
    assert!(queue::snapshot().lane_paused(Lane::Web));

    // Retry renews a failed job; with its lane paused it waits.
    queue::retry(j);
    assert_eq!(state_of(&queue::snapshot(), j), Some(JobState::Queued));
    queue::cancel(j);

    // Hide failed jobs, clear finished ones.
    queue::set_hidden_failed(true);
    assert!(queue::snapshot().job(i).is_some_and(|j| j.hidden));
    queue::clear_finished();
    let q = queue::snapshot();
    assert!(q.jobs.iter().all(|j| j.state != JobState::Done));
    assert_eq!(state_of(&q, i), Some(JobState::Failed));

    // Pause all: nothing new starts until Resume.
    queue::resume_lane(Lane::Web);
    queue::pause_all();
    let k = queue::enqueue(story("ep06"), "p", "story".into());
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(ran(k), 0);
    queue::resume_all();
    wait_for("runs after Resume all", 10, |q| state_of(q, k) == Some(JobState::Done));

    // Its work was saved just as Pause landed: done, never run again on Resume.
    let m = queue::enqueue(JobKind::Animatic { episode: "ep03".into() }, "p", "ok-after-stop".into());
    wait_for("the job runs", 10, |q| state_of(q, m) == Some(JobState::Running));
    queue::pause(m);
    wait_for("done despite the pause", 10, |q| state_of(q, m) == Some(JobState::Done));
    assert_eq!(ran(m), 1);

    // RUN1-24: pausing a running job is not one of its tries, however often the user does it.
    let p = queue::enqueue(JobKind::Animatic { episode: "ep04".into() }, "p", "block".into());
    for round in 1..=3u32 {
        wait_for("the job runs", 10, |q| state_of(q, p) == Some(JobState::Running));
        assert_eq!(queue::snapshot().job(p).map(|j| j.tries), Some(1), "round {round}: one try while it runs");
        queue::pause(p);
        let until = Instant::now() + Duration::from_secs(5);
        while stopped().lock().map(|s| s.iter().filter(|x| **x == p).count() < round as usize).unwrap_or(true) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(ran(p), round);
        assert_eq!(state_of(&queue::snapshot(), p), Some(JobState::Paused));
        assert_eq!(queue::snapshot().job(p).map(|j| j.tries), Some(0), "round {round}: the pause gave its try back");
        queue::resume(p);
    }
    queue::cancel(p);

    // A picture job is never run again by itself, even when the site says it is busy.
    let w = queue::enqueue(JobKind::Frame { episode: "ep01".into(), shot: "sh0002".into(), variants: 1 }, "p", "busy-once".into());
    wait_for("the busy picture fails once", 10, |q| state_of(q, w) == Some(JobState::Failed));
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(ran(w), 1, "no automatic second Generate");
    assert_eq!(state_of(&queue::snapshot(), w), Some(JobState::Failed));
    assert!(!queue::snapshot().lane_paused(Lane::Web), "a plain failure does not stop the lane");

    // Removing a project stops its running job and waits until it has wound down.
    let r = queue::enqueue(JobKind::Animatic { episode: "ep01".into() }, "gone", "block".into());
    wait_for("the doomed job runs", 10, |q| state_of(q, r) == Some(JobState::Running));
    assert!(queue::cancel_project_wait("gone", Duration::from_secs(5)), "every task stopped");
    assert!(stopped().lock().map(|s| s.contains(&r)).unwrap_or(false));
    assert_eq!(state_of(&queue::snapshot(), r), None);

    // The journal on disk matches the queue.
    assert!(queue::flush_journal(Duration::from_secs(5)));
    assert_eq!(Queue::load(&journal).jobs.len(), queue::snapshot().jobs.len());
    drop(rt);
}
