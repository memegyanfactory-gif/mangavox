//! The prompt compiler's phrase banks and a store-free compile (design 6.6): every bank covers
//! its enum, the lens is described by what it does, colours get words.
#![cfg(feature = "frames")]

use mv_anime::compile::{bank, colour_name, emotion_adjective, emotion_words, frame_prompt, lens_phrase, SECTIONS};
use mv_anime::geometry::derive;
use mv_anime::*;

#[test]
fn lenses_are_described_by_what_they_do() {
    assert!(lens_phrase(14).starts_with("ultra-wide 14 mm lens: camera very close"));
    assert!(lens_phrase(24).starts_with("wide 24 mm lens: camera close, space stretched"));
    assert!(lens_phrase(35).starts_with("35 mm lens"));
    assert!(lens_phrase(50).starts_with("normal 50 mm lens"));
    assert!(lens_phrase(85).starts_with("long 85 mm lens: camera far away, background compressed"));
    assert!(lens_phrase(135).starts_with("telephoto 135 mm lens"));
    assert!(lens_phrase(0).starts_with("35 mm lens"), "no lens = the default 35 mm");
}

#[test]
fn colour_words() {
    assert_eq!(colour_name("#B22222").as_deref(), Some("red"));
    assert_eq!(colour_name("#1E2C4F").as_deref(), Some("dark blue"));
    assert_eq!(colour_name("#28A03C").as_deref(), Some("green"));
    assert_eq!(colour_name("#FFFFFF").as_deref(), Some("white"));
    assert_eq!(colour_name("#000000").as_deref(), Some("black"));
    assert_eq!(colour_name("#808080").as_deref(), Some("grey"));
    assert_eq!(colour_name("#FFB6C1").as_deref(), Some("pink"));
    assert_eq!(colour_name("red"), None);
}

#[test]
fn banks_cover_their_enums() {
    use ShotSize::*;
    for s in [Els, Ls, Fs, Cowboy, Ms, Mcu, Cu, Ecu, Insert, Ots, TwoShot, Pov, Overhead] {
        assert!(bank::SIZE.iter().any(|(k, _)| *k == s), "{s:?}");
    }
    for a in [Angle::Eye, Angle::Low, Angle::High, Angle::Top, Angle::Worm, Angle::Dutch] {
        assert!(bank::ANGLE.iter().any(|(k, _)| *k == a), "{a:?}");
    }
    use EmotionKind as E;
    for e in [E::Neutral, E::Joy, E::Sadness, E::Anger, E::Fear, E::Surprise, E::Disgust, E::Resolve, E::Pride, E::Grief, E::Despair, E::Relief, E::Awe, E::Tender, E::Menace] {
        assert!(bank::EMOTION.iter().any(|(k, _)| *k == e), "{e:?}");
        assert_ne!(emotion_adjective(e), "");
    }
    assert_eq!(emotion_words(E::Anger, 1), "slightly annoyed, a small frown");
    assert_eq!(emotion_words(E::Anger, 0), emotion_words(E::Anger, 2), "unset strength reads as medium");
    assert_eq!(emotion_words(E::Anger, 3), "furious, teeth bared, eyes blazing");
}

#[test]
fn a_shot_with_no_bible_still_compiles_cleanly() {
    let shot = Shot {
        id: "sh0001".into(),
        camera: Camera { size: ShotSize::TwoShot, lens_mm: 85, angle: Angle::Low, ..Default::default() },
        marks: vec![Mark { character: "a".into(), ..Default::default() }],
        background: BackgroundKind::None,
        ..Default::default()
    };
    let scene = Scene { id: "sc1".into(), ..Default::default() };
    let bible = BibleIndex::default();
    let c = frame_prompt(&shot, &scene, &bible, &derive(&shot, None), &[]);
    let names: Vec<&str> = c.sections.iter().map(|(n, _)| n.as_str()).collect();
    // No pictures, props or keep-list here: those sections are left out, the order holds.
    assert_eq!(names, vec!["Style", "Camera", "Action", "Colour", "Avoid"]);
    assert!(names.iter().all(|n| SECTIONS.contains(n)));
    assert!(c.text.starts_with(mv_anime::compile::STYLE_FALLBACK));
    // A two-shot with one character falls back to a medium shot.
    assert!(c.text.contains("Medium shot: a from the waist up."), "{}", c.text);
    assert!(c.text.contains("Low-angle shot looking up at a."), "{}", c.text);
    assert!(!c.text.contains("{"), "unfilled placeholder: {}", c.text);
    assert!(!c.text.contains(".."), "{}", c.text);
    assert_eq!(frame_prompt(&shot, &scene, &bible, &derive(&shot, None), &[]), c);
}

#[test]
fn a_cut_outfit_leaves_no_dangling_punctuation() {
    let kai = Character {
        id: "kai".into(),
        name: "Kai".into(),
        // Cut at 60 characters back to the last space, right after "trousers;".
        look: Look { default_outfit: "black school jacket; dark trousers; white-sneakers-with-red-laces-and-socks".into(), signature_colour: "#2846C8".into(), ..Default::default() },
        ..Default::default()
    };
    let bible = BibleIndex { characters: vec![kai], ..Default::default() };
    let shot = Shot { id: "sh1".into(), camera: Camera { size: ShotSize::Ms, ..Default::default() }, marks: vec![Mark { character: "kai".into(), ..Default::default() }], ..Default::default() };
    let scene = Scene { id: "sc1".into(), ..Default::default() };
    let text = frame_prompt(&shot, &scene, &bible, &derive(&shot, None), &[]).text;
    assert!(text.contains("Kai (black school jacket; dark trousers, blue signature colour)"), "{text}");
    assert!(!text.contains(";,") && !text.contains(",,"), "{text}");
}

#[test]
fn a_person_framing_with_nobody_in_it_frames_the_place() {
    let place = Place { id: "pl_room".into(), name: "Hospital room".into(), ..Default::default() };
    let bible = BibleIndex { places: vec![place], ..Default::default() };
    let scene = Scene { id: "sc1".into(), place: Some("pl_room".into()), ..Default::default() };
    for size in [ShotSize::Ms, ShotSize::Mcu, ShotSize::Cu, ShotSize::Ecu] {
        let shot = Shot { id: "sh1".into(), camera: Camera { size, ..Default::default() }, ..Default::default() };
        let text = frame_prompt(&shot, &scene, &bible, &derive(&shot, None), &[]).text;
        assert!(!text.contains("Hospital room from") && !text.contains("Hospital room's"), "{size:?}: {text}");
        assert!(text.contains("nobody"), "{size:?}: {text}");
    }
}
