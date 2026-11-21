//! FRAMES (design 6.6): reference slots, the prompt compiler, the picture channel and the frame
//! job, against a small project built in a temp folder (tiny generated PNGs, no repo media).
//!
//! This binary owns its process: the data root points at a temp folder for all its tests. Tests
//! run in parallel, so each uses its own project.
#![cfg(feature = "frames")]

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use mv_anime::backend::{ImageBackend, ImageError, NoImages};
use mv_anime::compile::{self, SECTIONS};
use mv_anime::frames::{self, current_request, outdated};
use mv_anime::hash::request_hash;
use mv_anime::queue::{self, Job, JobCtx, JobKind, RunCtx};
use mv_anime::slots::{self, jobs, Failure, MAX_IDENTITY, MAX_REFS, SOFT_CAP};
use mv_anime::*;

static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
static MANGA: OnceLock<common::Manga> = OnceLock::new();
static SEED: AtomicU32 = AtomicU32::new(1);

const EP: &str = "ep01";
const SCENE: &str = "sc0001";
const SHOT: &str = "sh0001";

fn home() -> &'static Path {
    HOME.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        dir
    })
    .path()
}

fn manga() -> &'static common::Manga {
    MANGA.get_or_init(|| common::build(home()))
}

/// A tiny PNG, different for every call.
fn png(seed: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 3, image::Rgb([(seed & 255) as u8, ((seed >> 8) & 255) as u8, ((seed >> 16) & 255) as u8 ^ 0x5a]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).expect("encode png");
    out
}

/// A new picture stored as a take; its id.
fn pic(store: &Store) -> String {
    let n = SEED.fetch_add(1, Ordering::SeqCst);
    store.add_take(&png(n), "png", TakeMeta { kind: TakeKind::Image, target: Target::Upload, ..Default::default() }).expect("add take").id
}

fn take_file(store: &Store, id: &str) -> String {
    store.take(id).expect("take").file
}

fn card(store: &Store, status: Status) -> Card {
    Card { status, selected: Some(pic(store)), ..Default::default() }
}

fn character(store: &Store, id: &str, name: &str, colour: &str, sheets: &[SheetKind]) -> Character {
    Character {
        id: id.into(),
        name: name.into(),
        look: Look {
            hair: "short black hair".into(),
            eyes: format!("{} eyes", if name == "Ren" { "amber" } else { "grey" }),
            default_outfit: if id == "kai" { "black hunter coat".into() } else { "grey school uniform".into() },
            signature_colour: colour.into(),
            keep: if id == "kai" { vec!["scar over the left eyebrow".into(), "silver earring on the right ear".into()] } else { vec![] },
            height_m: 1.75,
            ..Default::default()
        },
        sheets: sheets.iter().map(|k| Sheet { kind: k.clone(), card: card(store, Status::Approved) }).collect(),
        status: Status::Approved,
        ..Default::default()
    }
}

fn mark(id: &str, x: f32, facing: f32) -> Mark {
    Mark { character: id.into(), pos: [x, 0.], facing_deg: facing, emotion: Emotion { kind: EmotionKind::Neutral, strength: 1 }, ..Default::default() }
}

/// Kai (left, facing the camera, angry, katana in the right hand) and Ren (right, in profile
/// facing Kai), filmed from the door station with a 24 mm lens, at night.
fn two_shot() -> Shot {
    let mut kai = mark("kai", -1.5, 180.);
    kai.emotion = Emotion { kind: EmotionKind::Anger, strength: 3 };
    kai.action = "raises the katana toward Ren".into();
    kai.holding = vec![Hold { prop: "sword".into(), hand: Hand::Right }];
    kai.gaze = Some("char:ren".into());
    let mut ren = mark("ren", 1.5, 270.);
    ren.emotion = Emotion { kind: EmotionKind::Fear, strength: 2 };
    ren.action = "steps back, one hand raised".into();
    ren.gaze = Some("char:kai".into());
    Shot {
        id: SHOT.into(),
        tells: "Kai threatens Ren".into(),
        camera: Camera {
            size: ShotSize::Ms,
            angle: Angle::Eye,
            station: Some("st_door".into()),
            look_at: LookAt::Angles { yaw_deg: 0., pitch_deg: 0. },
            lens_mm: 24,
            depth: Depth::Shallow,
            focus: Focus { target: FocusTarget::Character("kai".into()), ..Default::default() },
            ..Default::default()
        },
        marks: vec![kai, ren],
        lines: vec!["ln0001".into()],
        mouth: MouthPolicy::Closed,
        background: BackgroundKind::Matched,
        planned_s: 3.,
        ..Default::default()
    }
}

fn place(store: &Store) -> Place {
    let station = |id: &str, z: f32, yaw: f32| Station { id: id.into(), name: id.trim_start_matches("st_").replace('_', " "), pos: [0., 1.6, z], yaw_deg: yaw, lens_mm: 24, ..Default::default() };
    let plate = |st: &str, state: &str, status: Status| Plate { station: st.into(), state: state.into(), card: card(store, status) };
    Place {
        id: "pl_roof".into(),
        name: "the school rooftop".into(),
        kind: PlaceKind::Exterior,
        plan: FloorPlan {
            w_m: 10.,
            d_m: 12.,
            landmarks: vec![
                Landmark { id: "lm_tank".into(), name: "water tank".into(), kind: LandmarkKind::Feature, pos: [-2.5, -3.], size: [1.5, 1.5], h_m: 2., ..Default::default() },
                Landmark { id: "lm_rail".into(), name: "railing".into(), kind: LandmarkKind::Wall, pos: [2.5, -4.5], size: [3., 0.2], h_m: 1.1, ..Default::default() },
            ],
            ..Default::default()
        },
        stations: vec![station("st_door", 4., 0.), station("st_rail", -4., 180.)],
        states: vec![
            PlaceState { id: "day".into(), name: "day".into(), key_dir_deg: 90., kelvin: 5600, mood: String::new() },
            PlaceState { id: "night".into(), name: "night".into(), key_dir_deg: 270., kelvin: 9000, mood: String::new() },
        ],
        sheet: card(store, Status::Approved),
        plates: vec![
            plate("st_door", "night", Status::Approved),
            plate("st_rail", "night", Status::Approved),
            plate("st_door", "day", Status::Draft),
            plate("st_rail", "day", Status::Approved),
        ],
        status: Status::Approved,
        ..Default::default()
    }
}

/// A full project: bible, story, one scene with the two-shot.
fn setup(id: &str) -> Store {
    home();
    let store = Store::open(id);
    let style = Style {
        line: "Crisp TV-anime key frame: thin dark-brown lines, two-tone hard cel shading, painted backgrounds".into(),
        negative: vec!["3D render".into(), "photorealism".into()],
        frame: card(&store, Status::Approved),
        palettes: vec![Palette {
            id: "p_night".into(),
            name: "night threat".into(),
            swatches: vec![
                Swatch { role: "dominant".into(), hex: "#1E2C4F".into() },
                Swatch { role: "light".into(), hex: "#9FB8E0".into() },
                Swatch { role: "shadow".into(), hex: "#121A33".into() },
                Swatch { role: "accent".into(), hex: "#E03030".into() },
            ],
            brightness: 0.3,
            saturation: 0.4,
        }],
        ..Default::default()
    };
    store.save_style(&style).expect("style");
    let all = [SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::ColourModel, SheetKind::Back, SheetKind::Grip("sword".into())];
    store.save_character(&character(&store, "kai", "Kai", "#B22222", &all)).expect("kai");
    store.save_character(&character(&store, "ren", "Ren", "#2846C8", &[SheetKind::Face, SheetKind::Turnaround, SheetKind::ColourModel])).expect("ren");
    for (cid, name) in [("ama", "Ama"), ("c4", "Dae"), ("c5", "Mio"), ("c6", "Jun")] {
        store.save_character(&character(&store, cid, name, "#28A03C", &[SheetKind::Face, SheetKind::Turnaround])).expect("cast");
    }
    store.save_place(&place(&store)).expect("place");
    let prop = Prop { id: "sword".into(), name: "katana".into(), owner: Some("kai".into()), held: true, sheets: vec![Sheet { kind: SheetKind::Master, card: card(&store, Status::Approved) }], ..Default::default() };
    store.save_prop(&prop).expect("prop");
    let series = Series {
        id: id.into(),
        manga_id: common::MANGA_ID.into(),
        manga_title: common::MANGA_TITLE.into(),
        episodes: vec![EpisodeRef { id: EP.into(), title: "Episode 1".into() }],
        ..Default::default()
    };
    store.save_series(&series).expect("series");
    let story = Story {
        episode: EP.into(),
        scenes: vec![Scene {
            id: SCENE.into(),
            title: "Rooftop".into(),
            place: Some("pl_roof".into()),
            time_of_day: TimeOfDay::Night,
            colour: ColourTheme { name: "deep blue threat".into(), palette: Some("p_night".into()) },
            ..Default::default()
        }],
        lines: vec![Line { id: "ln0001".into(), scene: SCENE.into(), speaker: Some("kai".into()), spoken: "Stay back.".into(), ..Default::default() }],
        ..Default::default()
    };
    store.save_story(&story).expect("story");
    save_shots(&store, vec![two_shot()]);
    store
}

fn save_shots(store: &Store, shots: Vec<Shot>) {
    store.save_scene_shots(&SceneShots { episode: EP.into(), scene: SCENE.into(), shots, ..Default::default() }).expect("shots");
}

fn load_shot(store: &Store) -> Shot {
    store.scene_shots(EP, SCENE).expect("shots").shots.into_iter().find(|s| s.id == SHOT).expect("shot")
}

fn edit_shot(store: &Store, f: impl FnOnce(&mut Shot)) {
    let mut s = store.scene_shots(EP, SCENE).expect("shots");
    if let Some(shot) = s.shots.iter_mut().find(|s| s.id == SHOT) {
        f(shot);
    }
    store.save_scene_shots(&s).expect("save shots");
}

fn scene(store: &Store) -> Scene {
    store.story(EP).expect("story").scenes.into_iter().next().expect("scene")
}

fn roles(slots: &[RefSlot]) -> Vec<RefRole> {
    slots.iter().map(|r| r.role.clone()).collect()
}

fn identities(slots: &[RefSlot]) -> Vec<String> {
    slots.iter().filter_map(|r| if let RefRole::Identity(id) = &r.role { Some(id.clone()) } else { None }).collect()
}

fn sheet_file(store: &Store, c: &Character, kind: &SheetKind) -> String {
    let id = c.sheets.iter().find(|s| &s.kind == kind).and_then(|s| s.card.selected.clone()).expect("sheet");
    take_file(store, &id)
}

// ---------------------------------------------------------------- slots

#[test]
fn slots_follow_the_priority_order_and_stop_at_the_soft_cap() {
    let store = setup("slots-order");
    let bible = store.bible().expect("bible");
    let s = slots::pick(&two_shot(), &scene(&store), &bible, &store);
    use RefRole::*;
    assert_eq!(
        roles(&s),
        vec![Plate, Identity("kai".into()), Identity("ren".into()), Style, Expression("kai".into()), Grip("sword".into()), ColourModel("kai".into()), ColourModel("ren".into())]
    );
    assert_eq!(s.len(), SOFT_CAP);
    // Every reference states a job and carries its content hash; identities go to the Character slot.
    assert!(s.iter().all(|r| !r.job.trim().is_empty() && r.hash.len() == 64 && store.path(&r.file).is_file()));
    assert_eq!(s[0].job, jobs::PLATE);
    assert_eq!(s[1].slot, SiteSlot::Character);
    assert_eq!(s[3].slot, SiteSlot::Style);
    assert_eq!(s[1].job, "Kai's body and outfit");

    // A wide shot would add a second place view, but the soft cap is already reached.
    let mut wide = two_shot();
    wide.camera.size = ShotSize::Ls;
    let s = slots::pick(&wide, &scene(&store), &bible, &store);
    assert_eq!(s.len(), SOFT_CAP);
    assert!(!roles(&s).contains(&PlaceView));
    // In daylight there are no colour models, so the second view fits.
    let mut day = scene(&store);
    day.time_of_day = TimeOfDay::Day;
    let s = slots::pick(&wide, &day, &bible, &store);
    assert!(roles(&s).contains(&PlaceView), "{:?}", roles(&s));
}

#[test]
fn identity_picture_follows_shot_size_and_facing() {
    let store = setup("slots-identity");
    let bible = store.bible().expect("bible");
    let kai = bible.character("kai").expect("kai").clone();
    let file_of = |shot: &Shot| slots::pick(shot, &scene(&store), &bible, &store).into_iter().find(|r| r.role == RefRole::Identity("kai".into())).map(|r| (r.file, r.job)).expect("kai slot");

    let mut close = two_shot();
    close.camera.size = ShotSize::Cu;
    assert_eq!(file_of(&close), (sheet_file(&store, &kai, &SheetKind::Face), "Kai's face and hair only".to_string()));

    let mut back = two_shot();
    back.marks[0].visible = Visibility::Back;
    assert_eq!(file_of(&back), (sheet_file(&store, &kai, &SheetKind::Back), "Kai seen from behind".to_string()));

    assert_eq!(file_of(&two_shot()).0, sheet_file(&store, &kai, &SheetKind::Turnaround));
}

#[test]
fn caps_hold_fourteen_references_and_four_identities() {
    let store = setup("slots-caps");
    let bible = store.bible().expect("bible");
    // 20 pinned user references: the plate first, then pins, never more than 14.
    let mut shot = two_shot();
    shot.frame.gen.refs = (0..20).map(|i| RefSlot { file: take_file(&store, &pic(&store)), role: RefRole::User, job: format!("extra {i}"), pinned: true, ..Default::default() }).collect();
    let s = slots::pick(&shot, &scene(&store), &bible, &store);
    assert_eq!(s.len(), MAX_REFS);
    assert_eq!(s[0].role, RefRole::Plate);
    assert!(s[1..].iter().all(|r| r.pinned && r.role == RefRole::User));

    // Four characters on screen plus a pinned identity of a fifth: still four identities.
    let mut shot = two_shot();
    shot.marks = vec![mark("kai", -1.5, 180.), mark("ren", -0.5, 180.), mark("ama", 0.5, 180.), mark("c4", 1.5, 180.)];
    let c5 = bible.character("c5").expect("c5").clone();
    shot.frame.gen.refs = vec![RefSlot { file: sheet_file(&store, &c5, &SheetKind::Turnaround), role: RefRole::Identity("c5".into()), job: "Mio".into(), pinned: true, ..Default::default() }];
    let s = slots::pick(&shot, &scene(&store), &bible, &store);
    assert_eq!(identities(&s).len(), MAX_IDENTITY);
    assert_eq!(identities(&s), vec!["c5", "kai", "ren", "ama"]);
    assert!(!slots::is_crowded(&shot));
}

#[test]
fn crowded_shots_keep_two_identities_nearest_the_centre() {
    let store = setup("slots-crowded");
    let bible = store.bible().expect("bible");
    let mut shot = two_shot();
    // Screen x = 0.5 + x/6 with a 24 mm lens 4 m away: 0.0, 0.2, 0.4, 0.57, 0.77, 0.97.
    shot.marks = ["kai", "ren", "ama", "c4", "c5", "c6"].iter().enumerate().map(|(i, id)| mark(id, -3. + 1.2 * i as f32, 180.)).collect();
    let sc = scene(&store);
    let s = slots::pick(&shot, &sc, &bible, &store);
    assert_eq!(identities(&s), vec!["ama", "c4"]);
    let issue = slots::crowded_issue(&shot, &sc, &bible).expect("crowded issue");
    assert_eq!(issue.severity, Severity::Info);
    assert_eq!(issue.code, slots::CROWDED_CODE);
    assert!(issue.message.starts_with("Crowded: second pass."), "{}", issue.message);
    assert!(issue.message.contains("Ama and Dae"), "{}", issue.message);
    assert!(slots::crowded_issue(&two_shot(), &sc, &bible).is_none());
    let place = bible.place("pl_roof");
    let compiled = compile::frame_prompt(&shot, &sc, &bible, &geometry::derive(&shot, place), &s);
    assert!(compiled.text.contains("Only Ama and Dae have reference pictures"), "{}", compiled.text);
}

#[test]
fn the_three_background_kinds() {
    let store = setup("slots-background");
    let mut bible = store.bible().expect("bible");
    let sc = scene(&store);
    let place = bible.place("pl_roof").expect("place").clone();
    let plate_door_night = take_file(&store, place.plates[0].card.selected.as_deref().expect("plate"));
    let master = take_file(&store, place.sheet.selected.as_deref().expect("master"));

    // Matched room: the station's approved plate owns slot 1.
    let s = slots::pick(&two_shot(), &sc, &bible, &store);
    assert_eq!((s[0].role.clone(), s[0].file.clone(), s[0].job.as_str()), (RefRole::Plate, plate_door_night.clone(), jobs::PLATE));
    // The other station's plate when the camera stands there.
    let mut rev = two_shot();
    rev.camera.station = Some("st_rail".into());
    rev.camera.look_at = LookAt::Angles { yaw_deg: 180., pitch_deg: 0. };
    let s = slots::pick(&rev, &sc, &bible, &store);
    assert_eq!(s[0].file, take_file(&store, place.plates[1].card.selected.as_deref().expect("plate")));
    // No approved plate for the station: the environment master.
    if let Some(p) = bible.places.iter_mut().find(|p| p.id == "pl_roof") {
        p.plates.iter_mut().for_each(|pl| pl.card.status = Status::Draft);
    }
    let s = slots::pick(&two_shot(), &sc, &bible, &store);
    assert_eq!((s[0].role.clone(), s[0].file.clone(), s[0].job.as_str()), (RefRole::Plate, master.clone(), jobs::PLACE_MASTER));
    let bible = store.bible().expect("bible");

    // Simple backdrop: the master for colours only, never a plate.
    let mut backdrop = two_shot();
    backdrop.background = BackgroundKind::Backdrop;
    let s = slots::pick(&backdrop, &sc, &bible, &store);
    assert_eq!((s[0].role.clone(), s[0].file.clone(), s[0].job.as_str()), (RefRole::PlaceView, master.clone(), jobs::BACKDROP));
    assert!(!s.iter().any(|r| r.role == RefRole::Plate || place.plates.iter().any(|p| p.card.selected.as_deref() == Some(slots_stem(&r.file)))));
    let text = compile::frame_prompt(&backdrop, &sc, &bible, &geometry::derive(&backdrop, Some(&place)), &s).text;
    assert!(text.contains("simple backdrop in the school rooftop's colours"), "{text}");
    assert!(text.contains("Image 1 is the school rooftop: use only its colours and materials"), "{text}");
    assert!(!text.contains("Behind Kai"), "a backdrop has no room behind anyone");

    // No background: no place picture at all.
    let mut none = two_shot();
    none.background = BackgroundKind::None;
    let s = slots::pick(&none, &sc, &bible, &store);
    assert!(!s.iter().any(|r| matches!(r.role, RefRole::Plate | RefRole::PlaceView)));
    assert_eq!(s[0].role, RefRole::Identity("kai".into()));
    let text = compile::frame_prompt(&none, &sc, &bible, &geometry::derive(&none, Some(&place)), &s).text;
    assert!(text.contains("Background: no room"), "{text}");
}

fn slots_stem(file: &str) -> &str {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.split('.').next().unwrap_or(name)
}

#[test]
fn the_source_panel_is_the_layout_guide() {
    let m = manga();
    let store = setup("slots-panel");
    let episode = Episode { id: EP.into(), title: "Episode 1".into(), chapters: m.picks(), ..Default::default() };
    store.save_episode(&episode).expect("episode");
    let bible = store.bible().expect("bible");
    let mut day = scene(&store);
    day.time_of_day = TimeOfDay::Day;
    let mut shot = two_shot();
    shot.marks.truncate(1);
    shot.marks[0].holding.clear();
    shot.marks[0].emotion.strength = 1;
    shot.source = vec![SourceRef { chapter_id: common::chapter_id(0), page: 0, panel: Some(1) }];
    let s = slots::pick(&shot, &day, &bible, &store);
    assert_eq!(roles(&s), vec![RefRole::Plate, RefRole::Identity("kai".into()), RefRole::Style, RefRole::Panel]);
    let panel = &s[3];
    assert_eq!(panel.job, jobs::PANEL);
    let take = store.take(slots_stem(&panel.file)).expect("the panel is kept as a take");
    assert!(matches!(take.source, TakeSource::MangaCrop(ref src) if src.panel == Some(1)));
    // Picking again reuses the same take.
    assert_eq!(slots::pick(&shot, &day, &bible, &store)[3].file, panel.file);
    let text = compile::frame_prompt(&shot, &day, &bible, &geometry::derive(&shot, bible.place("pl_roof")), &s).text;
    assert!(text.contains("Image 4 is the manga panel this shot adapts: use it only for the pose and the composition"), "{text}");
}

#[test]
fn swaps_after_a_failed_check() {
    let store = setup("slots-swap");
    let mut bible = store.bible().expect("bible");
    let sc = scene(&store);
    let shot = two_shot();
    let s = slots::pick(&shot, &sc, &bible, &store);

    let styled = slots::swap(&s, &Failure::Style, &shot, &sc, &bible, &store);
    assert_eq!(styled[0].role, RefRole::Style);
    assert_eq!(styled.len(), s.len());
    let text = compile::frame_prompt(&shot, &sc, &bible, &geometry::derive(&shot, bible.place("pl_roof")), &styled).text;
    assert!(text.ends_with("Once more: in the exact style of Image 1."), "{text}");

    let prop = slots::swap(&s, &Failure::Prop("sword".into()), &shot, &sc, &bible, &store);
    let g = prop.iter().position(|r| r.role == RefRole::Grip("sword".into())).expect("grip");
    assert_eq!(prop[g + 1].role, RefRole::Prop("sword".into()));

    let place = slots::swap(&s, &Failure::Place, &shot, &sc, &bible, &store);
    assert!(place.iter().any(|r| r.role == RefRole::PlaceView));
    assert!(place.len() <= MAX_REFS);

    // An identity failure puts a bigger picture of the failing view (an approved full crop) in Kai's slot.
    let crop_file = take_file(&store, &pic(&store));
    if let Some(k) = bible.characters.iter_mut().find(|c| c.id == "kai") {
        k.crops.push(RefCrop { file: crop_file.clone(), framing: CropFraming::Full, approved: true, score: 0.9, ..Default::default() });
    }
    let fixed = slots::swap(&s, &Failure::Identity("kai".into()), &shot, &sc, &bible, &store);
    let i = fixed.iter().position(|r| r.role == RefRole::Identity("kai".into())).expect("kai");
    assert_eq!(i, 1);
    assert_eq!(fixed[i].file, crop_file);
    assert_eq!(identities(&fixed).len(), 2);
}

// ---------------------------------------------------------------- the prompt

#[test]
fn every_section_is_present_in_order_and_the_prompt_is_stable() {
    let store = setup("compile-sections");
    let p = frames::prepare(&store, EP, SHOT).expect("prepare");
    let names: Vec<&str> = p.compiled.sections.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, SECTIONS.to_vec());
    let mut at = 0;
    for (name, para) in &p.compiled.sections {
        assert!(!para.trim().is_empty(), "{name} is empty");
        let i = p.compiled.text[at..].find(para.as_str()).unwrap_or_else(|| panic!("{name} missing or out of order")) + at;
        at = i + para.len();
    }
    let text = &p.compiled.text;
    if let Ok(path) = std::env::var("MV_FRAMES_EXAMPLE") {
        let _ = std::fs::write(path, text);
    }
    for want in [
        "Crisp TV-anime key frame: thin dark-brown lines, two-tone hard cel shading, painted backgrounds. In the exact style of Image 4.",
        "Image 1 is the background plate of the school rooftop from the camera position \"door\" in night light: keep the room, the layout and the camera exactly.",
        "Image 2 is Kai's turnaround sheet: use it only for Kai's body, hair and outfit, not for the pose.",
        "Image 6 is how Kai holds the katana: use it only for the grip and the hand.",
        "Medium shot: Kai from the waist up.",
        "Wide 24 mm lens: camera close, space stretched",
        "The horizon crosses the characters' eyes at 50% from the top",
        "Shallow depth of field: focus on Kai's eyes, background softly out of focus.",
        "Kai (black hunter coat, red signature colour) on the left third of the frame, facing the camera, standing, raises the katana toward Ren, looking at Ren, expression: furious",
        "Ren (grey school uniform, blue signature colour) on the right third of the frame, in profile, facing screen left",
        "Keep Ren's short black hair and amber eyes.",
        "Behind Kai: the water tank.",
        "Behind Ren: the railing.",
        "Mouths closed.",
        "Kai holds the katana in the right hand (on screen left), fingers wrapped around it.",
        "Colour key: deep blue threat.",
        "a single #E03030 (red) accent",
        "Key light: cold blue (9000 K), from screen left.",
        "Keep Kai's scar over the left eyebrow and silver earring on the right ear.",
        "16:9 frame. No text, no speech bubbles, no captions, no extra characters, no watermark. Avoid 3D render, photorealism.",
    ] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
    // The same shot always compiles to the same prompt, also after a round trip through JSON.
    assert_eq!(frames::prepare(&store, EP, SHOT).expect("prepare").compiled, p.compiled);
    let shot: Shot = serde_json::from_str(&serde_json::to_string(&load_shot(&store)).expect("json")).expect("json");
    let again = frames::prepare_shot(&shot, &scene(&store), &store.bible().expect("bible"), &ProjectSettings::default(), &store);
    assert_eq!(again.compiled.text, p.compiled.text);
    assert_eq!(request_hash(&again.request), request_hash(&p.request));
}

#[test]
fn a_pinned_prompt_survives_a_recompile_and_reset_restores_it() {
    let store = setup("compile-pin");
    let before = frames::recompile(&store, EP, SHOT).expect("recompile");
    assert_eq!(current_request(&store, EP, SHOT).prompt, before.compiled.text);

    frames::set_prompt(&store, EP, SHOT, Some("Kai alone on the roof at night, my own words.")).expect("pin");
    assert_eq!(current_request(&store, EP, SHOT).prompt, "Kai alone on the roof at night, my own words.");

    // The shot changes: the compiled prompt moves on, the pinned one stays. (On a simple backdrop:
    // with its station's plate behind it, the plate's lens would be the one described.)
    edit_shot(&store, |s| {
        s.camera.lens_mm = 85;
        s.background = BackgroundKind::Backdrop;
    });
    let after = frames::recompile(&store, EP, SHOT).expect("recompile");
    assert_ne!(after.compiled.text, before.compiled.text);
    assert!(after.compiled.text.contains("Long 85 mm lens: camera far away"), "{}", after.compiled.text);
    let spec = load_shot(&store).frame.gen;
    assert_eq!(spec.prompt_user.as_deref(), Some("Kai alone on the roof at night, my own words."));
    assert_eq!(spec.prompt_ai, after.compiled.text);
    assert!(frames::prompt_moved(&spec));
    assert_eq!(current_request(&store, EP, SHOT).prompt, "Kai alone on the roof at night, my own words.");

    // Reset: the compiled prompt is sent again.
    frames::set_prompt(&store, EP, SHOT, None).expect("reset");
    assert!(load_shot(&store).frame.gen.prompt_user.is_none());
    assert_eq!(current_request(&store, EP, SHOT).prompt, after.compiled.text);
}

// ---------------------------------------------------------------- the picture channel

#[test]
fn the_request_hash_follows_reference_content_not_voices_or_timing() {
    let store = setup("hash-channel");
    let h0 = request_hash(&current_request(&store, EP, SHOT));
    assert!(!current_request(&store, EP, SHOT).prompt.is_empty());

    // Voices and timing: the same picture.
    let mut story = store.story(EP).expect("story");
    story.lines[0].voice.voice_override = Some("cb_storyteller_m".into());
    story.lines[0].voice.speed = 1.2;
    story.lines[0].spoken = "Stay back, I said.".into();
    store.save_story(&story).expect("story");
    store.save_cast(&VoiceCast { voices: vec![CharVoice { character: "kai".into(), voice_id: "kokoro_am_adam".into(), speed: 0.9, ..Default::default() }], ..Default::default() }).expect("cast");
    edit_shot(&store, |s| {
        s.planned_s = 7.5;
        s.locked_s = Some(6.25);
    });
    assert_eq!(request_hash(&current_request(&store, EP, SHOT)), h0);

    // A new picture on a referenced sheet: a different request.
    let mut kai = store.bible().expect("bible").character("kai").expect("kai").clone();
    if let Some(s) = kai.sheets.iter_mut().find(|s| s.kind == SheetKind::Turnaround) {
        s.card.selected = Some(pic(&store));
    }
    store.save_character(&kai).expect("kai");
    let h1 = request_hash(&current_request(&store, EP, SHOT));
    assert_ne!(h1, h0);

    // A reference file changed in place (a crop outside the take store): a different request.
    let crop = "bible/crops/ama-face.png";
    std::fs::create_dir_all(store.path("bible/crops")).expect("dir");
    std::fs::write(store.path(crop), png(900_001)).expect("crop");
    let mut ama = store.bible().expect("bible").character("ama").expect("ama").clone();
    ama.sheets.clear();
    ama.crops = vec![RefCrop { file: crop.into(), framing: CropFraming::Full, approved: true, score: 1., ..Default::default() }];
    store.save_character(&ama).expect("ama");
    edit_shot(&store, |s| s.marks.push(mark("ama", 0.2, 180.)));
    let h2 = request_hash(&current_request(&store, EP, SHOT));
    let mut bigger = png(900_002);
    bigger.extend_from_slice(&[0; 16]);
    std::fs::write(store.path(crop), bigger).expect("crop");
    assert_ne!(request_hash(&current_request(&store, EP, SHOT)), h2);
}

#[test]
fn unreviewed_sheets_are_never_references_and_the_panel_is_told() {
    let store = setup("slots-unapproved");
    let mut kai = store.bible().expect("bible").character("kai").expect("kai").clone();
    for s in kai.sheets.iter_mut().filter(|s| s.kind == SheetKind::Turnaround) {
        s.card.status = Status::NeedsReview;
    }
    let crop = take_file(&store, &pic(&store));
    kai.crops = vec![RefCrop { file: crop.clone(), framing: CropFraming::Full, approved: true, score: 1., ..Default::default() }];
    store.save_character(&kai).expect("kai");
    let p = frames::prepare(&store, EP, SHOT).expect("prepare");
    let kai_ref = p.spec.refs.iter().find(|r| r.role == RefRole::Identity("kai".into())).expect("kai has a picture");
    assert_eq!(kai_ref.file, crop, "the approved crop stands in for the turnaround waiting for review");
    assert!(p.left_out.iter().any(|n| n == "Kai's turnaround is not approved yet, so a manga picture of Kai stands in for it."), "{:?}", p.left_out);
}

#[test]
fn the_camera_words_follow_the_station_it_stands_on() {
    let store = setup("compile-station");
    let mut place = store.bible().expect("bible").place("pl_roof").expect("roof").clone();
    // The door station is 6 m up, looking down.
    if let Some(st) = place.stations.iter_mut().find(|s| s.id == "st_door") {
        st.pos[1] = 6.;
        st.pitch_deg = -30.;
    }
    store.save_place(&place).expect("place");
    edit_shot(&store, |s| {
        s.camera.lens_mm = 85;
        s.camera.angle = Angle::Eye;
        s.camera.look_at = LookAt::Angles { yaw_deg: 0., pitch_deg: -30. };
    });
    let text = frames::prepare(&store, EP, SHOT).expect("prepare").compiled.text;
    assert!(text.contains("High-angle shot"), "the height decides the angle words: {text}");
    assert!(!text.contains("Eye-level"), "{text}");
    assert!(text.contains("Wide 24 mm lens"), "the plate was painted with the station's lens: {text}");
    assert!(text.contains("Camera above eye level"), "{text}");
}

#[tokio::test]
async fn a_frame_drawn_from_a_sheet_goes_out_of_date_and_an_uploaded_one_does_not() {
    let store = setup("job-sheet-change");
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: usize::MAX });
    run(&store, fake, 1).await.expect("frame job");
    let drawn = load_shot(&store).frame;
    assert!(!outdated(&drawn, &store));
    // The user's own picture for the same shot, approved.
    let mine = store.add_take(&png(777_001), "png", TakeMeta { kind: TakeKind::Image, target: Target::Upload, source: TakeSource::Upload { name: "mine.png".into() }, ..Default::default() }).expect("upload").id;
    let uploaded = Card { status: Status::Approved, takes: vec![mine.clone()], selected: Some(mine), ..Default::default() };
    // The user replaces Kai's turnaround (the shot's identity sheet) and approves it.
    let mut kai = store.bible().expect("bible").character("kai").expect("kai").clone();
    for s in kai.sheets.iter_mut().filter(|s| s.kind == SheetKind::Turnaround) {
        s.card.selected = Some(pic(&store));
        s.card.status = Status::Approved;
    }
    store.save_character(&kai).expect("kai");
    assert!(outdated(&drawn, &store), "a frame the engine drew from the sheet is out of date");
    assert!(!outdated(&uploaded, &store), "a picture the user uploaded was never made from the sheet");
}

// ---------------------------------------------------------------- the frame job

/// Answers one new picture per call; fails from call `fail_from` on.
struct Fake {
    calls: AtomicUsize,
    fail_from: usize,
}

impl ImageBackend for Fake {
    fn generate<'a>(
        &'a self,
        _req: &'a GenRequest,
        files: &'a [(std::path::PathBuf, RefSlot)],
        _count: u8,
        on_status: Box<dyn Fn(&str) + Send + Sync + 'a>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let fail = n >= self.fail_from;
        Box::pin(async move {
            on_status("drawing");
            assert!(files.iter().all(|(p, _)| p.is_file()));
            if fail {
                Err(ImageError::Site("Magnific did not answer.".into()))
            } else {
                Ok(vec![png(500_000 + SEED.fetch_add(1, Ordering::SeqCst))])
            }
        })
    }
}

async fn run(store: &Store, backend: Arc<dyn ImageBackend>, variants: u8) -> anyhow::Result<()> {
    let job = Job { kind: JobKind::Frame { episode: EP.into(), shot: SHOT.into(), variants }, project: store.id.clone(), ..Default::default() };
    let run = RunCtx { ai: Box::new(|_| None), image: backend, python: Box::new(|| Err(anyhow::anyhow!("no python in tests"))) };
    let progress = |_: f32, _: &str| {};
    let cancel = mv_ai::CancellationToken::new();
    let cx = JobCtx { job: &job, store, run: &run, progress: &progress, cancel: &cancel };
    queue::execute(&cx).await
}

#[tokio::test]
async fn a_failed_backend_keeps_the_earlier_takes() {
    let store = setup("job-fail");
    let earlier = pic(&store);
    edit_shot(&store, |s| {
        s.frame.takes = vec![earlier.clone()];
        s.frame.selected = Some(earlier.clone());
        s.frame.status = Status::Approved;
    });
    let before = load_shot(&store).frame;
    let err = run(&store, Arc::new(NoImages), 2).await.expect_err("no pictures");
    assert!(!err.to_string().is_empty());
    assert_eq!(load_shot(&store).frame, before);

    // A failure on the second picture keeps the first one too.
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: 1 });
    let err = run(&store, fake, 2).await.expect_err("second picture fails");
    assert!(err.chain().any(|c| c.downcast_ref::<ImageError>().is_some()), "the queue must see the picture error: {err:#}");
    let frame = load_shot(&store).frame;
    assert_eq!(frame.takes.len(), 2);
    assert_eq!(frame.takes[0], earlier);
    assert_eq!(frame.selected.as_deref(), Some(earlier.as_str()));
}

#[tokio::test]
async fn variants_become_takes_and_outdated_follows_the_request() {
    let store = setup("job-ok");
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: usize::MAX });
    run(&store, fake.clone(), 2).await.expect("frame job");
    assert_eq!(fake.calls.load(Ordering::SeqCst), 2, "one submission per variant");
    let frame = load_shot(&store).frame;
    assert_eq!(frame.takes.len(), 2);
    assert_eq!(frame.selected.as_ref(), frame.takes.first());
    assert_eq!(frame.status, Status::NeedsReview);
    assert!(!frame.gen.prompt_ai.is_empty());
    assert_eq!(frame.gen.refs.len(), SOFT_CAP);
    let take = store.take(&frame.takes[0]).expect("take");
    assert_eq!(take.request_hash, request_hash(&current_request(&store, EP, SHOT)));
    assert_eq!(take.target, Target::ShotFrame { episode: EP.into(), shot: SHOT.into() });
    assert_eq!(take.batch, store.take(&frame.takes[1]).expect("take").batch);
    assert!(!outdated(&frame, &store));

    // A voice change does not outdate it; a new style frame does; Keep (locked) hides it.
    let mut cast = store.bible().expect("bible").cast;
    cast.narrator.voice_id = "kokoro_bf_emma".into();
    store.save_cast(&cast).expect("cast");
    assert!(!outdated(&frame, &store));
    let mut style = store.bible().expect("bible").style;
    style.frame.selected = Some(pic(&store));
    store.save_style(&style).expect("style");
    assert!(outdated(&frame, &store));
    let kept = Card { locked: true, ..frame.clone() };
    assert!(!outdated(&kept, &store));

    // A second run adds takes and keeps the selection; approve and restore.
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: usize::MAX });
    run(&store, fake, 1).await.expect("frame job");
    let frame2 = load_shot(&store).frame;
    assert_eq!(frame2.takes.len(), 3);
    assert_eq!(frame2.selected, frame.selected);
    assert!(outdated(&frame2, &store), "the selected picture was drawn before the new style frame");
    frames::approve(&store, EP, SHOT).expect("approve");
    let approved = load_shot(&store).frame;
    assert_eq!(approved.status, Status::Approved);
    assert!(!outdated(&approved, &store), "approving accepts the picture as it is");
    // More variants to compare keep the approval of the selected picture.
    let fake = Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: usize::MAX });
    run(&store, fake, 1).await.expect("frame job");
    assert_eq!(load_shot(&store).frame.status, Status::Approved);
    assert_eq!(load_shot(&store).frame.selected, frame.selected);
    // A later change outdates it again.
    let mut style = store.bible().expect("bible").style;
    style.frame.selected = Some(pic(&store));
    store.save_style(&style).expect("style");
    assert!(outdated(&load_shot(&store).frame, &store));
    frames::select_take(&store, EP, SHOT, &frame2.takes[2]).expect("select");
    assert_eq!(load_shot(&store).frame.selected.as_deref(), Some(frame2.takes[2].as_str()));
    assert!(frames::select_take(&store, EP, SHOT, "not-a-take").is_err());
}

#[test]
fn a_missing_reference_stops_the_job_with_what_to_do() {
    let store = setup("job-missing");
    let p = frames::prepare(&store, EP, SHOT).expect("prepare");
    let style_file = p.spec.refs.iter().find(|r| r.role == RefRole::Style).expect("style").file.clone();
    edit_shot(&store, |s| {
        s.frame.gen.refs = p.spec.refs.clone();
        s.frame.gen.refs_user = true;
    });
    std::fs::remove_file(store.path(&style_file)).expect("remove");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let err = rt.block_on(run(&store, Arc::new(Fake { calls: AtomicUsize::new(0), fail_from: usize::MAX }), 1)).expect_err("missing file");
    assert!(err.to_string().contains("A reference picture of this shot is missing (line, shading and colour style only)"), "{err}");
}

#[test]
fn frame_jobs_are_queued_with_shot_numbers() {
    let store = setup("job-enqueue");
    frames::enqueue_frames(&store, EP, &[SHOT.to_string(), " ".to_string()], 3);
    let q = queue::snapshot();
    let mine: Vec<&Job> = q.jobs.iter().filter(|j| j.project == store.id).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].kind, JobKind::Frame { episode: EP.into(), shot: SHOT.into(), variants: 3 });
    assert_eq!(mine[0].label, "Picture for shot 1.1");
}

/// Draws one picture; while it "draws", the user changes the shot (another model and size, a
/// hand-written note in the prompt) and saves it, as the Frames screen would.
struct UserEditsWhileDrawing {
    store: Store,
}

impl ImageBackend for UserEditsWhileDrawing {
    fn generate<'a>(
        &'a self,
        _req: &'a GenRequest,
        _files: &'a [(std::path::PathBuf, RefSlot)],
        _count: u8,
        _on_status: Box<dyn Fn(&str) + Send + Sync + 'a>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        Box::pin(async move {
            edit_shot(&self.store, |s| {
                s.frame.gen.model = "user-picked-model".into();
                s.frame.gen.res = "2K".into();
                s.frame.gen.prompt_user = Some("my own words".into());
            });
            Ok(vec![png(600_000 + SEED.fetch_add(1, Ordering::SeqCst))])
        })
    }
}

#[tokio::test]
async fn a_picture_arriving_keeps_the_settings_the_user_chose_while_it_was_drawing() {
    let store = setup("job-user-edit");
    let before = load_shot(&store).frame.gen;
    assert!(before.model.is_empty() && before.res.is_empty() && before.aspect.is_empty());
    run(&store, Arc::new(UserEditsWhileDrawing { store: store.clone() }), 1).await.expect("frame job");
    let frame = load_shot(&store).frame;
    assert_eq!(frame.takes.len(), 1, "the new picture is on the card");
    assert_eq!(frame.selected.as_ref(), frame.takes.first());
    assert_eq!(frame.status, Status::NeedsReview);
    // Chosen while drawing: kept.
    assert_eq!(frame.gen.model, "user-picked-model");
    assert_eq!(frame.gen.res, "2K");
    assert_eq!(frame.gen.prompt_user.as_deref(), Some("my own words"));
    // Untouched by the user: the job's values written back, as before.
    assert_eq!(frame.gen.aspect, frames::DEFAULT_ASPECT);
    assert!(!frame.gen.prompt_ai.is_empty());
    assert_eq!(frame.gen.refs.len(), SOFT_CAP);
}