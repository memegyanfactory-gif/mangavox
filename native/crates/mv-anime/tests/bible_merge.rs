#![cfg(feature = "bible")]
//! Bible merge and gate 1 (design 6.2, 5.4): characters keyed by brain id, locked and filled
//! fields kept on a second run, places clamped into their plan with an overhead station, the
//! default style line, and what blocks "Approve the bible".
//!
//! This test binary points `MANGAVOX_HOME` at its own temp folder (one process, one data root).

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use mv_anime::bible::{gate_missing, merge, Draft, DEFAULT_STYLE_LINE, MAX_STATIONS, MIN_STATIONS};
use mv_anime::model::{
    BibleIndex, Card, Character, FloorPlan, Landmark, LandmarkKind, Look, Outfit, Place, PlaceKind, Plate, Prop, Role, Sheet, SheetKind, Station, Status, Style,
};
use mv_anime::Store;

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

fn store(id: &str) -> Store {
    home();
    Store::open(id)
}

fn kai(hair: &str, eyes: &str) -> Character {
    Character {
        brain_id: Some("kai".into()),
        name: "Kai".into(),
        role: Role::Hero,
        look: Look { hair: hair.into(), eyes: eyes.into(), signature_colour: "#C82828".into(), height_m: 1.75, keep: vec!["scar over his left eyebrow".into()], ..Default::default() },
        outfits: vec![Outfit { id: "hoodie".into(), name: "Hoodie".into(), description: "grey hoodie".into(), chapters: vec!["c1".into()] }],
        ..Default::default()
    }
}

fn station(name: &str, x: f32, z: f32, yaw: f32) -> Station {
    Station { id: String::new(), name: name.into(), pos: [x, 1.5, z], yaw_deg: yaw, pitch_deg: 0., lens_mm: 35, overhead: false }
}

fn hall(stations: Vec<Station>) -> Place {
    Place {
        name: "Statue hall".into(),
        kind: PlaceKind::Interior,
        description: "A round stone hall.".into(),
        plan: FloorPlan {
            w_m: 20.,
            d_m: 16.,
            landmarks: vec![
                Landmark { name: "Door".into(), kind: LandmarkKind::Door, pos: [0., 8.], size: [2., 0.3], h_m: 3., ..Default::default() },
                Landmark { name: "Statue".into(), kind: LandmarkKind::Feature, pos: [40., -30.], size: [3., 3.], h_m: 9., ..Default::default() },
            ],
            ..Default::default()
        },
        stations,
        ..Default::default()
    }
}

fn inside(p: &Place) -> bool {
    let (hw, hd) = (p.plan.w_m / 2. + 1e-3, p.plan.d_m / 2. + 1e-3);
    p.stations.iter().all(|s| s.pos[0].abs() <= hw && s.pos[2].abs() <= hd) && p.plan.landmarks.iter().all(|l| l.pos[0].abs() <= hw && l.pos[1].abs() <= hd)
}

#[test]
fn characters_merge_by_brain_id_and_keep_what_is_set() {
    let st = store("merge_brain");
    let r = merge(&st, Draft { characters: vec![kai("short black hair", "")], ..Default::default() }).expect("merge");
    assert_eq!(r.added, vec!["kai".to_string()], "the brain id names the character file");
    let b = st.bible().expect("bible");
    let c = b.character("kai").expect("kai");
    assert_eq!(c.brain_id.as_deref(), Some("kai"));
    assert_eq!(c.status, Status::Draft);

    // A second run under another name for the same brain id: same card, gaps filled, set values kept.
    let mut again = kai("long white hair", "grey eyes");
    again.name = "Kai Sung".into();
    again.look.keep.push("silver ring on his right hand".into());
    again.outfits.push(Outfit { id: "armour".into(), name: "Armour".into(), description: "black plate armour".into(), chapters: vec!["c2".into()] });
    let r = merge(&st, Draft { characters: vec![again], ..Default::default() }).expect("merge again");
    assert!(r.added.is_empty(), "no second card: {r:?}");
    assert_eq!(r.updated, vec!["kai".to_string()]);
    let b = st.bible().expect("bible");
    assert_eq!(b.characters.len(), 1);
    let c = b.character("kai").expect("kai");
    assert_eq!(c.name, "Kai", "a set name is not overwritten");
    assert_eq!(c.look.hair, "short black hair", "a set field is not overwritten");
    assert_eq!(c.look.eyes, "grey eyes", "an empty field is filled");
    assert_eq!(c.outfits.len(), 2, "new outfits are added");
    assert_eq!(c.look.keep.len(), 2, "new keep features are added");

    // Nothing new: nothing changes.
    let r = merge(&st, Draft { characters: vec![kai("short black hair", "grey eyes")], ..Default::default() }).expect("merge");
    assert!(r.updated.is_empty() && r.added.is_empty(), "{r:?}");
}

#[test]
fn locked_fields_status_and_sheets_are_never_touched() {
    let st = store("merge_locks");
    let mut c = kai("", "");
    c.id = "kai".into();
    c.role = Role::Ally;
    c.status = Status::Approved;
    c.locked = BTreeSet::from(["/look/hair".to_string(), "/role".to_string()]);
    c.sheets = vec![Sheet { kind: SheetKind::Master, card: Card { status: Status::Approved, takes: vec!["t1".into()], selected: Some("t1".into()), ..Default::default() } }];
    st.save_character(&c).expect("save");

    let mut ai = kai("blue hair", "red eyes");
    ai.role = Role::Threat;
    merge(&st, Draft { characters: vec![ai], ..Default::default() }).expect("merge");
    let b = st.bible().expect("bible");
    let got = b.character("kai").expect("kai");
    assert_eq!(got.look.hair, "", "a locked field stays as the user left it, even empty");
    assert_eq!(got.role, Role::Ally, "a locked role stays");
    assert_eq!(got.look.eyes, "red eyes", "an unlocked empty field is filled");
    assert_eq!(got.status, Status::Approved, "the review status is the user's");
    assert_eq!(got.sheets, c.sheets, "sheets are cards: never touched by a merge");
    assert_eq!(got.locked, c.locked);
}

#[test]
fn a_character_without_brain_id_matches_by_name_or_alias() {
    let st = store("merge_names");
    let mut c = Character { id: "ch_ren".into(), name: "Ren".into(), aliases: vec!["Shadow".into()], ..Default::default() };
    c.look.hair = "black".into();
    st.save_character(&c).expect("save");
    let ai = Character { name: "shadow".into(), look: Look { eyes: "violet".into(), ..Default::default() }, ..Default::default() };
    let r = merge(&st, Draft { characters: vec![ai], ..Default::default() }).expect("merge");
    assert!(r.added.is_empty());
    let b = st.bible().expect("bible");
    assert_eq!(b.characters.len(), 1);
    assert_eq!(b.characters[0].look.eyes, "violet");
}

#[test]
fn what_the_user_removed_does_not_come_back_on_a_rebuild() {
    let st = store("merge_removed");
    let mut ai = kai("short black hair", "grey eyes");
    ai.look.keep.push("scar over his RIGHT eyebrow".into());
    merge(&st, Draft { characters: vec![ai.clone()], ..Default::default() }).expect("merge");
    // The user removes the wrong feature and the outfit, without locking anything.
    let mut c = st.bible().expect("bible").character("kai").expect("kai").clone();
    c.look.keep.retain(|k| !k.contains("RIGHT"));
    c.outfits.clear();
    st.save_character(&c).expect("save");
    // The same answer again, plus one new feature.
    ai.look.keep.push("silver ring".into());
    merge(&st, Draft { characters: vec![ai], ..Default::default() }).expect("merge again");
    let c = st.bible().expect("bible").character("kai").expect("kai").clone();
    assert!(!c.look.keep.iter().any(|k| k.contains("RIGHT")), "{:?}", c.look.keep);
    assert!(c.outfits.is_empty(), "{:?}", c.outfits);
    assert!(c.look.keep.iter().any(|k| k == "silver ring"), "something new is still added");
}

#[test]
fn the_gate_never_shows_an_id() {
    let hero = Character { id: "ch_x9".into(), role: Role::Hero, ..Default::default() };
    let place = Place { id: "pl_q7".into(), ..Default::default() };
    let bible = BibleIndex { characters: vec![hero], places: vec![place], style: Style { line: "Lines.".into(), ..Default::default() }, ..Default::default() };
    let missing = gate_missing(&bible);
    assert!(missing.iter().any(|m| m.starts_with("An unnamed character: approve")), "{missing:?}");
    assert!(missing.iter().any(|m| m.starts_with("An unnamed place: approve")), "{missing:?}");
    assert!(missing.iter().all(|m| !m.contains("ch_x9") && !m.contains("pl_q7")), "{missing:?}");
}

#[test]
fn a_renamed_character_is_not_added_twice() {
    let st = store("merge_renamed");
    let ren = Character { name: "Ren".into(), look: Look { hair: "black".into(), ..Default::default() }, ..Default::default() };
    merge(&st, Draft { characters: vec![ren.clone()], ..Default::default() }).expect("merge");
    let mut c = st.bible().expect("bible").characters[0].clone();
    assert_eq!(c.id, "ch_ren");
    c.name = "Ren Kurosawa".into();
    st.save_character(&c).expect("save");
    let r = merge(&st, Draft { characters: vec![ren], ..Default::default() }).expect("merge again");
    assert!(r.added.is_empty(), "{r:?}");
    let b = st.bible().expect("bible");
    assert_eq!(b.characters.len(), 1);
    assert_eq!(b.characters[0].name, "Ren Kurosawa");
}

#[test]
fn places_are_clamped_into_the_plan_and_always_have_an_overhead_station() {
    let st = store("merge_places");
    let stations = vec![
        station("Door, looking in", 0., 9.5, 0.),         // a little outside: clamped
        station("Far away", 100., 100., 0.),             // far outside: dropped
        station("Statue, looking at the door", 0., -6., 180.),
    ];
    let r = merge(&st, Draft { places: vec![hall(stations)], ..Default::default() }).expect("merge");
    assert_eq!(r.added.len(), 1);
    assert!(r.issues.iter().any(|i| i.message.contains("Far away")), "the dropped station is reported: {:?}", r.issues);
    let b = st.bible().expect("bible");
    let p = &b.places[0];
    assert_eq!(p.id, "pl_statue_hall");
    assert!(inside(p), "stations and landmarks inside the plan: {:?}", p.stations);
    assert_eq!(p.stations.iter().filter(|s| s.overhead).count(), 1, "exactly one overhead");
    let over = p.stations.iter().find(|s| s.overhead).expect("overhead");
    assert_eq!(over.pitch_deg, -90.);
    assert!(over.pos[1] >= 16., "as high as the plan is large: {}", over.pos[1]);
    assert!(!p.stations.iter().any(|s| s.name == "Far away"));
    let door = p.stations.iter().find(|s| s.name == "Door, looking in").expect("door station kept");
    assert!((door.pos[2] - 8.).abs() < 1e-3, "clamped to the south edge: {:?}", door.pos);
    assert!(p.stations.len() >= MIN_STATIONS && p.stations.len() <= MAX_STATIONS, "{} stations", p.stations.len());
    assert!(p.stations.iter().all(|s| [14, 24, 35, 50, 85, 135].contains(&s.lens_mm)));
    let ids: BTreeSet<&str> = p.stations.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids.len(), p.stations.len(), "station ids are unique");
    assert!(!p.states.is_empty(), "a place always has a light state");
    assert_eq!(p.plan.walls.len(), 4, "an interior gets its outer walls");

    // The user removes the overhead and moves a station (locked); a second run puts the overhead
    // back, keeps the user's station where it is, and adds no twin of an existing view.
    let mut p = p.clone();
    p.stations.retain(|s| !s.overhead);
    let door_id = p.stations.iter().find(|s| s.name == "Door, looking in").map(|s| s.id.clone()).unwrap_or_default();
    if let Some(s) = p.stations.iter_mut().find(|s| s.id == door_id) {
        s.pos = [3., 1.2, 7.];
    }
    p.locked.insert(format!("/stations/{door_id}/pos"));
    st.save_place(&p).expect("save");
    let before = p.stations.len();
    let again = hall(vec![station("Door, looking in", 0., 8., 0.), station("Statue, looking at the door", 0., -6., 180.)]);
    merge(&st, Draft { places: vec![again], ..Default::default() }).expect("merge again");
    let b = st.bible().expect("bible");
    assert_eq!(b.places.len(), 1, "same place, no second card");
    let p2 = &b.places[0];
    assert_eq!(p2.stations.iter().filter(|s| s.overhead).count(), 1, "the overhead is back");
    assert_eq!(p2.stations.len(), before + 1, "nothing but the overhead was added: {:?}", p2.stations.iter().map(|s| &s.name).collect::<Vec<_>>());
    let door = p2.stations.iter().find(|s| s.id == door_id).expect("door");
    assert_eq!(door.pos, [3., 1.2, 7.], "a locked position is kept");
}

#[test]
fn the_style_line_falls_back_to_the_default_and_props_merge_by_name() {
    let st = store("merge_style");
    let prop = Prop { name: "Dagger".into(), description: String::new(), held: true, ..Default::default() };
    merge(&st, Draft { props: vec![prop], style: Some(Style::default()), ..Default::default() }).expect("merge");
    let b = st.bible().expect("bible");
    assert_eq!(b.style.line, DEFAULT_STYLE_LINE);
    assert!(b.style.negative.iter().any(|n| n == "text"));
    assert_eq!(b.props[0].id, "pr_dagger");

    let prop = Prop { name: "dagger".into(), description: "A short black dagger.".into(), ..Default::default() };
    let style = Style { line: "Thick ink lines.".into(), ..Default::default() };
    merge(&st, Draft { props: vec![prop], style: Some(style), ..Default::default() }).expect("merge");
    let b = st.bible().expect("bible");
    assert_eq!(b.props.len(), 1);
    assert_eq!(b.props[0].description, "A short black dagger.");
    assert!(b.props[0].held, "held stays true");
    assert_eq!(b.style.line, DEFAULT_STYLE_LINE, "a set style line is not overwritten");
}

fn approved(take: &str) -> Card {
    Card { status: Status::Approved, takes: vec![take.into()], selected: Some(take.into()), ..Default::default() }
}

#[test]
fn the_gate_lists_what_is_missing_until_everything_is_approved() {
    let empty = BibleIndex::default();
    let missing = gate_missing(&empty);
    assert!(missing.iter().any(|m| m.contains("style line")));
    assert!(missing.iter().any(|m| m.contains("no characters")));
    assert!(missing.iter().any(|m| m.contains("no places")));

    let mut hero = Character { id: "kai".into(), name: "Kai".into(), role: Role::Hero, ..Default::default() };
    let extra = Character { id: "ama".into(), name: "Ama".into(), role: Role::Neutral, ..Default::default() };
    let mut place = hall(vec![]);
    place.id = "pl_hall".into();
    place.stations = vec![Station { id: "st_overhead".into(), overhead: true, pitch_deg: -90., ..Default::default() }, station("Door", 0., 7., 0.)];
    let mut bible = BibleIndex { characters: vec![hero.clone(), extra], places: vec![place.clone()], style: Style { line: "Lines.".into(), ..Default::default() }, ..Default::default() };
    let missing = gate_missing(&bible);
    assert!(missing.iter().any(|m| m.starts_with("Kai:") && m.contains("master design") && m.contains("face sheet") && m.contains("turnaround")), "{missing:?}");
    assert!(!missing.iter().any(|m| m.starts_with("Ama:")), "a neutral side character is not required when main characters exist");
    assert!(missing.iter().any(|m| m.contains("environment master")));
    assert!(missing.iter().any(|m| m.contains("overhead plate")));

    hero.sheets = vec![
        Sheet { kind: SheetKind::Master, card: approved("a") },
        Sheet { kind: SheetKind::Face, card: approved("b") },
        Sheet { kind: SheetKind::Turnaround, card: Card { status: Status::NeedsReview, selected: Some("c".into()), ..Default::default() } },
    ];
    bible.characters[0] = hero.clone();
    let missing = gate_missing(&bible);
    assert!(missing.iter().any(|m| m == "Kai: approve the turnaround."), "{missing:?}");

    hero.sheets[2].card = approved("c");
    bible.characters[0] = hero;
    place.sheet = approved("d");
    place.plates = vec![Plate { station: "st_door".into(), state: "day".into(), card: approved("e") }];
    bible.places[0] = place.clone();
    let missing = gate_missing(&bible);
    assert_eq!(missing, vec!["Statue hall: approve the overhead plate.".to_string()], "a plate from another station is not the overhead plate");

    place.plates.push(Plate { station: "st_overhead".into(), state: "day".into(), card: approved("f") });
    bible.places[0] = place;
    assert!(gate_missing(&bible).is_empty(), "{:?}", gate_missing(&bible));

    // A rejected character or place does not block the gate.
    bible.places.push(Place { id: "pl_old".into(), name: "Old".into(), status: Status::Rejected, ..Default::default() });
    assert!(gate_missing(&bible).is_empty());
}
