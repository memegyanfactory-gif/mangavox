#![cfg(feature = "bible")]
//! Bible sheet specs and the Sheet job (design 6.2, 6.6): every prompt opens with the style line
//! word for word and follows the fixed order, every reference has a stated job, there are never
//! more than 4 identity pictures nor more than 14 references, places are described from their
//! camera stations, and the job stores each picture as a take on the right card.
//!
//! This test binary points `MANGAVOX_HOME` at its own temp folder (one process, one data root).

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use futures::future::BoxFuture;
use image::{Rgb, RgbImage};
use mv_anime::backend::{ImageBackend, ImageError, NoImages};
use mv_anime::hash::request_hash;
use mv_anime::model::{
    Card, Character, CropFraming, FloorPlan, GenRequest, GenSpec, Landmark, LandmarkKind, Look, Place, PlaceKind, PlaceState, Prop, RefCrop, RefRole, RefSlot, Role, Series, Sheet,
    SheetKind, SiteSlot, SourceRef, Station, Status, Style, Target, TakeMeta, ViewAngle,
};
use mv_anime::queue::{Job, JobCtx, JobKind, RunCtx, Tier};
use mv_anime::sheets::{character_pack, master_station, plate_spec, run_job, sheet_key, sheet_kind, spec, style_frame_spec, Subject, MAX_IDENTITY, MAX_REFS, SOFT_REFS};
use mv_anime::Store;

const LINE: &str = "Thin navy lines, two-tone hard cel shading, painted backgrounds without outlines.";

fn home() -> &'static Path {
    static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = common::temp_root();
        common::set_home(dir.path());
        std::env::set_var("MANGAVOX_LOG_DIR", dir.path().join("logs"));
        dir
    })
    .path()
}

fn png(seed: u8) -> Vec<u8> {
    png_wh(seed, 64, 48)
}

fn png_wh(seed: u8, w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| Rgb([seed, (x * 3) as u8, (y * 5) as u8]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

fn take(st: &Store, seed: u8) -> String {
    st.add_take(&png(seed), "png", TakeMeta::default()).expect("take").id
}

fn crop(st: &Store, seed: u8, framing: CropFraming, angle: ViewAngle, agreed: u8) -> RefCrop {
    let id = take(st, seed);
    let file = st.take(&id).expect("take").file;
    RefCrop { file, take: id, source: SourceRef { chapter_id: "c1".into(), page: seed as usize, panel: Some(0) }, angle, framing, score: 100. - seed as f32, approved: false, agreed }
}

fn approved(id: &str) -> Card {
    Card { status: Status::Approved, takes: vec![id.into()], selected: Some(id.into()), ..Default::default() }
}

/// A project with Kai (8 usable crops + 2 suggestions), a roof, a dagger and a picked style frame.
fn project(id: &str) -> (Store, Character, Place, Style) {
    home();
    let st = Store::open(id);
    st.save_series(&Series { id: id.into(), manga_id: "m1".into(), manga_title: "Test".into(), ..Default::default() }).expect("series");
    let angles = [ViewAngle::Front, ViewAngle::ThreeQuarter, ViewAngle::Side, ViewAngle::Back];
    let framings = [CropFraming::Face, CropFraming::Bust, CropFraming::Full];
    let mut crops: Vec<RefCrop> = (0..8).map(|i| crop(&st, i + 1, framings[i as usize % 3], angles[i as usize % 4], 2)).collect();
    crops.push(crop(&st, 30, CropFraming::Face, ViewAngle::Front, 1));
    crops.push(crop(&st, 31, CropFraming::Face, ViewAngle::Front, 1));
    let kai = Character {
        id: "kai".into(),
        name: "Kai".into(),
        role: Role::Hero,
        age: "about 17".into(),
        look: Look {
            hair: "short black hair".into(),
            eyes: "dark grey eyes".into(),
            build: "lean".into(),
            default_outfit: "grey hoodie".into(),
            signature_colour: "#C82828".into(),
            keep: vec!["scar over his LEFT eyebrow".into()],
            height_m: 1.75,
        },
        crops,
        ..Default::default()
    };
    st.save_character(&kai).expect("kai");
    let place = Place {
        id: "pl_roof".into(),
        name: "Tower roof".into(),
        kind: PlaceKind::Exterior,
        description: "A flat concrete roof.".into(),
        plan: FloorPlan {
            w_m: 20.,
            d_m: 14.,
            landmarks: vec![
                Landmark { id: "lm_tank".into(), name: "water tank".into(), kind: LandmarkKind::Feature, pos: [-5., -5.], size: [3., 3.], h_m: 4., ..Default::default() },
                Landmark { id: "lm_door".into(), name: "stair door".into(), kind: LandmarkKind::Door, pos: [5., -6.5], size: [1., 0.3], h_m: 2.1, ..Default::default() },
                Landmark { id: "lm_rail".into(), name: "railing".into(), kind: LandmarkKind::Wall, pos: [0., 6.8], size: [20., 0.2], h_m: 1.1, ..Default::default() },
            ],
            ..Default::default()
        },
        stations: vec![
            Station { id: "st_overhead".into(), name: "Overhead".into(), pos: [0., 20., 0.], yaw_deg: 0., pitch_deg: -90., lens_mm: 24, overhead: true },
            Station { id: "st_south".into(), name: "South edge, looking north".into(), pos: [0., 1.5, 6.], yaw_deg: 0., pitch_deg: 0., lens_mm: 24, overhead: false },
            Station { id: "st_north".into(), name: "North, looking south".into(), pos: [0., 1.5, -6.], yaw_deg: 180., pitch_deg: 0., lens_mm: 35, overhead: false },
        ],
        states: vec![PlaceState { id: "night".into(), name: "night".into(), key_dir_deg: 90., kelvin: 7500, mood: "cold and still".into() }],
        ..Default::default()
    };
    st.save_place(&place).expect("place");
    let dagger = Prop { id: "pr_dagger".into(), name: "dagger".into(), owner: Some("kai".into()), description: "A short black dagger.".into(), held: true, states: vec!["sheathed".into(), "drawn".into()], ..Default::default() };
    st.save_prop(&dagger).expect("prop");
    let frame = take(&st, 200);
    let style = Style { line: LINE.into(), negative: vec!["text".into(), "3D render look".into()], frame: Card { status: Status::NeedsReview, takes: vec![frame.clone()], selected: Some(frame), ..Default::default() }, ..Default::default() };
    st.save_style(&style).expect("style");
    (st, kai, place, style)
}

/// The shared rules of every spec: style line first, numbering matches the references, limits,
/// a job per reference, every file present.
fn check(st: &Store, s: &GenSpec, what: &str) {
    assert!(s.prompt_ai.starts_with(LINE), "{what}: the style line opens the prompt:\n{}", s.prompt_ai);
    assert!(s.refs.len() <= SOFT_REFS && s.refs.len() <= MAX_REFS, "{what}: {} references", s.refs.len());
    let ids = s.refs.iter().filter(|r| matches!(r.role, RefRole::Identity(_))).count();
    assert!(ids <= MAX_IDENTITY, "{what}: {ids} identity pictures");
    for r in &s.refs {
        assert!(!r.job.trim().is_empty(), "{what}: every reference has a job");
        assert!(st.path(&r.file).is_file(), "{what}: {} exists", r.file);
        assert_eq!(r.hash.len(), 64);
    }
    assert_eq!(described(&s.prompt_ai), (1..=s.refs.len()).collect::<Vec<_>>(), "{what}: each reference is described once, in upload order:\n{}", s.prompt_ai);
    assert!(s.prompt_ai.contains("No text"), "{what}: the avoid section");
    assert!(s.prompt_ai.contains("3D render look"), "{what}: the style's own negatives");
    assert_eq!(s.res, "1K");
    assert!(s.prompt_user.is_none());
}

/// Image numbers the prompt describes ("Image 3 is …", "Images 1, 2 and 3 are each …"), in order.
fn described(prompt: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for sentence in prompt.lines().flat_map(|l| l.split(". ")) {
        let s = sentence.trim();
        if let Some(rest) = s.strip_prefix("Images ") {
            let list = rest.split(" are each").next().unwrap_or("");
            out.extend(list.replace(" and ", ", ").split(", ").filter_map(|n| n.trim().parse::<usize>().ok()));
        } else if let Some(rest) = s.strip_prefix("Image ") {
            if let Some(n) = rest.split(" is ").next().and_then(|n| n.trim().parse::<usize>().ok()) {
                out.push(n);
            }
        }
    }
    out
}

fn section_order(s: &GenSpec, words: &[&str]) {
    let at: Vec<usize> = words.iter().map(|w| s.prompt_ai.find(w).unwrap_or_else(|| panic!("{w:?} in:\n{}", s.prompt_ai))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "sections in the fixed order {words:?}: {at:?}");
}

#[test]
fn character_sheets_keep_the_style_line_and_the_reference_limits() {
    let (st, kai, _, style) = project("sheets_char");
    let master = spec(&SheetKind::Master, Subject::Character(&kai), &style, &st);
    check(&st, &master, "master");
    let ids: Vec<&RefSlot> = master.refs.iter().filter(|r| matches!(r.role, RefRole::Identity(_))).collect();
    assert_eq!(ids.len(), MAX_IDENTITY, "four manga crops carry the identity");
    assert!(ids.iter().all(|r| r.slot == SiteSlot::Character));
    let suggestions: Vec<&str> = kai.crops.iter().filter(|c| c.agreed < 2).map(|c| c.take.as_str()).collect();
    assert!(master.refs.iter().all(|r| !suggestions.contains(&r.hash.as_str())), "suggested crops are never references");
    assert!(master.refs.iter().any(|r| r.role == RefRole::Style && r.slot == SiteSlot::Style), "the style frame goes along");
    // The count follows the project's "Pictures per Generate" (the Bible's "Design options"
    // button asks for MASTER_OPTIONS explicitly).
    assert_eq!((master.aspect.as_str(), master.variants), ("3:4", mv_anime::ProjectSettings::default().variants), "the master draws the project's count");
    section_order(&master, &[LINE, "Images 1, 2, 3 and 4 are each a manga panel of Kai", "Full-body character design", "Kai, about 17", "signature colour is", "Keep exactly", "No text"]);
    assert!(master.prompt_ai.contains("vivid red") || master.prompt_ai.contains("red"), "the colour is named");

    // With an approved master, the other sheets start from it.
    let mut kai2 = kai.clone();
    let m = take(&st, 120);
    kai2.sheets = vec![Sheet { kind: SheetKind::Master, card: approved(&m) }, Sheet { kind: SheetKind::Turnaround, card: approved(&take(&st, 121)) }];
    for kind in [SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::Back, SheetKind::ColourModel, SheetKind::Grip("pr_dagger".into())] {
        let s = spec(&kind, Subject::Character(&kai2), &style, &st);
        check(&st, &s, &sheet_key(&kind));
        assert_eq!(s.refs[0].hash, m, "{}: the approved master is Image 1", sheet_key(&kind));
        assert_eq!(s.refs[0].role, RefRole::Identity("kai".into()));
        assert_eq!(s.variants, 2);
    }
    let face = spec(&SheetKind::Face, Subject::Character(&kai2), &style, &st);
    assert!(face.prompt_ai.contains("front view on the left, three-quarter view on the right"));
    let turn = spec(&SheetKind::Turnaround, Subject::Character(&kai2), &style, &st);
    assert_eq!(turn.aspect, "21:9");
    assert!(turn.refs.iter().any(|r| r.role == RefRole::Panel), "side and back manga views go along");
    let grip = spec(&SheetKind::Grip("pr_dagger".into()), Subject::Character(&kai2), &style, &st);
    assert!(grip.prompt_ai.contains("holds the dagger") && grip.prompt_ai.contains("A short black dagger."));
    let pack = character_pack(&kai2, &st.bible().expect("bible"));
    assert!(pack.contains(&SheetKind::Grip("pr_dagger".into())) && pack[0] == SheetKind::Master);
}

#[test]
fn identity_pictures_never_exceed_four_even_with_many_approved_crops() {
    let (st, mut kai, _, style) = project("sheets_crowd");
    for c in &mut kai.crops {
        c.approved = true;
    }
    for i in 0..12u8 {
        let mut c = crop(&st, 60 + i, CropFraming::Face, ViewAngle::Front, 2);
        c.approved = true;
        kai.crops.push(c);
    }
    for kind in [SheetKind::Master, SheetKind::Face, SheetKind::Turnaround, SheetKind::Back] {
        let s = spec(&kind, Subject::Character(&kai), &style, &st);
        check(&st, &s, &sheet_key(&kind));
    }
}

#[test]
fn places_are_described_from_their_stations() {
    let (st, _, place, style) = project("sheets_place");
    let master = spec(&SheetKind::Master, Subject::Place(&place), &style, &st);
    check(&st, &master, "place master");
    assert!(master.refs.iter().all(|r| !matches!(r.role, RefRole::Identity(_))), "no identity pictures in a background");
    assert!(master.prompt_ai.contains("No people"));
    assert_eq!(master_station(&place).map(|s| s.id.as_str()), Some("st_south"), "the master is painted from the station that sees most");
    // From the south edge looking north: the tank is on the left, the door on the right, the railing behind.
    let p = &master.prompt_ai;
    let (tank, door) = (p.find("the water tank (left").expect("tank on the left"), p.find("the stair door (right").expect("door on the right"));
    assert!(tank < door, "listed left to right");
    assert!(p.contains("Behind the camera, not in the picture: the railing"), "{p}");
    assert!(p.contains("facing north") && p.contains("24 mm"), "{p}");
    assert!(p.contains("from screen right"), "night key light from the east falls from screen right: {p}");

    let over = plate_spec(&place, "st_overhead", "night", &style, &st);
    check(&st, &over, "overhead plate");
    assert!(over.prompt_ai.contains("Seen from directly above, top-down plan view, no perspective"), "{}", over.prompt_ai);
    assert!(over.prompt_ai.contains("20 m wide (west to east) by 14 m long (north to south)") && over.prompt_ai.contains("1.4 times as wide as it is deep"), "{}", over.prompt_ai);
    assert!(over.prompt_ai.contains("water tank in the north-west part"), "{}", over.prompt_ai);

    // With an environment master picked, every plate starts from it.
    let mut place2 = place.clone();
    place2.sheet = approved(&take(&st, 150));
    let north = plate_spec(&place2, "st_north", "night", &style, &st);
    check(&st, &north, "north plate");
    assert_eq!(north.refs[0].role, RefRole::PlaceView);
    assert!(north.prompt_ai.contains("Same place as Image 1"));
    assert!(north.prompt_ai.contains("facing south") && north.prompt_ai.contains("the railing"));
    assert!(north.prompt_ai.contains("from screen left"), "east key light, camera facing south: screen left");
}

#[test]
fn props_and_the_style_frame_keep_the_style_line() {
    let (st, kai, _, style) = project("sheets_misc");
    let bible = st.bible().expect("bible");
    let dagger = bible.prop("pr_dagger").expect("dagger");
    let s = spec(&SheetKind::Master, Subject::Prop(dagger), &style, &st);
    check(&st, &s, "prop");
    assert!(s.prompt_ai.contains("sheathed, drawn"));
    let mut kai2 = kai.clone();
    kai2.sheets = vec![Sheet { kind: SheetKind::Master, card: approved(&take(&st, 140)) }];
    st.save_character(&kai2).expect("save");
    let frame = style_frame_spec(&style, &st);
    assert!(frame.prompt_ai.starts_with(LINE));
    assert!(frame.prompt_ai.contains("Kai"));
    assert!(frame.refs.iter().any(|r| matches!(r.role, RefRole::Identity(_))));
}

/// Every bible picture drawn and approved: hero sheets, a prop sheet, the place master and a
/// plate from every station. Returns each picture's target and its selected take.
fn everything_drawn(st: &Store) -> Vec<(Target, String)> {
    let mut out = Vec::new();
    let mut b = st.bible().expect("bible");
    let mut kai = b.character("kai").expect("kai").clone();
    kai.sheets.clear();
    for (i, kind) in [SheetKind::Master, SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::Back, SheetKind::ColourModel].into_iter().enumerate() {
        let t = take(st, 160 + i as u8);
        out.push((Target::Sheet { owner: "kai".into(), sheet: sheet_key(&kind) }, t.clone()));
        kai.sheets.push(Sheet { kind, card: approved(&t) });
    }
    st.save_character(&kai).expect("kai");
    let mut dagger = b.prop("pr_dagger").expect("dagger").clone();
    let t = take(st, 170);
    dagger.sheets = vec![Sheet { kind: SheetKind::Master, card: approved(&t) }];
    out.push((Target::Sheet { owner: "pr_dagger".into(), sheet: "master".into() }, t));
    st.save_prop(&dagger).expect("dagger");
    let mut place = b.place("pl_roof").expect("roof").clone();
    let t = take(st, 171);
    place.sheet = approved(&t);
    out.push((Target::PlaceSheet { place: "pl_roof".into() }, t));
    for (i, s) in place.stations.clone().iter().enumerate() {
        let t = take(st, 172 + i as u8);
        place.plates.push(mv_anime::model::Plate { station: s.id.clone(), state: "night".into(), card: approved(&t) });
        out.push((Target::Plate { place: "pl_roof".into(), station: s.id.clone(), state: "night".into() }, t));
    }
    st.save_place(&place).expect("place");
    b.style.frame = approved(&b.style.frame.selected.clone().unwrap_or_default());
    out.push((Target::StyleFrame, b.style.frame.selected.clone().unwrap_or_default()));
    st.save_style(&b.style).expect("style");
    out
}

#[test]
fn bible_pictures_reference_each_other_one_way_only() {
    let (st, _, _, _) = project("sheets_no_loops");
    let all = everything_drawn(&st);
    // An edge from each picture to every bible picture its request uses.
    let edges: Vec<(usize, Vec<usize>)> = all
        .iter()
        .enumerate()
        .map(|(i, (target, _))| {
            let spec = mv_anime::sheets::current_spec(&st, target).expect("spec");
            let uses = spec.refs.iter().filter_map(|r| all.iter().position(|(_, t)| *t == r.hash)).collect();
            (i, uses)
        })
        .collect();
    // Redrawing one picture changes the requests of those that use it, then of those that use
    // them, ...; it must never come back to itself.
    for (start, _) in &edges {
        let mut seen = vec![false; all.len()];
        let mut stack: Vec<usize> = edges.iter().filter(|(_, uses)| uses.contains(start)).map(|(i, _)| *i).collect();
        while let Some(n) = stack.pop() {
            assert_ne!(n, *start, "redrawing {:?} would make it outdated through a loop", all[*start].0);
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(edges.iter().filter(|(_, uses)| uses.contains(&n)).map(|(i, _)| *i));
            }
        }
    }
    // The style frame uses no drawn picture at all.
    let style = all.iter().position(|(t, _)| *t == Target::StyleFrame).unwrap_or(usize::MAX);
    assert!(edges.iter().any(|(i, uses)| *i == style && uses.is_empty()));
}

#[tokio::test]
async fn approving_a_picture_accepts_it_as_it_is() {
    let (st, _, _, _) = project("sheets_accept");
    let painter = Arc::new(Painter { n: AtomicU8::new(1), seen: Default::default() });
    let target = Target::Sheet { owner: "kai".into(), sheet: "face".into() };
    draw(&st, painter, target.clone(), 1).await.expect("drawn");
    let card = |st: &Store| st.bible().expect("bible").character("kai").expect("kai").sheets.iter().find(|s| s.kind == SheetKind::Face).expect("face").card.clone();
    assert!(!mv_anime::sheets::outdated(&card(&st), &st));
    // The style line changes: the picture is outdated, until the user approves it as it is.
    let mut style = st.bible().expect("bible").style;
    style.line = "Thick brush lines.".into();
    st.save_style(&style).expect("style");
    assert!(mv_anime::sheets::outdated(&card(&st), &st));
    mv_anime::sheets::approve(&st, &target).expect("approve");
    assert_eq!(card(&st).status, Status::Approved);
    assert!(!mv_anime::sheets::outdated(&card(&st), &st), "accepted as it is");
    // A later change outdates it again.
    style.line = "Thin pencil lines.".into();
    st.save_style(&style).expect("style");
    assert!(mv_anime::sheets::outdated(&card(&st), &st));
    // Nothing to approve: a message, nothing written.
    assert!(mv_anime::sheets::approve(&st, &Target::Sheet { owner: "kai".into(), sheet: "back".into() }).is_err());
}

/// Every order of `items`.
fn orders<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let first = rest.remove(i);
        for mut tail in orders(&rest) {
            tail.insert(0, first.clone());
            out.push(tail);
        }
    }
    out
}

/// RUN1-19: approving a turnaround flipped the already-approved face sheet to "Outdated". A
/// character's sheets build on the master design only, so approving them in any order leaves
/// none outdated.
#[tokio::test]
async fn approving_a_characters_sheets_in_any_order_leaves_none_outdated() {
    let (st, _, _, _) = project("sheets_any_order");
    let painter = Arc::new(Painter { n: AtomicU8::new(1), seen: Default::default() });
    let target = |kind: &SheetKind| Target::Sheet { owner: "kai".into(), sheet: sheet_key(kind) };
    draw(&st, painter.clone(), target(&SheetKind::Master), 1).await.expect("master");
    mv_anime::sheets::approve(&st, &target(&SheetKind::Master)).expect("approve the master");
    let kinds = [SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::Back, SheetKind::ColourModel];
    for kind in &kinds {
        draw(&st, painter.clone(), target(kind), 1).await.expect("sheet");
    }
    let card = |st: &Store, kind: &SheetKind| st.bible().expect("bible").character("kai").expect("kai").sheets.iter().find(|s| &s.kind == kind).expect("sheet").card.clone();
    for order in orders(&kinds) {
        // Back to "drawn, not approved yet" for every sheet but the master.
        let mut kai = st.bible().expect("bible").character("kai").cloned().expect("kai");
        for s in kai.sheets.iter_mut().filter(|s| s.kind != SheetKind::Master) {
            s.card.status = Status::NeedsReview;
        }
        kai.extra.remove(mv_anime::sheets::EXTRA_ACCEPTED);
        st.save_character(&kai).expect("reset");
        for (i, kind) in order.iter().enumerate() {
            mv_anime::sheets::approve(&st, &target(kind)).expect("approve");
            for done in order.iter().take(i + 1).chain(std::iter::once(&SheetKind::Master)) {
                let c = card(&st, done);
                assert_eq!(c.status, Status::Approved);
                assert!(!mv_anime::sheets::outdated(&c, &st), "approving {kind:?} (order {order:?}) made the approved {done:?} outdated");
            }
        }
    }
    // Without the accepted-request shortcut too: no sheet's request names a sibling sheet.
    let b = st.bible().expect("bible");
    let kai = b.character("kai").expect("kai");
    let siblings: Vec<String> = kai.sheets.iter().filter(|s| s.kind != SheetKind::Master).filter_map(|s| s.card.selected.clone()).collect();
    for kind in &kinds {
        let spec = mv_anime::sheets::current_spec(&st, &target(kind)).expect("spec");
        assert!(spec.refs.iter().all(|r| !siblings.contains(&r.hash)), "{kind:?} uses a sibling sheet as a reference");
    }
}

#[test]
fn sheet_keys_round_trip() {
    for k in [SheetKind::Master, SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::Outfit("armour".into()), SheetKind::Grip("pr_dagger".into()), SheetKind::ColourModel, SheetKind::Back] {
        assert_eq!(sheet_kind(&sheet_key(&k)), Some(k.clone()));
    }
    assert_eq!(sheet_kind("outfit:"), None);
    assert_eq!(sheet_kind("mouth_chart"), None);
}

/// Answers every request with a new small picture and records what it was sent.
struct Painter {
    n: AtomicU8,
    seen: std::sync::Mutex<Vec<(GenRequest, Vec<PathBuf>)>>,
}

impl ImageBackend for Painter {
    fn generate<'a>(&'a self, req: &'a GenRequest, files: &'a [(PathBuf, RefSlot)], count: u8, on_status: Box<dyn Fn(&str) + Send + Sync + 'a>) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        on_status("painting");
        if let Ok(mut s) = self.seen.lock() {
            s.push((req.clone(), files.iter().map(|f| f.0.clone()).collect()));
        }
        let first = self.n.fetch_add(count, Ordering::SeqCst);
        Box::pin(async move { Ok((0..count).map(|i| png_wh(first.wrapping_add(i).wrapping_mul(7), 80, 45)).collect()) })
    }
}

async fn draw(st: &Store, image: Arc<dyn ImageBackend>, target: Target, variants: u8) -> anyhow::Result<()> {
    let ctx = RunCtx { ai: Box::new(|_: Tier| None), image, python: Box::new(|| Err(anyhow::anyhow!("no voices in tests"))) };
    let job = Job { id: 11, project: st.id.clone(), kind: JobKind::Sheet { target: target.clone(), variants }, ..Default::default() };
    let progress = |_: f32, _: &str| {};
    let cancel = mv_ai::CancellationToken::new();
    let cx = JobCtx { job: &job, store: st, run: &ctx, progress: &progress, cancel: &cancel };
    run_job(&cx, &target, variants).await
}

#[tokio::test]
async fn the_sheet_job_stores_each_picture_on_its_card() {
    let (st, _, _, _) = project("sheets_job");
    let painter = Arc::new(Painter { n: AtomicU8::new(1), seen: Default::default() });
    let target = Target::Sheet { owner: "kai".into(), sheet: "face".into() };
    draw(&st, painter.clone(), target.clone(), 2).await.expect("the job runs");
    let b = st.bible().expect("bible");
    let kai = b.character("kai").expect("kai");
    let card = &kai.sheets.iter().find(|s| s.kind == SheetKind::Face).expect("the face sheet").card;
    assert_eq!(card.takes.len(), 2, "two variants, two takes");
    assert_eq!(card.status, Status::NeedsReview);
    assert_eq!(card.selected.as_deref(), Some(card.takes[0].as_str()));
    assert!(card.gen.prompt_ai.starts_with(LINE), "the card keeps the spec it was drawn from");
    let seen = painter.seen.lock().map(|s| s.clone()).unwrap_or_default();
    assert_eq!(seen.len(), 2, "each variant is its own submission");
    let (req, files) = &seen[0];
    assert_eq!(req.prompt, card.gen.prompt_ai);
    assert_eq!(files.len(), req.refs.len());
    assert!(files.iter().all(|f| f.is_file()));
    let t = st.take(&card.takes[0]).expect("take");
    assert_eq!(t.target, target);
    assert_eq!(t.request_hash, request_hash(req));
    assert_eq!((t.w, t.h), (80, 45));

    // The user's own prompt is what is sent next time, and stays theirs.
    let mut kai = kai.clone();
    if let Some(s) = kai.sheets.iter_mut().find(|s| s.kind == SheetKind::Face) {
        s.card.gen.prompt_user = Some("My own prompt.".into());
    }
    st.save_character(&kai).expect("save");
    draw(&st, painter.clone(), target.clone(), 1).await.expect("again");
    let seen = painter.seen.lock().map(|s| s.clone()).unwrap_or_default();
    assert_eq!(seen.last().map(|s| s.0.prompt.as_str()), Some("My own prompt."));
    let b = st.bible().expect("bible");
    let card = &b.character("kai").expect("kai").sheets.iter().find(|s| s.kind == SheetKind::Face).expect("face").card;
    assert_eq!(card.takes.len(), 3);
    assert_eq!(card.gen.prompt_user.as_deref(), Some("My own prompt."));

    // A plate creates its entry; a picture failure fails the job with the backend's error.
    draw(&st, painter.clone(), Target::Plate { place: "pl_roof".into(), station: "st_overhead".into(), state: "night".into() }, 1).await.expect("plate");
    let b = st.bible().expect("bible");
    let plate = b.place("pl_roof").expect("roof").plates.iter().find(|p| p.station == "st_overhead").expect("plate").clone();
    assert_eq!(plate.card.takes.len(), 1);
    let err = draw(&st, Arc::new(NoImages), Target::StyleFrame, 1).await.expect_err("no pictures");
    assert!(err.chain().any(|e| e.downcast_ref::<ImageError>().is_some()), "the queue sees the ImageError: {err:#}");
    let err = draw(&st, painter, Target::Plate { place: "pl_roof".into(), station: "st_gone".into(), state: "night".into() }, 1).await.expect_err("unknown station");
    assert!(err.to_string().contains("no longer on"), "{err}");
}

/// A picture that never comes (the page is still drawing).
struct NeverAnswers;

impl ImageBackend for NeverAnswers {
    fn generate<'a>(&'a self, _req: &'a GenRequest, _files: &'a [(PathBuf, RefSlot)], _count: u8, _on_status: Box<dyn Fn(&str) + Send + Sync + 'a>) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        Box::pin(futures::future::pending())
    }
}

#[tokio::test]
async fn cancel_stops_a_sheet_that_is_drawing_at_once() {
    let (st, _, _, _) = project("sheets_cancel");
    let target = Target::Sheet { owner: "kai".into(), sheet: "face".into() };
    let ctx = RunCtx { ai: Box::new(|_: Tier| None), image: Arc::new(NeverAnswers), python: Box::new(|| Err(anyhow::anyhow!("no voices in tests"))) };
    let job = Job { id: 12, project: st.id.clone(), kind: JobKind::Sheet { target: target.clone(), variants: 1 }, ..Default::default() };
    let progress = |_: f32, _: &str| {};
    let cancel = mv_ai::CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        stopper.cancel();
    });
    let cx = JobCtx { job: &job, store: &st, run: &ctx, progress: &progress, cancel: &cancel };
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), run_job(&cx, &target, 1)).await;
    let err = out.expect("the job stops when cancelled, without waiting for the picture").expect_err("cancelled");
    assert!(err.to_string().contains("cancelled"), "{err}");
    let b = st.bible().expect("bible");
    let takes = b.character("kai").and_then(|k| k.sheets.iter().find(|s| s.kind == SheetKind::Face)).map(|s| s.card.takes.len()).unwrap_or(0);
    assert_eq!(takes, 0, "nothing is attached after a cancel");
}

/// Draws one picture; while it draws, the user picks another size for the card and saves it.
struct UserSavesWhileDrawing {
    st: Store,
}

impl ImageBackend for UserSavesWhileDrawing {
    fn generate<'a>(&'a self, _req: &'a GenRequest, _files: &'a [(PathBuf, RefSlot)], _count: u8, _on_status: Box<dyn Fn(&str) + Send + Sync + 'a>) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ImageError>> {
        Box::pin(async move {
            let mut kai = self.st.bible().expect("bible").character("kai").cloned().expect("kai");
            if let Some(s) = kai.sheets.iter_mut().find(|s| s.kind == SheetKind::Face) {
                s.card.gen.res = "2K".into();
            }
            self.st.save_character(&kai).expect("save");
            Ok(vec![png_wh(201, 80, 45)])
        })
    }
}

#[tokio::test]
async fn a_sheet_job_keeps_settings_saved_after_it_started() {
    let (st, _, _, _) = project("sheets_user_edit");
    let target = Target::Sheet { owner: "kai".into(), sheet: "face".into() };
    draw(&st, Arc::new(UserSavesWhileDrawing { st: st.clone() }), target, 1).await.expect("the job runs");
    let b = st.bible().expect("bible");
    let card = &b.character("kai").expect("kai").sheets.iter().find(|s| s.kind == SheetKind::Face).expect("face").card;
    assert_eq!(card.takes.len(), 1, "the picture is on the card");
    assert_eq!(card.gen.res, "2K", "the size the user picked while it drew is kept");
}