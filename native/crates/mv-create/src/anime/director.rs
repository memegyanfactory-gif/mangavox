//! The director at work: the writing steps of an anime chapter run in order by whoever can ask
//! the AI (the Creative page with the model picked in the app, or a headless run). Each step
//! takes the project and gives it back changed; nothing here draws or touches the window.
//!
//! read the panels → gather the cast from the pages → divide into scenes → storyboard each
//! scene → the episode director's check → revise the scenes it faulted (kept only when the
//! revision is better) → the board, with what is still open noted on it.
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use super::{apply_read, board, checks, panel_digest, read_panels, SourcePanel, READ_BATCH};
use crate::project::Project;

/// One question to the AI, which must answer with one JSON object.
pub struct Query {
    pub system: String,
    pub user: String,
    /// Pictures attached, for a model that can see.
    pub images: Vec<PathBuf>,
}

pub type Answer = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;
/// Whoever answers (the model picked in the app). An error that reads "stopped" is the user's
/// Stop: every step ends at once on it.
pub type Ask = Arc<dyn Fn(Query) -> Answer + Send + Sync>;
/// Told where a step is ("Storyboarding scene 3 of 7").
pub type Status = Arc<dyn Fn(String) + Send + Sync>;

/// How many times the board goes back to the director after the check.
pub const REVISIONS: usize = 2;

fn text(q: (String, String)) -> Query {
    Query { system: q.0, user: q.1, images: vec![] }
}

fn stopped(e: &anyhow::Error) -> bool {
    e.to_string() == "stopped"
}

/// Reads every unread panel of chapter `n` with the vision model, a few at a time, and returns
/// the chapter's panels with what was read.
pub async fn read(ask: &Ask, p: &Project, n: u32, status: &Status) -> Result<Vec<SourcePanel>> {
    let mut panels = p.chapter(n).map(|c| c.anime.panels.clone()).unwrap_or_default();
    let total = panels.len();
    let mut from = 0;
    while from < total {
        if panels[from].read {
            from += 1;
            continue;
        }
        // A batch never runs past the next panel that is already read.
        let count = panels[from..].iter().take(READ_BATCH).take_while(|x| !x.read).count();
        status(format!("Reading the manga: panels {}-{} of {total}", from + 1, from + count));
        let images: Vec<PathBuf> = panels[from..from + count].iter().map(|sp| p.file(&sp.file)).collect();
        let earlier = panel_digest(&panels, from.saturating_sub(3), from);
        let (system, user) = read_panels(p, n, from, count, &earlier);
        let v = ask(Query { system, user, images }).await?;
        apply_read(p, &mut panels, from, count, v)?;
        from += count;
    }
    Ok(panels)
}

/// Gathers the cast and places from the chapter's read panels (and `notes`: what Studio knows
/// of the manga). Returns the project and how many people were added.
pub async fn cast(ask: &Ask, mut p: Project, n: u32, notes: &str, status: &Status) -> Result<(Project, usize)> {
    status(format!("Gathering the characters and places of chapter {n} from its pages"));
    let v = ask(text(board::cast_from_pages(&p, n, notes))).await?;
    let added = board::apply_cast(&mut p, n, v)?;
    Ok((p, added))
}

/// Divides the chapter into scenes.
pub async fn scenes(ask: &Ask, mut p: Project, n: u32, status: &Status) -> Result<Project> {
    status(format!("Dividing chapter {n} into scenes"));
    let v = ask(text(board::scenes(&p, n))).await?;
    board::apply_scenes(&mut p, n, v)?;
    Ok(p)
}

/// Asks for one scene's board and stores it; an answer that cannot be used is asked for once more.
async fn board_one(ask: &Ask, p: &mut Project, n: u32, scene: u32, q: impl Fn(&Project) -> Option<(String, String)>) -> Result<usize> {
    let mut last = anyhow!("chapter {n} has no scene {scene}");
    for _ in 0..2 {
        let Some(query) = q(p) else { break };
        match ask(text(query)).await {
            Ok(v) => match board::apply_scene_board(p, n, scene, v) {
                Ok(cuts) => return Ok(cuts),
                Err(e) => last = e,
            },
            Err(e) if stopped(&e) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn scene_score(p: &Project, n: u32, scene: u32) -> usize {
    let found: Vec<_> = checks::check(p, n).into_iter().filter(|i| i.scene == scene).collect();
    checks::score(&found)
}

/// Sends the scenes the check faulted back to the director, up to [`REVISIONS`] times. A
/// revision is kept only when it leaves its scene better than it was.
async fn revise(ask: &Ask, mut p: Project, n: u32, only: Option<u32>, status: &Status) -> Result<Project> {
    for round in 0..REVISIONS {
        checks::autofix(&mut p, n);
        let todo: Vec<(u32, Vec<String>)> = checks::to_revise(&checks::check(&p, n)).into_iter().filter(|(s, _)| only.is_none_or(|o| o == *s)).collect();
        if todo.is_empty() {
            break;
        }
        for (scene, notes) in todo {
            status(format!("The episode director's check, round {}: scene {scene} goes back with {} note{}", round + 1, notes.len(), if notes.len() == 1 { "" } else { "s" }));
            let before = p.clone();
            let was = scene_score(&before, n, scene);
            match board_one(ask, &mut p, n, scene, |p| board::revise_scene(p, n, scene, &notes)).await {
                Ok(_) => {
                    checks::autofix(&mut p, n);
                    if scene_score(&p, n, scene) >= was {
                        p = before;
                    }
                }
                Err(e) if stopped(&e) => return Err(e),
                // The board as it was is still a board.
                Err(_) => p = before,
            }
        }
    }
    checks::autofix(&mut p, n);
    let issues = checks::check(&p, n);
    p.chapter_mut(n).anime.issues = issues;
    Ok(p)
}

/// Draws up the whole chapter's storyboard, scene by scene, then checks and revises it.
pub async fn board(ask: &Ask, mut p: Project, n: u32, note: &str, status: &Status) -> Result<Project> {
    let scenes: Vec<(u32, String)> = p.chapter(n).map(|c| c.anime.scenes.iter().map(|s| (s.number, s.title.clone())).collect()).unwrap_or_default();
    if scenes.is_empty() {
        bail!("chapter {n} has no scenes yet");
    }
    {
        let a = &mut p.chapter_mut(n).anime;
        a.cuts.clear();
        a.issues.clear();
        a.animatic = None;
        a.approved = false;
        a.video = None;
    }
    for (i, (scene, title)) in scenes.iter().enumerate() {
        let so_far = p.chapter(n).map(|c| c.anime.cuts.len()).unwrap_or(0);
        status(format!("Storyboarding scene {} of {}: {title} ({so_far} cuts so far)", i + 1, scenes.len()));
        board_one(ask, &mut p, n, *scene, |p| board::storyboard_scene(p, n, *scene, note)).await.map_err(|e| if stopped(&e) { e } else { anyhow!("scene {scene} could not be boarded: {e:#}") })?;
    }
    let mut p = revise(ask, p, n, None, status).await?;
    prune_plates(&mut p);
    Ok(p)
}

/// Drops the plates no cut of any chapter uses and nobody has painted (a board drawn again
/// leaves the old board's plates behind otherwise).
pub fn prune_plates(p: &mut Project) {
    let used: std::collections::HashSet<String> = p.chapters.iter().flat_map(|c| c.anime.cuts.iter().map(|x| x.plate.clone())).collect();
    p.plates.retain(|pl| pl.image.is_some() || used.contains(&pl.id));
}

/// Draws up one scene again (with the user's note), then checks and revises that scene.
pub async fn board_scene(ask: &Ask, mut p: Project, n: u32, scene: u32, note: &str, status: &Status) -> Result<Project> {
    let title = p.chapter(n).and_then(|c| c.anime.scene(scene)).map(|s| s.title.clone()).ok_or_else(|| anyhow!("chapter {n} has no scene {scene}"))?;
    status(format!("Storyboarding scene {scene} again: {title}"));
    board_one(ask, &mut p, n, scene, |p| board::storyboard_scene(p, n, scene, note)).await?;
    revise(ask, p, n, Some(scene), status).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anime::tests::proj;
    use crate::project::{Line, PanelChar};
    use std::sync::Mutex;

    fn quiet() -> Status {
        Arc::new(|_| {})
    }

    /// An AI that answers each kind of question from a script, and notes what it was asked.
    fn scripted(log: Arc<Mutex<Vec<String>>>, answer: impl Fn(&str, &Query, usize) -> Result<Value> + Send + Sync + 'static) -> Ask {
        Arc::new(move |q: Query| {
            let kind = if q.system.contains("assistant director") {
                "read"
            } else if q.system.contains("character designer") {
                "cast"
            } else if q.system.contains("series-composition") {
                "scenes"
            } else if q.user.contains("THE EPISODE DIRECTOR CHECKED IT") {
                "revise"
            } else {
                "board"
            };
            let asked = {
                let mut l = log.lock().unwrap();
                l.push(kind.to_string());
                l.iter().filter(|k| *k == kind).count()
            };
            let a = answer(kind, &q, asked);
            Box::pin(async move { a })
        })
    }

    fn two_shot(l: &str, r: &str) -> Value {
        serde_json::json!([{ "name": l, "side": "left", "facing": "right" }, { "name": r, "side": "right", "facing": "left" }])
    }

    #[tokio::test]
    async fn a_chapter_goes_from_pages_to_a_checked_board() {
        let mut p = proj();
        p.chapter_mut(1).anime.panels = (0..8).map(|i| SourcePanel { file: format!("source/ch001/p{i}.jpg"), page: 1, ..Default::default() }).collect();
        let log = Arc::new(Mutex::new(vec![]));
        let ask = scripted(log.clone(), |kind, q, asked| {
            Ok(match kind {
                // Eight panels in two batches of six and two; each is told what came before.
                "read" => {
                    assert_eq!(q.images.len(), if asked == 1 { 6 } else { 2 });
                    assert_eq!(q.user.contains("The panels just before these"), asked == 2);
                    serde_json::json!({ "panels": (0..q.images.len()).map(|i| serde_json::json!({ "index": i, "description": format!("panel of batch {asked}"), "characters": [{ "name": if i % 2 == 0 { "Jin Woo" } else { "Stranger" } }], "lines": [{ "speaker": "Jin Woo", "text": format!("Line {asked} {i} is here."), "kind": "speech" }] })).collect::<Vec<_>>() })
                }
                "cast" => serde_json::json!({ "summary": "They meet.", "cast": [{ "name": "Mira", "aliases": ["Stranger"] }], "locations": [{ "name": "Street", "look": "wet asphalt" }] }),
                "scenes" => serde_json::json!({ "treatment": "Two scenes.", "scenes": [{ "title": "Meeting", "kind": "dialogue", "location": "Street", "grade": "daylight", "from": 1, "to": 5 }, { "title": "Parting", "kind": "dialogue", "location": "Street", "grade": "daylight", "from": 6, "to": 8 }] }),
                // The first scene comes back with a jump cut; the second is fine.
                "board" if q.user.contains("\"Meeting\"") => serde_json::json!({ "plates": [{ "id": "street", "view": "wide" }], "cuts": [
                    { "recipe": "master-two-shot", "from": [1, 2], "characters": two_shot("Jin Woo", "Mira"), "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 0 is here." }, { "speaker": "Jin Woo", "text": "Line 1 1 is here." }] },
                    { "recipe": "clean-single", "seconds": 4, "from": [3], "characters": [{ "name": "Mira", "side": "right", "facing": "left" }], "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 2 is here." }] },
                    { "recipe": "clean-single", "seconds": 4, "from": [4, 5], "characters": [{ "name": "Mira", "side": "right", "facing": "left" }], "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 3 is here." }, { "speaker": "Jin Woo", "text": "Line 1 4 is here." }] }
                ]}),
                "board" => serde_json::json!({ "cuts": [
                    { "recipe": "master-two-shot", "from": [6, 7], "characters": two_shot("Jin Woo", "Mira"), "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 5 is here." }, { "speaker": "Jin Woo", "text": "Line 2 0 is here." }] },
                    { "recipe": "held-last-image", "seconds": 5, "from": [8], "characters": [{ "name": "Jin Woo", "facing": "camera" }], "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 2 1 is here.", "kind": "thought" }] }
                ]}),
                // The revision goes in on the listener instead of repeating the single.
                "revise" => {
                    assert!(q.user.contains("jump cut") && q.user.contains("\"Meeting\""));
                    serde_json::json!({ "cuts": [
                        { "recipe": "master-two-shot", "from": [1, 2], "characters": two_shot("Jin Woo", "Mira"), "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 0 is here." }, { "speaker": "Jin Woo", "text": "Line 1 1 is here." }] },
                        { "recipe": "clean-single", "seconds": 4, "from": [3], "characters": [{ "name": "Mira", "side": "right", "facing": "left" }], "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 2 is here." }] },
                        { "recipe": "ecu-eyes", "seconds": 4, "from": [4, 5], "characters": [{ "name": "Mira", "side": "right", "facing": "left" }], "plate": "street", "lines": [{ "speaker": "Jin Woo", "text": "Line 1 3 is here." }, { "speaker": "Jin Woo", "text": "Line 1 4 is here." }] }
                    ]})
                }
                _ => unreachable!(),
            })
        });
        let st = quiet();
        let panels = read(&ask, &p, 1, &st).await.unwrap();
        assert!(panels.iter().all(|x| x.read) && panels[7].description == "panel of batch 2");
        p.chapter_mut(1).anime.panels = panels;
        assert_eq!(board::unknown_people(&p, 1), vec!["Stranger"]);
        let (p, added) = cast(&ask, p, 1, "", &st).await.unwrap();
        assert_eq!(added, 0, "the stranger was Mira all along");
        assert!(board::unknown_people(&p, 1).is_empty() && p.chapter(1).unwrap().anime.panels[1].characters[0].name == "Mira");
        let p = scenes(&ask, p, 1, &st).await.unwrap();
        assert_eq!(p.chapter(1).unwrap().anime.scenes.iter().map(|s| (s.from, s.to, s.seconds)).collect::<Vec<_>>(), vec![(0, 5, 20.), (5, 8, 12.)]);
        let p = board(&ask, p, 1, "", &st).await.unwrap();
        let a = &p.chapter(1).unwrap().anime;
        assert_eq!(*log.lock().unwrap(), ["read", "read", "cast", "scenes", "board", "board", "revise"]);
        // The revision was better, so it is the board; nothing is left open.
        assert_eq!(a.cuts.iter().map(|c| (c.number, c.scene, c.recipe.as_str())).collect::<Vec<_>>(), vec![(1, 1, "master-two-shot"), (2, 1, "clean-single"), (3, 1, "ecu-eyes"), (4, 2, "master-two-shot"), (5, 2, "held-last-image")]);
        assert_eq!(a.issues, vec![], "{:#?}", a.issues);
        // The lines made their cuts long enough; an off-screen line moves no mouth.
        // (The wide master plays its lines over a drifting camera; the single is of the listener.)
        assert!(a.cuts[0].seconds >= 4.5 && !a.cuts[0].mouth && a.cuts[0].motion == "camera" && !a.cuts[1].mouth);
        assert_eq!(p.plates.len(), 1);
        // A plate left from an old board goes; a painted one stays.
        let mut q = p.clone();
        q.plates.push(crate::anime::Plate { id: "old".into(), ..Default::default() });
        q.plates.push(crate::anime::Plate { id: "painted".into(), image: Some("plates/x.png".into()), ..Default::default() });
        prune_plates(&mut q);
        assert_eq!(q.plates.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["street", "painted"]);
    }

    #[tokio::test]
    async fn a_worse_revision_is_thrown_away_and_stop_stops() {
        let mut p = proj();
        p.chapter_mut(1).anime.panels = vec![SourcePanel { read: true, description: "x".into(), characters: vec![PanelChar { name: "Mira".into(), ..Default::default() }], lines: vec![Line { speaker: "Mira".into(), text: "Go on then.".into(), kind: "speech".into() }], ..Default::default() }];
        board::apply_scenes(&mut p, 1, serde_json::json!({ "scenes": [{ "title": "One", "kind": "dialogue", "location": "Street", "grade": "daylight", "from": 1, "to": 1 }] })).unwrap();
        let log = Arc::new(Mutex::new(vec![]));
        let single = |facing: &str| serde_json::json!({ "recipe": "clean-single", "seconds": 4, "characters": [{ "name": "Mira", "facing": facing }], "plate": "street", "lines": [{ "speaker": "Mira", "text": "Go on then." }] });
        let ask = scripted(log.clone(), move |kind, _, _| {
            Ok(match kind {
                // Two identical singles: a jump cut.
                "board" => serde_json::json!({ "cuts": [single("left"), single("left")] }),
                // The revision is worse: three of them.
                _ => serde_json::json!({ "cuts": [single("left"), single("left"), single("left")] }),
            })
        });
        let done = board(&ask, p.clone(), 1, "", &quiet()).await.unwrap();
        let a = &done.chapter(1).unwrap().anime;
        assert_eq!(a.cuts.len(), 2, "the first board stays");
        assert_eq!(*log.lock().unwrap(), ["board", "revise", "revise"]);
        assert!(a.issues.iter().any(|i| i.code == "jump-cut"));
        // Stop ends the step at once; any other failure of a scene names it.
        let halt: Ask = Arc::new(|_| Box::pin(async { Err(anyhow!("stopped")) }));
        assert_eq!(board(&halt, p.clone(), 1, "", &quiet()).await.unwrap_err().to_string(), "stopped");
        let broken: Ask = Arc::new(|_| Box::pin(async { Ok(serde_json::json!({ "cuts": [] })) }));
        assert!(board(&broken, p.clone(), 1, "", &quiet()).await.unwrap_err().to_string().starts_with("scene 1 could not be boarded"));
        p.chapter_mut(1).anime.scenes.clear();
        assert!(board(&broken, p, 1, "", &quiet()).await.is_err());
    }
}
