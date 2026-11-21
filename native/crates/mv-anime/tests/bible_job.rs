#![cfg(feature = "bible")]
//! The Bible job end to end on the synthetic manga (design 6.2), with canned AI replies through
//! the `llm` test seam: the reply is validated into the bible (brain ids, distinct colours,
//! panels that exist, an overhead station, stations inside the plan), scenes get their place,
//! speakers get a voice row, and reference crops are cut from the generated pages. A second run
//! only adds and fills gaps. A reply that is not JSON fails cleanly and is kept for the user.
//!
//! This test binary points `MANGAVOX_HOME` at its own temp folder (one process, one data root).

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use mv_anime::backend::NoImages;
use mv_anime::bible::{run_job, EXTRA_BEST_PANELS, EXTRA_FORCES, EXTRA_VOICE};
use mv_anime::crops::usable;
use mv_anime::llm::{Ai, UNREADABLE};
use mv_anime::model::{Episode, EpisodeRef, PlaceKind, Role, Scene, Series, SourceRef, Story, TimeOfDay};
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx, Tier};
use mv_anime::Store;

fn manga() -> &'static common::Manga {
    static M: OnceLock<(tempfile::TempDir, common::Manga)> = OnceLock::new();
    &M.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        let m = common::build(dir.path());
        (dir, m)
    })
    .1
}

fn project(id: &str) -> Store {
    let m = manga();
    let st = Store::open(id);
    let series = Series {
        id: id.into(),
        manga_id: common::MANGA_ID.into(),
        manga_title: common::MANGA_TITLE.into(),
        episodes: vec![EpisodeRef { id: "ep01".into(), title: "Episode 1".into() }],
        ..Default::default()
    };
    st.save_series(&series).expect("series");
    st.save_episode(&Episode { id: "ep01".into(), title: "Episode 1".into(), chapters: m.picks(), ..Default::default() }).expect("episode");
    let night = Scene {
        id: "sc0001".into(),
        title: "On the roof".into(),
        time_of_day: TimeOfDay::Night,
        source: vec![SourceRef { chapter_id: "c1".into(), page: 0, panel: None }],
        ..Default::default()
    };
    let pinned = Scene { id: "sc0002".into(), title: "Somewhere".into(), locked: BTreeSet::from(["/place".to_string()]), ..Default::default() };
    st.save_story(&Story { episode: "ep01".into(), scenes: vec![night, pinned], ..Default::default() }).expect("story");
    st
}

async fn run(st: &Store, replies: &[&str]) -> anyhow::Result<()> {
    let ai = Ai::canned(replies.iter().map(|r| r.to_string()));
    let ctx = RunCtx { ai: Box::new(move |_: Tier| Some(ai.clone())), image: Arc::new(NoImages), python: Box::new(|| Err(anyhow::anyhow!("no voices in tests"))) };
    let job = Job { id: 7, project: st.id.clone(), kind: JobKind::Bible { episode: "ep01".into() }, ..Default::default() };
    let progress = |_: f32, _: &str| {};
    let cancel = mv_ai::CancellationToken::new();
    let cx = JobCtx { job: &job, store: st, run: &ctx, progress: &progress, cancel: &cancel };
    run_job(&cx, "ep01").await
}

const FIRST: &str = r##"Here is the bible.
{"characters":[
  {"brain_id":"kai","name":"Kai","role":"hero","age":"about 17","description":"A quiet hunter.",
   "look":{"hair":"short black hair","eyes":"dark grey eyes","build":"lean","outfit":"grey hoodie","signature_colour":"#C82828","keep":["scar over his LEFT eyebrow"],"height_m":1.75},
   "outfits":[{"name":"Hoodie","description":"grey hoodie, black jeans","chapters":["c1","c7"]}],
   "voice":{"gender":"male","age":"teen","type":"calm, low"},
   "best_panels":[{"chapter":"c1","page":0,"panel":0,"angle":"front","framing":"face"},{"chapter":"c1","page":42,"panel":0}]},
  {"brain_id":"","name":"Ren","role":"threat","look":{"signature_colour":"#C9292A"},"best_panels":[]}
 ],
 "places":[{"name":"Tower roof","kind":"exterior","description":"A flat concrete roof.","size_m":[20,14],
   "landmarks":[{"name":"Water tank","kind":"feature","x":6,"z":-5,"w":3,"d":3,"h":4},{"name":"Stair door","kind":"door","x":50,"z":0,"w":1,"d":0.3,"h":2.1}],
   "stations":[{"name":"Stair door, looking out","x":9,"z":0,"height":1.5,"yaw":270,"lens":24},{"name":"Helicopter","x":500,"z":500,"height":30,"yaw":0,"lens":24}],
   "states":[{"id":"dusk","name":"dusk","key_dir_deg":300,"kelvin":3200,"mood":"red and fading"}],
   "forces":{"wind_deg":270,"wind":"strong gusts","moving":["blowing dust"]},
   "scenes":["sc0001","sc0002","sc9999"],
   "panels":[{"chapter":"c1","page":0,"panel":1}]}],
 "props":[{"name":"Dagger","owner":"Kai","description":"A short black dagger.","held":true,"states":["sheathed","drawn"],"panels":[{"chapter":"c1","page":0,"panel":0}]}],
 "style":{"line":"Thin navy lines, two-tone cel shading.","negative":["text"],"palette_hint":["#203040","#101820","#F0E0C0"]}}"##;

const SECOND: &str = r##"{"characters":[
  {"brain_id":"kai","name":"Kai","role":"threat",
   "look":{"hair":"long white hair","eyes":"dark grey eyes","signature_colour":"#00FF00"},
   "outfits":[{"name":"Hoodie","description":"another hoodie"},{"name":"Armour","description":"black plate armour","chapters":["c2"]}]}],
 "places":[{"name":"tower roof","kind":"exterior","size_m":[20,14],
   "landmarks":[{"name":"Water tank","kind":"feature","x":6,"z":-5},{"name":"Antenna","kind":"feature","x":-8,"z":5,"h":6}],
   "stations":[{"name":"Stair door, looking out","x":9,"z":0,"height":1.5,"yaw":270,"lens":24},{"name":"Top","x":0,"z":0,"height":20,"yaw":0,"pitch":-90,"overhead":true}]}],
 "props":[],"style":{"line":"Something else entirely."}}"##;

#[tokio::test]
async fn the_bible_is_built_from_the_reply_and_a_second_run_only_fills_gaps() {
    let st = project("job_build");
    run(&st, &[FIRST]).await.expect("the bible job runs");
    let b = st.bible().expect("bible");

    // Characters: brain ids, distinct colours, valid panels, voices.
    let kai = b.character("kai").expect("Kai keyed by brain id");
    assert_eq!(kai.role, Role::Hero);
    assert_eq!(kai.look.signature_colour, "#C82828");
    assert_eq!(kai.outfits[0].chapters, vec!["c1".to_string()], "unknown chapter ids are dropped");
    let best = kai.extra.get(EXTRA_BEST_PANELS).and_then(|v| v.as_array()).expect("best panels");
    assert_eq!(best.len(), 1, "the panel this chapter does not have is dropped");
    assert!(kai.extra.get(EXTRA_VOICE).is_some());
    let ren = b.character("ren").expect("Ren found in the brain by name");
    assert_eq!(ren.brain_id.as_deref(), Some("ren"));
    assert_ne!(ren.look.signature_colour, "#C9292A", "a colour too close to Kai's is replaced");
    assert!(!ren.look.signature_colour.is_empty());

    // Crops: Kai's AI-picked panel the Read also lists is usable; Ren's are suggestions only.
    assert!(kai.crops.iter().any(|c| c.agreed == 2 && usable(c)), "{:?}", kai.crops);
    assert!(!ren.crops.is_empty() && ren.crops.iter().all(|c| !usable(c)), "{:?}", ren.crops);
    assert!(kai.crops.iter().chain(&ren.crops).all(|c| st.path(&c.file).is_file()));

    // The place: clamped, an overhead, the scene's time as a state, forces kept.
    let place = &b.places[0];
    assert_eq!((place.id.as_str(), place.kind), ("pl_tower_roof", PlaceKind::Exterior));
    assert!(place.stations.iter().filter(|s| s.overhead).count() == 1);
    assert!(!place.stations.iter().any(|s| s.name == "Helicopter"));
    assert!(place.stations.len() >= 5 && place.stations.len() <= 9);
    let (hw, hd) = (place.plan.w_m / 2., place.plan.d_m / 2.);
    assert!(place.stations.iter().all(|s| s.pos[0].abs() <= hw && s.pos[2].abs() <= hd));
    assert!(place.plan.landmarks.iter().all(|l| l.pos[0].abs() <= hw && l.pos[1].abs() <= hd));
    let states: Vec<&str> = place.states.iter().map(|s| s.id.as_str()).collect();
    assert!(states.contains(&"dusk") && states.contains(&"night"), "{states:?}");
    assert!(place.extra.get(EXTRA_FORCES).is_some());
    assert!(place.extra.get("scenes").is_none(), "scene links are not stored on the place");
    assert!(place.crops.iter().any(|c| c.agreed == 2), "the script places scene 1 on that page: a second signal");

    // The script: the unset scene gets the place, the user's pinned one does not.
    let story = st.story("ep01").expect("story");
    assert_eq!(story.scenes[0].place.as_deref(), Some("pl_tower_roof"));
    assert_eq!(story.scenes[1].place, None);

    // Props, style, voices.
    assert_eq!(b.props[0].owner.as_deref(), Some("kai"));
    assert!(b.props[0].held);
    assert_eq!(b.style.line, "Thin navy lines, two-tone cel shading.");
    assert!(b.style.negative.iter().any(|n| n == "speech bubbles"));
    assert_eq!(b.style.palettes[0].swatches.len(), 3);
    let voiced: BTreeSet<&str> = b.cast.voices.iter().map(|v| v.character.as_str()).collect();
    assert_eq!(voiced, BTreeSet::from(["kai", "ren"]));

    // Second run: only gaps and new things.
    let stations_before = place.stations.len();
    let kai_crops = kai.crops.clone();
    run(&st, &[SECOND]).await.expect("second run");
    let b = st.bible().expect("bible");
    let kai = b.character("kai").expect("kai");
    assert_eq!(kai.look.hair, "short black hair");
    assert_eq!(kai.role, Role::Hero);
    assert_eq!(kai.look.signature_colour, "#C82828", "a set colour stays");
    assert_eq!(kai.outfits.len(), 2, "the new outfit is added");
    assert_eq!(kai.outfits[0].description, "grey hoodie, black jeans");
    assert_eq!(kai.crops, kai_crops, "existing crops are not cut again");
    assert_eq!(b.places.len(), 1);
    let place = &b.places[0];
    assert_eq!(place.stations.iter().filter(|s| s.overhead).count(), 1, "the AI's overhead joins the one there");
    assert_eq!(place.stations.len(), stations_before);
    assert!(place.plan.landmarks.iter().any(|l| l.name == "Antenna"));
    assert_eq!(place.plan.landmarks.len(), 3);
    assert_eq!(b.style.line, "Thin navy lines, two-tone cel shading.");
    assert_eq!(b.characters.len(), 2);
}

#[tokio::test]
async fn an_unreadable_reply_fails_cleanly_and_is_kept() {
    let st = project("job_junk");
    let err = run(&st, &["I cannot help with that.", "Still no JSON here."]).await.expect_err("junk fails");
    assert_eq!(err.to_string(), UNREADABLE);
    let raw = st.work_dir().join("ai").join("7.txt");
    assert!(raw.is_file(), "the raw reply is kept in work/ai");
    assert!(st.bible().expect("bible").characters.is_empty(), "nothing was written");
}

#[tokio::test]
async fn an_empty_reply_is_an_error_not_an_empty_bible() {
    let st = project("job_empty");
    let err = run(&st, &[r#"{"characters":[],"places":[],"props":[]}"#]).await.expect_err("empty fails");
    assert!(err.to_string().contains("no characters, places or props"), "{err}");
}

#[test]
fn the_crop_upgrade_is_quiet_and_does_nothing_once_stamped() {
    let st = project("bible_upgrade");
    let (id, name, alias, _) = common::CHARACTERS[0];
    let c = mv_anime::model::Character { id: id.into(), brain_id: Some(id.into()), name: name.into(), aliases: vec![alias.into()], ..Default::default() };
    st.save_character(&c).expect("character");
    st.save_style(&mv_anime::model::Style::default()).expect("style");
    let rel = format!("bible/characters/{id}.json");
    assert!(mv_anime::bible::upgrade_crops(&st).expect("upgrade"), "an unstamped bible is scored again");
    let after = st.bible().expect("bible");
    assert!(!after.character(id).expect("kept").crops.is_empty(), "the crops were cut");
    assert!(st.history(&rel).is_empty(), "no earlier-version copy for a score-only upgrade");
    assert!(st.history("bible/style.json").is_empty());
    // Stamped now: nothing is read beyond the style, nothing is written.
    let mtime = |rel: &str| std::fs::metadata(st.path(rel)).and_then(|m| m.modified()).ok();
    let before = (mtime(&rel), mtime("bible/style.json"));
    assert!(!mv_anime::bible::upgrade_crops(&st).expect("again"));
    assert_eq!((mtime(&rel), mtime("bible/style.json")), before, "nothing rewritten");
}