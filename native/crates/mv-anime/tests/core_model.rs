//! The data model (design 5.3), canonical hashes (5.5) and lock-respecting merges (5.4): pure,
//! no data root needed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use mv_anime::hash::{canonical, file_hash, request_hash, sha256_hex, sound_hash, value_hash};
use mv_anime::merge::{is_locked, merge_unlocked, CARD_FIELDS};
use mv_anime::queue::JobKind;
use mv_anime::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------
// Helpers

fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| panic!("serialize: {e}"))
}

fn from_value<T: DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("deserialize {v}: {e}"))
}

/// Pretty JSON text and back: equal.
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(v: &T) {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|e| panic!("serialize: {e}"));
    let back: T = serde_json::from_str(&text).unwrap_or_else(|e| panic!("deserialize {text}: {e}"));
    assert_eq!(&back, v, "round trip changed the value");
}

/// A top-level entity: fields written by a newer app survive, and `schema` is kept as written.
fn keeps_unknown_fields<T: Serialize + DeserializeOwned + PartialEq + Debug>(v: &T) {
    round_trip(v);
    let mut j = to_value(v);
    let obj = j.as_object_mut().unwrap_or_else(|| panic!("not an object"));
    obj.insert("from_the_future".into(), json!({"panoramas": [1, 2, {"x": "y"}]}));
    obj.insert("another_new_field".into(), json!("kept"));
    obj.insert("schema".into(), json!(1));
    let back: T = from_value(j.clone());
    let again = to_value(&back);
    assert_eq!(again, j, "an unknown field or the schema was lost");
    // A file from a newer app keeps its higher schema number as written.
    let mut newer = j.clone();
    newer["schema"] = json!(7);
    let back: T = from_value(newer.clone());
    assert_eq!(to_value(&back)["schema"], json!(7));
}

fn card(seed: &str) -> Card {
    Card {
        status: Status::NeedsReview,
        locked: true,
        takes: vec![format!("{seed}1"), format!("{seed}2")],
        selected: Some(format!("{seed}2")),
        gen: GenSpec {
            model: "nano-banana-2-1".into(),
            prompt_ai: format!("prompt for {seed}"),
            prompt_user: Some("mine".into()),
            prompt_user_base: Some("abc".into()),
            refs: vec![
                RefSlot { file: "takes/ab/ab.png".into(), hash: "ab".into(), role: RefRole::Plate, job: "owns the room".into(), slot: SiteSlot::Add, pinned: false },
                RefSlot { file: "takes/cd/cd.png".into(), hash: "cd".into(), role: RefRole::Identity("kai".into()), job: "face".into(), slot: SiteSlot::Character, pinned: true },
                RefSlot { file: "x.png".into(), hash: "ef".into(), role: RefRole::ColourModel("ren".into()), job: "".into(), slot: SiteSlot::Style, pinned: false },
            ],
            refs_user: true,
            aspect: "16:9".into(),
            res: "1K".into(),
            variants: 2,
        },
    }
}

fn source(page: usize) -> SourceRef {
    SourceRef { chapter_id: "c1".into(), page, panel: Some(page % 3) }
}

fn sample_series() -> Series {
    Series {
        schema: SCHEMA,
        id: "m1".into(),
        manga_id: "m1".into(),
        manga_title: "Test Manga".into(),
        created_at: 1_700_000_000,
        episodes: vec![EpisodeRef { id: "ep01".into(), title: "The Tower".into() }],
        settings: ProjectSettings {
            planner: Some(ModelPick { provider: "claude".into(), model: "big".into() }),
            helper: None,
            variants: 3,
            ..Default::default()
        },
        next_ids: BTreeMap::from([("scene".to_string(), 4), ("shot".to_string(), 12)]),
        bible_approved: Some(Approval { at: 1_700_000_100, hash: "h".into() }),
        extra: Default::default(),
    }
}

fn sample_episode() -> Episode {
    Episode {
        schema: SCHEMA,
        id: "ep01".into(),
        title: "The Tower".into(),
        chapters: vec![ChapterPick {
            chapter_id: "c1".into(),
            manga_id: "m1".into(),
            label: "Chapter 1".into(),
            number: Some(1.5),
            pages_dir: "library/manga/m1/c1".into(),
            output_dir: "x/c1".into(),
        }],
        target_minutes: 7.5,
        scene_order: vec!["sc0001".into(), "sc0002".into()],
        animatic_approved: Some(Approval { at: 5, hash: "t".into() }),
        extra: Default::default(),
    }
}

fn sample_line(id: &str, scene: &str) -> Line {
    Line {
        id: id.into(),
        scene: scene.into(),
        speaker: Some("kai".into()),
        kind: LineKind::Thought,
        original: "I must go.".into(),
        spoken: "I have to go.".into(),
        keep_words: true,
        emotion: Emotion { kind: EmotionKind::Resolve, strength: 3 },
        pace: Pace::Fast,
        source: Some(source(2)),
        voice: VoiceCard {
            status: Status::Approved,
            locked: false,
            takes: vec!["v1".into()],
            selected: Some("v1".into()),
            voice_override: Some("cb_storyteller_m".into()),
            speed: 1.1,
        },
        edited: true,
        locked: BTreeSet::from(["/spoken".to_string()]),
    }
}

fn sample_story() -> Story {
    Story {
        schema: SCHEMA,
        episode: "ep01".into(),
        scenes: vec![Scene {
            id: "sc0001".into(),
            title: "Rooftop".into(),
            place: Some("pl_rooftop".into()),
            time_of_day: TimeOfDay::Night,
            kind: SceneKind::Dread,
            tone: Tone::Horror,
            fight: true,
            colour: ColourTheme { name: "deep blue threat".into(), palette: Some("p1".into()) },
            intensity: 8,
            target_s: 42.5,
            summary: "They meet.".into(),
            beats: vec![Beat { id: "bt0001".into(), purpose: "establish".into(), source: vec![source(0)], lines: vec!["ln0001".into()], hook: true }],
            source: vec![source(0), source(1)],
            edited: true,
            locked: BTreeSet::from(["/tone".to_string()]),
        }],
        lines: vec![sample_line("ln0001", "sc0001")],
        model: "test-model".into(),
        made_at: 1_700_000_000,
        extra: Default::default(),
    }
}

fn sample_shot(id: &str) -> Shot {
    Shot {
        id: id.into(),
        tells: "Kai sees the tower.".into(),
        beat: Some("bt0001".into()),
        origin: Origin::Anime,
        source: vec![source(1)],
        pattern: Some(Pattern::ThreatReveal),
        camera: Camera {
            size: ShotSize::TwoShot,
            angle: Angle::Dutch,
            station: Some("st1".into()),
            pos: Some([1.0, 1.5, -2.25]),
            look_at: LookAt::Character("kai".into()),
            roll_deg: 15.0,
            lens_mm: 85,
            depth: Depth::Shallow,
            focus: Focus { target: FocusTarget::Character("kai".into()), rack_to: Some(FocusTarget::Prop("sword".into())), rack_at_s: 1.25 },
            key: Key::Dark,
            grain: Grain::Heavy,
            glow: true,
            light: LightSetup::UnderLight,
            movement: Movement { kind: Move::CrashZoom, speed: Speed::Snap, amount: 1.5 },
        },
        marks: vec![
            Mark {
                character: "kai".into(),
                pos: [0.5, -1.0],
                facing_deg: 90.0,
                pose: Pose::Fight,
                visible: Visibility::Ots,
                outfit: Some("coat".into()),
                emotion: Emotion { kind: EmotionKind::Menace, strength: 2 },
                action: "draws the sword".into(),
                holding: vec![Hold { prop: "sword".into(), hand: Hand::Both }],
                gaze: Some("char:ren".into()),
            },
            Mark { character: "ren".into(), pos: [2.0, 1.0], ..Default::default() },
        ],
        lines: vec!["ln0001".into()],
        mouth: MouthPolicy::Flap,
        motion: MotionKind::Partial,
        background: BackgroundKind::Backdrop,
        route: MotionRoute::Kling25Web,
        hit: true,
        flash: true,
        bridge: true,
        transition: Transition::White,
        planned_s: 3.5,
        locked_s: Some(4.0),
        frame: card("f"),
        clip: Some(card("c")),
        suggested: json!({"size": "cu", "lens": 50}),
        edited: true,
        locked: BTreeSet::from(["/camera/lens_mm".to_string()]),
    }
}

fn sample_scene_shots() -> SceneShots {
    let mut a = sample_shot("sh0001");
    let mut b = sample_shot("sh0002");
    a.camera.look_at = LookAt::Point([1.0, 2.0, 3.0]);
    b.camera.look_at = LookAt::Landmark("door".into());
    b.camera.focus.target = FocusTarget::Background;
    b.camera.focus.rack_to = Some(FocusTarget::None);
    let mut c = sample_shot("sh0003");
    c.camera.look_at = LookAt::Angles { yaw_deg: 45.0, pitch_deg: -10.0 };
    c.camera.focus.target = FocusTarget::Auto;
    SceneShots { schema: SCHEMA, episode: "ep01".into(), scene: "sc0001".into(), shots: vec![a, b, c], planned_by: "planner".into(), revisions: 2, extra: Default::default() }
}

fn crop() -> RefCrop {
    RefCrop { file: "takes/aa/aa.png".into(), take: "aa".into(), source: source(3), angle: ViewAngle::ThreeQuarter, framing: CropFraming::Bust, score: 0.75, approved: true, agreed: 2 }
}

fn sample_character() -> Character {
    Character {
        schema: SCHEMA,
        id: "kai".into(),
        brain_id: Some("kai".into()),
        name: "Kai".into(),
        aliases: vec!["The Hunter".into()],
        role: Role::Hero,
        description: "A hunter.".into(),
        age: "17".into(),
        look: Look {
            hair: "black".into(),
            eyes: "grey".into(),
            build: "lean".into(),
            default_outfit: "coat".into(),
            signature_colour: "#C82828".into(),
            keep: vec!["scar".into()],
            height_m: 1.78,
        },
        outfits: vec![Outfit { id: "coat".into(), name: "Coat".into(), description: "long".into(), chapters: vec!["c1".into()] }],
        crops: vec![crop()],
        sheets: vec![
            Sheet { kind: SheetKind::Master, card: card("m") },
            Sheet { kind: SheetKind::Outfit("coat".into()), card: card("o") },
            Sheet { kind: SheetKind::Grip("sword".into()), card: Card::default() },
            Sheet { kind: SheetKind::ColourModel, card: Card::default() },
            Sheet { kind: SheetKind::Back, card: Card::default() },
        ],
        status: Status::Approved,
        locked: BTreeSet::from(["/look/hair".to_string()]),
        extra: Default::default(),
    }
}

fn sample_place() -> Place {
    Place {
        schema: SCHEMA,
        id: "pl_rooftop".into(),
        name: "Rooftop".into(),
        kind: PlaceKind::Exterior,
        description: "Windy.".into(),
        plan: FloorPlan {
            w_m: 12.0,
            d_m: 8.0,
            walls: vec![Wall { a: [-6.0, -4.0], b: [6.0, -4.0], h_m: 1.2 }],
            landmarks: vec![Landmark { id: "door".into(), name: "Door".into(), kind: LandmarkKind::Door, pos: [0.0, -4.0], size: [1.0, 0.2], rot_deg: 0.0, h_m: 2.1 }],
            north_deg: 10.0,
        },
        stations: vec![Station { id: "st1".into(), name: "door, looking in".into(), pos: [0.0, 1.5, -3.5], yaw_deg: 180.0, pitch_deg: -5.0, lens_mm: 24, overhead: false }],
        states: vec![PlaceState { id: "night".into(), name: "Night".into(), key_dir_deg: 30.0, kelvin: 4000, mood: "cold".into() }],
        sheet: card("s"),
        plates: vec![Plate { station: "st1".into(), state: "night".into(), card: card("p") }],
        crops: vec![crop()],
        status: Status::Rejected,
        locked: BTreeSet::new(),
        extra: Default::default(),
    }
}

fn sample_prop() -> Prop {
    Prop {
        schema: SCHEMA,
        id: "sword".into(),
        name: "Sword".into(),
        owner: Some("kai".into()),
        description: "Black blade.".into(),
        held: true,
        states: vec!["sheathed".into(), "drawn".into()],
        sheets: vec![Sheet { kind: SheetKind::Master, card: card("sw") }],
        crops: vec![],
        status: Status::Draft,
        locked: BTreeSet::from(["/held".to_string()]),
        extra: Default::default(),
    }
}

fn sample_style() -> Style {
    Style {
        schema: SCHEMA,
        line: "thin dark-brown lines".into(),
        negative: vec!["text".into(), "speech bubbles".into()],
        frame: card("st"),
        palettes: vec![Palette {
            id: "p1".into(),
            name: "Night".into(),
            swatches: vec![Swatch { role: "dominant".into(), hex: "#102040".into() }, Swatch { role: "accent".into(), hex: "#F0C020".into() }],
            brightness: 0.3,
            saturation: 0.6,
        }],
        status: Status::NeedsReview,
        locked: BTreeSet::from(["/line".to_string()]),
        extra: Default::default(),
    }
}

fn sample_cast() -> VoiceCast {
    VoiceCast {
        schema: SCHEMA,
        narrator: CharVoice { character: String::new(), voice_id: "cb_storyteller_m".into(), reference: None, speed: 1.0, locked: true, auditions: vec![] },
        voices: vec![CharVoice {
            character: "kai".into(),
            voice_id: "am_adam".into(),
            reference: Some("takes/ab/ab.wav".into()),
            speed: 0.95,
            locked: false,
            auditions: vec!["t1".into(), "t2".into()],
        }],
        extra: Default::default(),
    }
}

fn sample_request(prompt: &str, refs: &[(&str, &str)]) -> GenRequest {
    GenRequest {
        template: "nb21.frame@1".into(),
        model: "nano-banana-2-1".into(),
        prompt: prompt.into(),
        refs: refs.iter().map(|(h, r)| (h.to_string(), r.to_string(), SiteSlot::Add)).collect(),
        aspect: "16:9".into(),
        res: "1K".into(),
    }
}

fn sample_takes() -> Vec<Take> {
    let base = Take {
        schema: SCHEMA,
        id: "ab".repeat(32),
        file: format!("takes/ab/{}.png", "ab".repeat(32)),
        kind: TakeKind::Image,
        target: Target::ShotFrame { episode: "ep01".into(), shot: "sh0001".into() },
        request: Some(sample_request("a prompt", &[("h1", "plate")])),
        request_hash: "rh".into(),
        source: TakeSource::MagnificWeb { model: "nano-banana-2-1".into(), res: "1K".into(), free: true, site_id: "x1".into() },
        created_at: 1_700_000_000,
        w: 1376,
        h: 768,
        seconds: 0.0,
        batch: Some("b1".into()),
        note: "first".into(),
    };
    let targets = vec![
        Target::Sheet { owner: "kai".into(), sheet: "face".into() },
        Target::PlaceSheet { place: "pl_rooftop".into() },
        Target::Plate { place: "pl_rooftop".into(), station: "st1".into(), state: "night".into() },
        Target::StyleFrame,
        Target::LineVoice { episode: "ep01".into(), line: "ln0001".into() },
        Target::Audition { character: "kai".into() },
        Target::Crop { owner: "kai".into() },
        Target::Upload,
        Target::Animatic { episode: "ep01".into() },
    ];
    let sources = vec![
        TakeSource::Voice { engine: "kokoro".into(), voice: "am_adam".into() },
        TakeSource::Upload { name: "mine.png".into() },
        TakeSource::MangaCrop(source(2)),
        TakeSource::Ffmpeg,
    ];
    let mut out = vec![base.clone()];
    for (i, t) in targets.into_iter().enumerate() {
        let mut x = base.clone();
        x.target = t;
        x.source = sources[i % sources.len()].clone();
        x.request = None;
        x.kind = [TakeKind::Image, TakeKind::Audio, TakeKind::Video][i % 3];
        x.seconds = i as f32 * 0.5;
        out.push(x);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Round trips

#[test]
fn every_top_level_entity_round_trips_and_keeps_unknown_fields_and_schema() {
    keeps_unknown_fields(&sample_series());
    keeps_unknown_fields(&sample_episode());
    keeps_unknown_fields(&sample_story());
    keeps_unknown_fields(&sample_scene_shots());
    keeps_unknown_fields(&sample_character());
    keeps_unknown_fields(&sample_place());
    keeps_unknown_fields(&sample_prop());
    keeps_unknown_fields(&sample_style());
    keeps_unknown_fields(&sample_cast());
    keeps_unknown_fields(&mv_anime::settings::Settings {
        schema: SCHEMA,
        llm_parallel: 2,
        eta: BTreeMap::from([("frame".to_string(), vec![30.5, 41.0])]),
        extra: Default::default(),
    });
    for t in sample_takes() {
        round_trip(&t);
    }
}

#[test]
fn every_nested_type_round_trips() {
    round_trip(&sample_shot("sh0009"));
    round_trip(&sample_line("ln0009", "sc0001"));
    round_trip(&card("x"));
    round_trip(&sample_request("p", &[("a", "plate"), ("b", "Kai's face")]));
    round_trip(&TakeMeta {
        kind: TakeKind::Audio,
        target: Target::LineVoice { episode: "ep01".into(), line: "ln1".into() },
        request: None,
        request_hash: "x".into(),
        source: TakeSource::Voice { engine: "chatterbox".into(), voice: "cb_x".into() },
        w: 0,
        h: 0,
        seconds: 2.5,
        batch: None,
        note: "n".into(),
    });
    round_trip(&Issue { code: "axis".into(), severity: Severity::Error, scene: "sc1".into(), shot: Some("sh1".into()), message: "m".into(), fix: Some("f".into()) });
    round_trip(&Compiled { text: "t".into(), sections: vec![("Style".into(), "s".into())] });
    round_trip(&Derived {
        hfov_deg: 23.9,
        horizon_y: 0.42,
        perspective: "one-point".into(),
        screen_x: BTreeMap::from([("kai".to_string(), 0.33)]),
        sides: BTreeMap::from([("kai|ren".to_string(), -1i8)]),
        behind: BTreeMap::from([("kai".to_string(), vec!["door".to_string()])]),
        station: Some("st1".into()),
    });
    // Every tagged-enum variant with data, through the shots that hold them.
    for kind in [JobKind::Story { episode: "e".into() }, JobKind::Sheet { target: Target::StyleFrame, variants: 4 }, JobKind::Unknown] {
        round_trip(&kind);
    }
}

#[test]
fn an_empty_file_reads_as_defaults_and_nested_unknown_fields_are_harmless() {
    let s: Story = from_value(json!({}));
    assert_eq!(s, Story::default());
    let s: Story = from_value(json!({
        "schema": 1,
        "scenes": [{"id": "sc1", "new_scene_field": {"a": 1}, "beats": [{"id": "b", "brand_new": true}]}],
        "lines": [{"id": "ln1", "voice": {"status": "approved", "new_voice_field": 3}}]
    }));
    assert_eq!(s.scenes[0].id, "sc1");
    assert_eq!(s.lines[0].voice.status, Status::Approved);
    let shot: Shot = from_value(json!({"id": "sh1", "camera": {"lens_mm": 50, "look_at": {"kind": "character", "value": "kai"}}}));
    assert_eq!(shot.camera.look_at, LookAt::Character("kai".into()));
    assert_eq!(shot.camera.lens_mm, 50);
}

/// `words!` enums: every variant round trips as its snake_case word, and a word this build does
/// not know reads as `Unknown` (the `#[serde(other)]` fallback).
macro_rules! words_fall_back {
    ($($t:ident { $($v:ident),+ }),+ $(,)?) => {$(
        {
            for v in [$($t::$v),+, $t::Unknown] {
                round_trip(&v);
                let w = to_value(&v);
                assert!(w.as_str().is_some_and(|s| s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')), "{w} is not snake_case");
            }
            let future: $t = from_value(json!("a_word_from_the_future"));
            assert_eq!(future, $t::Unknown, "{} has no working fallback", stringify!($t));
            let _default = $t::default();
        }
    )+};
}

#[test]
fn every_plain_word_enum_falls_back_to_unknown() {
    words_fall_back! {
        ShotSize { Els, Ls, Fs, Cowboy, Ms, Mcu, Cu, Ecu, Insert, Ots, TwoShot, Pov, Overhead },
        Angle { Eye, Low, High, Top, Worm, Dutch },
        Move { Hold, PushIn, PullOut, PanLeft, PanRight, TiltUp, TiltDown, TruckLeft, TruckRight, Shake, Whip, CrashZoom, RackFocus, BgSlide },
        Speed { Creep, Slow, Medium, Fast, Snap },
        SceneKind { Dialogue, Action, Dread, Narration, Flashback, Montage },
        Tone { Drama, Romance, Action, Fight, Horror, Comedy, Calm, Mystery },
        EmotionKind { Neutral, Joy, Sadness, Anger, Fear, Surprise, Disgust, Resolve, Pride, Grief, Despair, Relief, Awe, Tender, Menace },
        LineKind { Speech, Thought, Narration, Sfx },
        Pace { Slow, Normal, Fast },
        MouthPolicy { Offscreen, Back, Hidden, Closed, Flap, Generate },
        MotionKind { Hold, MouthOnly, CameraOnly, Partial, Full },
        MotionRoute { Auto, Compositor, Kling25Web, LocalH3, Paid },
        BackgroundKind { Matched, Backdrop, None },
        Visibility { Full, Partial, Back, Ots, Offscreen },
        Pose { Stand, Sit, Kneel, Lie, Run, Fight },
        Hand { Left, Right, Both },
        Role { Hero, Ally, Threat, Neutral },
        Pattern { Arrival, Conversation, ThreatReveal, Stunned, Reveal, SuddenDeath, Chase },
        Transition { Cut, Dissolve, White, Black },
        TimeOfDay { Day, Dusk, Night, Dawn, Indoor },
        PlaceKind { Interior, Exterior },
        LandmarkKind { Door, Window, Wall, Furniture, Prop, Feature, Light },
        ViewAngle { Front, ThreeQuarter, Side, Back },
        CropFraming { Face, Bust, Full },
        SiteSlot { Add, Style, Character },
        Depth { Deep, Normal, Shallow },
        Key { Dark, Normal, Bright },
        Grain { Clean, Light, Heavy },
        LightSetup { Auto, Backlit, Window, UnderLight, Silhouette, Top, Rim },
        Status { Draft, NeedsReview, Approved, Rejected },
        Severity { Error, Warning, Info },
        TakeKind { Image, Audio, Video },
        Origin { Manga, Anime },
    }
    // Words inside an entity: one unknown word never fails the file.
    let shot: Shot = from_value(json!({"camera": {"size": "hyper_close", "movement": {"kind": "drone_orbit"}}, "mouth": "lip_sync_v9"}));
    assert_eq!(shot.camera.size, ShotSize::Unknown);
    assert_eq!(shot.camera.movement.kind, Move::Unknown);
    assert_eq!(shot.mouth, MouthPolicy::Unknown);
}

#[test]
fn every_tagged_enum_falls_back_to_unknown() {
    // Adjacently tagged (`kind` + `id`), with and without content.
    for future in [json!({"kind": "panorama"}), json!({"kind": "panorama", "id": "x"}), json!({"kind": "panorama", "id": {"deep": [1]}})] {
        let k: SheetKind = from_value(future.clone());
        assert_eq!(k, SheetKind::Unknown, "SheetKind from {future}");
        let r: RefRole = from_value(future.clone());
        assert_eq!(r, RefRole::Unknown, "RefRole from {future}");
    }
    // Internally tagged (`kind` + the variant's fields).
    for future in [json!({"kind": "clip"}), json!({"kind": "clip", "episode": "ep01", "shot": "sh1", "extra": [1, 2]})] {
        let t: Target = from_value(future.clone());
        assert_eq!(t, Target::Unknown, "Target from {future}");
        let s: TakeSource = from_value(future.clone());
        assert_eq!(s, TakeSource::Unknown, "TakeSource from {future}");
        let j: JobKind = from_value(future.clone());
        assert_eq!(j, JobKind::Unknown, "JobKind from {future}");
    }
    // Inside the files that hold them.
    let take: Take = from_value(json!({"schema": 2, "target": {"kind": "clip", "shot": "sh1"}, "source": {"kind": "kling_web", "credits": 3}}));
    assert_eq!((take.target, take.source), (Target::Unknown, TakeSource::Unknown));
    let ch: Character = from_value(json!({"id": "kai", "sheets": [{"kind": {"kind": "pose_sheet", "id": "x"}}]}));
    assert_eq!(ch.sheets[0].kind, SheetKind::Unknown);
    // Every known variant round trips through its tag.
    for k in [
        SheetKind::Master,
        SheetKind::Face,
        SheetKind::Turnaround,
        SheetKind::Expressions,
        SheetKind::Outfit("coat".into()),
        SheetKind::Grip("sword".into()),
        SheetKind::ColourModel,
        SheetKind::Back,
        SheetKind::Unknown,
    ] {
        round_trip(&k);
    }
    for r in [
        RefRole::Plate,
        RefRole::Layout,
        RefRole::Identity("kai".into()),
        RefRole::Style,
        RefRole::Expression("kai".into()),
        RefRole::Prop("sword".into()),
        RefRole::Grip("sword".into()),
        RefRole::ColourModel("kai".into()),
        RefRole::PlaceView,
        RefRole::Panel,
        RefRole::User,
        RefRole::Unknown,
    ] {
        round_trip(&r);
    }
    for t in [Target::Unknown] {
        round_trip(&t);
    }
    round_trip(&TakeSource::Unknown);
    for l in [LookAt::Character("a".into()), LookAt::Landmark("d".into()), LookAt::Point([1.0, 2.0, 3.0]), LookAt::Angles { yaw_deg: 1.0, pitch_deg: -2.0 }] {
        round_trip(&l);
    }
    for f in [FocusTarget::Auto, FocusTarget::Character("a".into()), FocusTarget::Prop("p".into()), FocusTarget::Background, FocusTarget::None] {
        round_trip(&f);
    }
}

// ---------------------------------------------------------------------------------------------
// Hashes

#[test]
fn canonical_hash_ignores_key_order() {
    let a: Value = serde_json::from_str(r#"{"prompt":"p","refs":[{"h":"1","r":"plate"}],"model":"m","nested":{"z":1,"a":[{"y":2,"b":3}]}}"#).unwrap_or_default();
    let b: Value = serde_json::from_str(r#"{"nested":{"a":[{"b":3,"y":2}],"z":1},"model":"m","refs":[{"r":"plate","h":"1"}],"prompt":"p"}"#).unwrap_or_default();
    assert_eq!(canonical(&a), canonical(&b));
    assert_eq!(value_hash(&a), value_hash(&b));
    assert!(!canonical(&a).contains(' '), "no whitespace");
    // Array order does matter (reference upload order is part of the request).
    let c: Value = serde_json::from_str(r#"{"refs":[2,1]}"#).unwrap_or_default();
    let d: Value = serde_json::from_str(r#"{"refs":[1,2]}"#).unwrap_or_default();
    assert_ne!(value_hash(&c), value_hash(&d));
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    // The request hash is the canonical hash of the request, whatever order serde writes.
    let req = sample_request("prompt", &[("h1", "plate")]);
    assert_eq!(request_hash(&req), value_hash(&to_value(&req)));
    assert_eq!(request_hash(&req).len(), 64);
}

#[test]
fn picture_hash_follows_reference_file_content_and_not_voices() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("temp: {e}"));
    let sheet = dir.path().join("face_sheet.png");
    std::fs::write(&sheet, b"first drawing").unwrap_or_else(|e| panic!("{e}"));
    let make = |line: &Line| -> (String, String) {
        let h = file_hash(&sheet).unwrap_or_default();
        let picture = request_hash(&sample_request("Kai on the roof", &[(&h, "Kai's face sheet")]));
        let sound = sound_hash(&line.spoken, line.voice.voice_override.as_deref().unwrap_or("am_adam"), None, line.voice.speed, &line.emotion, "kokoro");
        (picture, sound)
    };
    let mut line = sample_line("ln0001", "sc0001");
    let (pic1, snd1) = make(&line);
    assert_eq!(make(&line), (pic1.clone(), snd1.clone()), "stable");

    // A voice change: the sound channel moves, the picture does not.
    line.voice.voice_override = Some("bf_emma".into());
    let (pic2, snd2) = make(&line);
    assert_eq!(pic2, pic1, "a voice change must never outdate a picture");
    assert_ne!(snd2, snd1);
    line.voice.speed = 0.8;
    line.spoken = "Different words.".into();
    let (pic3, snd3) = make(&line);
    assert_eq!(pic3, pic1);
    assert_ne!(snd3, snd2);

    // The referenced sheet is redrawn: same path, new bytes -> the picture is outdated.
    std::fs::write(&sheet, b"second drawing").unwrap_or_else(|e| panic!("{e}"));
    let (pic4, snd4) = make(&line);
    assert_ne!(pic4, pic1);
    assert_eq!(snd4, snd3, "a picture change must never outdate a voice");
    assert_eq!(file_hash(&sheet).unwrap_or_default(), sha256_hex(b"second drawing"));
    assert!(file_hash(&dir.path().join("missing.png")).is_err());

    // A reference clip feeds the sound hash.
    let e = Emotion { kind: EmotionKind::Fear, strength: 2 };
    assert_ne!(sound_hash("a", "cb_x", Some("r1"), 1.0, &e, "chatterbox"), sound_hash("a", "cb_x", Some("r2"), 1.0, &e, "chatterbox"));
    assert_ne!(sound_hash("a", "cb_x", None, 1.0, &e, "chatterbox"), sound_hash("a", "cb_x", None, 1.0, &Emotion { kind: EmotionKind::Fear, strength: 3 }, "chatterbox"));
}

// ---------------------------------------------------------------------------------------------
// Merge

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn lock_pointers_cover_their_descendants_only() {
    let locked = set(&["/camera", "/marks/kai/pos", "/a~1b"]);
    assert!(is_locked("/camera", &locked));
    assert!(is_locked("/camera/lens_mm", &locked));
    assert!(is_locked("/marks/kai/pos", &locked));
    assert!(is_locked("/marks/kai/pos/0", &locked));
    assert!(!is_locked("/marks/kai", &locked));
    assert!(!is_locked("/marks/kai/facing_deg", &locked));
    assert!(!is_locked("/cameraman", &locked), "a prefix that is not an ancestor");
    assert!(is_locked("/a~1b/c", &locked));
    assert!(!is_locked("/a", &locked));
    assert_eq!(CARD_FIELDS, ["/frame", "/voice", "/sheets"]);
}

#[test]
fn an_ai_result_never_overwrites_a_locked_or_card_field() {
    let mut mine = sample_shot("sh0001");
    mine.locked = set(&["/camera/lens_mm", "/marks/kai/pos", "/tells"]);
    mine.tells = "My words.".into();
    let mut ai = sample_shot("sh0099");
    ai.tells = "AI words.".into();
    ai.camera.lens_mm = 24;
    ai.camera.angle = Angle::Low;
    ai.marks[0].pos = [9.0, 9.0];
    ai.marks[0].facing_deg = 270.0;
    ai.marks[1].character = "ama".into(); // Ren left the shot, Ama came in.
    ai.frame = Card::default();
    ai.clip = None;
    ai.edited = false;
    ai.locked = BTreeSet::new();
    ai.planned_s = 6.0;

    let mut cur = to_value(&mine);
    merge_unlocked(&mut cur, &to_value(&ai), &mine.locked);
    let merged: Shot = from_value(cur);

    assert_eq!(merged.tells, "My words.", "locked field kept");
    assert_eq!(merged.camera.lens_mm, mine.camera.lens_mm, "locked nested field kept");
    assert_eq!(merged.camera.angle, Angle::Low, "unlocked sibling updated");
    let kai = merged.marks.iter().find(|m| m.character == "kai").cloned().unwrap_or_default();
    assert_eq!(kai.pos, mine.marks[0].pos, "a mark's locked position kept (addressed by character)");
    assert_eq!(kai.facing_deg, 270.0, "the same mark's unlocked facing updated");
    assert!(merged.marks.iter().any(|m| m.character == "ama"), "new mark added");
    assert!(!merged.marks.iter().any(|m| m.character == "ren"), "a mark with nothing locked follows the AI");
    assert_eq!(merged.frame, mine.frame, "card never touched");
    assert_eq!(merged.clip, mine.clip, "clip card never touched");
    assert_eq!(merged.locked, mine.locked, "locks never cleared");
    assert!(merged.edited, "edited never cleared");
    assert_eq!(merged.id, "sh0001", "the entity keeps its id");
    assert_eq!(merged.planned_s, 6.0);

    // A whole locked object.
    let locked = set(&["/camera"]);
    let mut cur = to_value(&mine);
    merge_unlocked(&mut cur, &to_value(&ai), &locked);
    let merged: Shot = from_value(cur);
    assert_eq!(merged.camera, mine.camera);
}

#[test]
fn a_story_merge_keeps_lines_with_voices_and_locked_text() {
    let mut mine = sample_story();
    let mut plain = sample_line("ln0002", "sc0001");
    plain.voice = VoiceCard::default();
    plain.locked = BTreeSet::new();
    plain.edited = false;
    mine.lines.push(plain);
    let locked = set(&["/lines/ln0001/spoken", "/scenes/sc0001/tone"]);

    let mut ai = Story { episode: "ep01".into(), ..Default::default() };
    let mut scene = mine.scenes[0].clone();
    scene.tone = Tone::Comedy;
    scene.title = "New title".into();
    scene.locked = BTreeSet::new();
    scene.edited = false;
    ai.scenes = vec![scene];
    let mut l1 = sample_line("ln0001", "sc0001");
    l1.spoken = "AI rewrite.".into();
    l1.pace = Pace::Slow;
    l1.voice = VoiceCard::default();
    let l3 = Line { id: "ln0003".into(), scene: "sc0001".into(), spoken: "New line.".into(), voice: VoiceCard { takes: vec!["junk".into()], ..Default::default() }, ..Default::default() };
    ai.lines = vec![l1, l3]; // ln0002 left out.

    let mut cur = to_value(&mine);
    merge_unlocked(&mut cur, &to_value(&ai), &locked);
    let merged: Story = from_value(cur);
    let l1 = merged.lines.iter().find(|l| l.id == "ln0001").cloned().unwrap_or_default();
    assert_eq!(l1.spoken, "I have to go.", "locked line text kept");
    assert_eq!(l1.pace, Pace::Slow, "unlocked line field updated");
    assert_eq!(l1.voice, mine.lines[0].voice, "voice card untouched");
    let l3 = merged.lines.iter().find(|l| l.id == "ln0003").cloned().unwrap_or_default();
    assert_eq!(l3.spoken, "New line.");
    assert!(l3.voice.takes.is_empty(), "a new line never brings card contents");
    assert!(!merged.lines.iter().any(|l| l.id == "ln0002"), "a plain line the AI dropped goes");
    assert_eq!(merged.scenes[0].tone, Tone::Horror, "locked scene field kept");
    assert_eq!(merged.scenes[0].title, "New title");

    // Fields the AI did not send are kept.
    let mut cur = to_value(&mine.scenes[0]);
    merge_unlocked(&mut cur, &json!({"title": "Only the title", "status": "approved", "locked": []}), &BTreeSet::new());
    let scene: Scene = from_value(cur);
    assert_eq!(scene.title, "Only the title");
    assert_eq!(scene.summary, mine.scenes[0].summary);
    assert_eq!(scene.locked, mine.scenes[0].locked);

    // A line with a voice the AI left out is kept.
    let mut ai2 = ai.clone();
    ai2.lines.retain(|l| l.id != "ln0001");
    let mut cur = to_value(&mine);
    merge_unlocked(&mut cur, &to_value(&ai2), &BTreeSet::new());
    let merged: Story = from_value(cur);
    assert!(merged.lines.iter().any(|l| l.id == "ln0001"), "a voiced line is never dropped by an AI merge");
}

#[test]
fn a_bible_merge_keeps_sheets_and_locked_looks() {
    let mine = sample_character();
    let mut ai = Character { id: "kai".into(), name: "Kai Renamed".into(), ..Default::default() };
    ai.look.hair = "white".into();
    ai.look.eyes = "blue".into();
    let mut cur = to_value(&mine);
    merge_unlocked(&mut cur, &to_value(&ai), &mine.locked);
    let merged: Character = from_value(cur);
    assert_eq!(merged.look.hair, "black", "locked");
    assert_eq!(merged.look.eyes, "blue");
    assert_eq!(merged.name, "Kai Renamed");
    assert_eq!(merged.sheets, mine.sheets, "sheets are cards: never touched");
    assert_eq!(merged.locked, mine.locked);
    assert_eq!(merged.status, Status::Approved, "the user's approval is never undone by an AI merge");
}
