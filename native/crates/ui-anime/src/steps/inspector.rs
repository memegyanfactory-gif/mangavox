//! The shot inspector of the Shots step (design 9.7) [shots]: framing, lens and focus,
//! exposure, light, movement, acting per character, and the shot's lines, all as chip rows that
//! start on Auto (design 5.4). A pick is the user's (its JSON pointer goes into the shot's
//! `locked` set and the planner never overwrites it); Auto takes the director's suggestion back.
//! W3-S3.
//!
//! The inspector only reads the shot and asks the screen for changes ([`Ask`]); the screen
//! applies them to its copy at once and saves them in the background.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::merge::{self, is_locked};
use mv_anime::{
    geometry, patterns, Angle, BackgroundKind, BibleIndex, Depth, EmotionKind, FocusTarget, Grain, Hand, Hold, Key, LightSetup, Line, LineKind, Mark, MotionKind, MouthPolicy, Move, Origin,
    Place, Pose, Shot, ShotSize, Speed, Visibility,
};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, filter_chip, icon_button, icon_ph, menu_button, muted, ph, segmented, size, tip, ButtonKind, MenuItem, Tokens};
use serde_json::Value;

use crate::widgets::{auto_chips, column_head, info_line, inspector_row, label_caps, marker, spec, LineTone};

// ---------------------------------------------------------------- words

pub(super) const SIZES: [(ShotSize, &str); 13] = [
    (ShotSize::Els, "Extreme wide"),
    (ShotSize::Ls, "Wide"),
    (ShotSize::Fs, "Full"),
    (ShotSize::Cowboy, "Cowboy"),
    (ShotSize::Ms, "Medium"),
    (ShotSize::Mcu, "Medium close"),
    (ShotSize::Cu, "Close-up"),
    (ShotSize::Ecu, "Extreme close"),
    (ShotSize::Insert, "Insert"),
    (ShotSize::Ots, "Over the shoulder"),
    (ShotSize::TwoShot, "Two-shot"),
    (ShotSize::Pov, "Point of view"),
    (ShotSize::Overhead, "Overhead"),
];

const ANGLES: [(Angle, &str); 6] = [(Angle::Eye, "Eye level"), (Angle::Low, "Low"), (Angle::High, "High"), (Angle::Top, "Top-down"), (Angle::Worm, "Ground level"), (Angle::Dutch, "Tilted")];

/// Camera heights offered (centimetres, so the value is `Copy + Eq`).
const HEIGHTS: [(u16, &str); 6] = [(20, "Ground"), (60, "Knee"), (100, "Waist"), (150, "Eye"), (300, "High"), (600, "Very high")];

const DEPTHS: [(Depth, &str); 3] = [(Depth::Deep, "Deep"), (Depth::Normal, "Normal"), (Depth::Shallow, "Shallow")];
const KEYS: [(Key, &str); 3] = [(Key::Dark, "Dark"), (Key::Normal, "Normal"), (Key::Bright, "Bright")];
const GRAINS: [(Grain, &str); 3] = [(Grain::Clean, "Clean"), (Grain::Light, "Light"), (Grain::Heavy, "Heavy")];
const GLOWS: [(bool, &str); 2] = [(false, "Off"), (true, "On")];

const LIGHTS: [(LightSetup, &str); 7] = [
    (LightSetup::Auto, "From the place"),
    (LightSetup::Backlit, "Backlit"),
    (LightSetup::Window, "Window light"),
    (LightSetup::UnderLight, "Under-light"),
    (LightSetup::Silhouette, "Silhouette"),
    (LightSetup::Top, "Top light"),
    (LightSetup::Rim, "Rim light"),
];

pub(super) const MOVES: [(Move, &str); 14] = [
    (Move::Hold, "Still"),
    (Move::PushIn, "Push in"),
    (Move::PullOut, "Pull out"),
    (Move::PanLeft, "Pan left"),
    (Move::PanRight, "Pan right"),
    (Move::TiltUp, "Tilt up"),
    (Move::TiltDown, "Tilt down"),
    (Move::TruckLeft, "Slide left"),
    (Move::TruckRight, "Slide right"),
    (Move::Shake, "Shake"),
    (Move::Whip, "Whip pan"),
    (Move::CrashZoom, "Crash zoom"),
    (Move::RackFocus, "Rack focus"),
    (Move::BgSlide, "Background slide"),
];

const SPEEDS: [(Speed, &str); 5] = [(Speed::Creep, "Creep"), (Speed::Slow, "Slow"), (Speed::Medium, "Medium"), (Speed::Fast, "Fast"), (Speed::Snap, "Snap")];

const MOTIONS: [(MotionKind, &str); 5] = [
    (MotionKind::Hold, "Still picture"),
    (MotionKind::MouthOnly, "Mouth only"),
    (MotionKind::CameraOnly, "Camera only"),
    (MotionKind::Partial, "Some movement"),
    (MotionKind::Full, "Full animation"),
];

const MOUTHS: [(MouthPolicy, &str); 6] = [
    (MouthPolicy::Offscreen, "Speaker off screen"),
    (MouthPolicy::Back, "From behind"),
    (MouthPolicy::Hidden, "Mouth hidden"),
    (MouthPolicy::Closed, "Closed"),
    (MouthPolicy::Flap, "Drawn mouth shapes"),
    (MouthPolicy::Generate, "Drawn by the model"),
];

const BACKGROUNDS: [(BackgroundKind, &str); 3] = [(BackgroundKind::Matched, "The place"), (BackgroundKind::Backdrop, "Simple backdrop"), (BackgroundKind::None, "No background")];

const SEEN: [(Visibility, &str); 5] = [(Visibility::Full, "In full"), (Visibility::Partial, "Partly"), (Visibility::Back, "From behind"), (Visibility::Ots, "Shoulder in front"), (Visibility::Offscreen, "Off screen")];

const POSES: [(Pose, &str); 6] = [(Pose::Stand, "Standing"), (Pose::Sit, "Sitting"), (Pose::Kneel, "Kneeling"), (Pose::Lie, "Lying"), (Pose::Run, "Running"), (Pose::Fight, "Fighting")];

const EMOTIONS: [(EmotionKind, &str); 15] = [
    (EmotionKind::Neutral, "Neutral"),
    (EmotionKind::Joy, "Joy"),
    (EmotionKind::Sadness, "Sadness"),
    (EmotionKind::Anger, "Anger"),
    (EmotionKind::Fear, "Fear"),
    (EmotionKind::Surprise, "Surprise"),
    (EmotionKind::Disgust, "Disgust"),
    (EmotionKind::Resolve, "Resolve"),
    (EmotionKind::Pride, "Pride"),
    (EmotionKind::Grief, "Grief"),
    (EmotionKind::Despair, "Despair"),
    (EmotionKind::Relief, "Relief"),
    (EmotionKind::Awe, "Awe"),
    (EmotionKind::Tender, "Tender"),
    (EmotionKind::Menace, "Menace"),
];

const STRENGTHS: [&str; 3] = ["Mild", "Medium", "Strong"];
const HANDS: [(Hand, &str); 3] = [(Hand::Left, "Left"), (Hand::Right, "Right"), (Hand::Both, "Both")];

/// What the "ISO" controls are, in one line (plan 8).
const ISO_NOTE: &str = "A picture model has no sensor, so ISO is offered as what it changes: the key sets the brightness of the still; grain and glow are added when the shot is put together.";

/// Seconds the length buttons add or take away.
const LENGTH_STEP_S: f32 = 0.5;

/// The word of a value in a table.
pub(super) fn word<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table.iter().find(|(k, _)| *k == v).map(|(_, w)| *w).unwrap_or("Other")
}

/// A shot size in plain words.
pub(super) fn size_word(s: ShotSize) -> &'static str {
    if s == ShotSize::Unknown {
        "Shot"
    } else {
        word(&SIZES, s)
    }
}

/// A camera move in plain words.
pub(super) fn move_word(m: Move) -> &'static str {
    if m == Move::Unknown {
        "Still"
    } else {
        word(&MOVES, m)
    }
}

/// What a lens does, with its field of view: "35 mm · 54° across, close to how the eye sees".
pub(super) fn lens_text(mm: u16) -> String {
    let (h, _) = geometry::fov_deg(mm as f32);
    let feel = match mm {
        0..=20 => "very wide: near things loom, the room stretches",
        21..=30 => "wide: the space opens up, near things look big",
        31..=60 => "close to how the eye sees",
        61..=100 => "long: the face fills the frame, the background flattens and softens",
        _ => "very long: far things pulled close, the background a wash",
    };
    format!("{mm} mm sees {h:.0}° across: {feel}.")
}

/// Where a camera move is carried out, and what moves inside the picture.
pub(super) fn move_note(m: Move, motion: MotionKind) -> String {
    let camera = match m {
        Move::Hold | Move::Unknown => "The camera does not move.",
        _ => "Done when the shot is put together: the still is moved and zoomed, nothing new is drawn.",
    };
    match motion {
        MotionKind::Partial | MotionKind::Full => format!("{camera} Movement inside the picture comes with the Animate step."),
        MotionKind::MouthOnly => format!("{camera} The mouth shapes are swapped to the voice."),
        _ => camera.to_string(),
    }
}

/// The height option nearest to a camera height (metres).
pub(super) fn height_option(h: f32) -> u16 {
    let cm = if h.is_finite() { h * 100. } else { 150. };
    HEIGHTS.iter().map(|(v, _)| *v).min_by(|a, b| (*a as f32 - cm).abs().total_cmp(&(*b as f32 - cm).abs())).unwrap_or(150)
}

// ---------------------------------------------------------------- pointers and locks

/// A change to one shot, applied to the screen's copy and to the file's.
pub(super) type ShotFn = Arc<dyn Fn(&mut Shot) + Send + Sync>;

/// The pointer of a character's field: "/marks/jinwoo/emotion".
pub(super) fn mark_ptr(character: &str, field: &str) -> String {
    format!("/marks/{}/{field}", merge::escape(character))
}

/// The camera's place on the plan: its position, its station and where it looks.
pub(super) const POSITION: [&str; 3] = ["/camera/pos", "/camera/station", "/camera/look_at"];

/// The camera's height alone (the middle of its position), so the Height row and a move on the
/// map are the user's separately.
pub(super) const HEIGHT: &str = "/camera/pos/1";

/// Height back to Auto: the plan's height comes back and the camera stays where it stands on
/// the map. When nobody moved the camera by hand its whole position is the plan's again.
pub(super) fn height_auto(s: &mut Shot, place: Option<&Place>) {
    s.locked.remove(HEIGHT);
    if !is_locked("/camera/pos", &s.locked) {
        reset(s, "/camera/pos");
        return;
    }
    let planned = value_at(&s.suggested, HEIGHT)
        .and_then(Value::as_f64)
        .map(|h| h as f32)
        .or_else(|| value_at(&s.suggested, "/camera").cloned().and_then(|c| serde_json::from_value::<mv_anime::Camera>(c).ok()).map(|c| geometry::cam_pos(&c, place)[1]));
    if let Some(h) = planned.filter(|h| h.is_finite()) {
        let p = geometry::cam_pos(&s.camera, place);
        s.camera.pos = Some([p[0], h, p[2]]);
    }
}

/// Marks a field as the user's: the planner never overwrites it.
pub(super) fn lock(shot: &mut Shot, pointer: &str) {
    shot.locked.insert(pointer.to_string());
    shot.edited = true;
}

/// Back to Auto: the pointer (and any under it) is no longer the user's, and the director's
/// suggestion for it comes back. Without a suggestion the value stays as it is.
pub(super) fn reset(shot: &mut Shot, pointer: &str) {
    let under = format!("{pointer}/");
    shot.locked.retain(|p| p != pointer && !p.starts_with(&under));
    let Some(ai) = value_at(&shot.suggested, pointer).cloned() else { return };
    let Ok(mut cur) = serde_json::to_value(&*shot) else { return };
    let Some(slot) = value_at_mut(&mut cur, pointer) else { return };
    *slot = ai;
    if let Ok(s) = serde_json::from_value::<Shot>(cur) {
        *shot = s;
    }
}

fn segments(pointer: &str) -> Vec<String> {
    pointer.split('/').skip(1).map(|s| s.replace("~1", "/").replace("~0", "~")).collect()
}

/// An array element's key: its `id`, else its `character` (marks are addressed by character).
fn element_key(v: &Value) -> Option<&str> {
    ["id", "character"].iter().find_map(|k| v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
}

fn child_index(a: &[Value], seg: &str) -> Option<usize> {
    a.iter().position(|e| element_key(e) == Some(seg)).or_else(|| seg.parse::<usize>().ok().filter(|i| *i < a.len()))
}

/// The value at a pointer whose array steps name an element's id or character.
pub(super) fn value_at<'a>(v: &'a Value, pointer: &str) -> Option<&'a Value> {
    let mut cur = v;
    for seg in segments(pointer) {
        cur = match cur {
            Value::Object(m) => m.get(&seg)?,
            Value::Array(a) => &a[child_index(a, &seg)?],
            _ => return None,
        };
    }
    Some(cur)
}

fn value_at_mut<'a>(v: &'a mut Value, pointer: &str) -> Option<&'a mut Value> {
    let mut cur = v;
    for seg in segments(pointer) {
        cur = match cur {
            Value::Object(m) => m.get_mut(&seg)?,
            Value::Array(a) => {
                let i = child_index(a, &seg)?;
                &mut a[i]
            }
            _ => return None,
        };
    }
    Some(cur)
}

// ---------------------------------------------------------------- what the inspector asks

/// A request from the inspector to its screen.
pub(super) enum Ask {
    /// Set fields (`Some`: apply and lock every pointer) or put them back to Auto (`None`).
    Edit { pointers: Vec<String>, set: Option<ShotFn> },
    /// Show this character's acting.
    Character(String),
    /// Put a character of the scene into the shot.
    AddCharacter(String),
    /// Put the camera on a station of the place.
    Station(String),
    /// Move a line to the shot before or after.
    MoveLine { line: String, forward: bool },
    /// Close the drawer (narrow window).
    Close,
}

impl Ask {
    fn set(pointers: Vec<String>, f: impl Fn(&mut Shot) + Send + Sync + 'static) -> Ask {
        Ask::Edit { pointers, set: Some(Arc::new(f)) }
    }

    fn auto(pointers: Vec<String>) -> Ask {
        Ask::Edit { pointers, set: None }
    }
}

pub(super) type OnAsk = Rc<dyn Fn(Ask, &mut Window, &mut App)>;

/// Everything the inspector shows.
pub(super) struct Inspect<'a> {
    pub shot: &'a Shot,
    /// "3.4".
    pub number: &'a str,
    pub place: Option<&'a Place>,
    pub bible: &'a BibleIndex,
    /// The scene's lines.
    pub lines: &'a [Line],
    /// Characters of the scene (ids), for "Add a character".
    pub cast: &'a [String],
    /// The character whose acting is shown.
    pub character: Option<&'a str>,
    pub tells: Option<&'a Entity<TextareaState>>,
    pub action: Option<&'a Entity<TextareaState>>,
    /// The shot is the scene's first / last (lines cannot move past them).
    pub first: bool,
    pub last: bool,
    pub narrow: bool,
    /// The scene is being planned: the settings show but cannot be changed.
    pub busy: bool,
    /// What the scene's planning job is doing, said while `busy` ("Planning…" only while it runs;
    /// waiting in a paused queue says so).
    pub busy_note: &'static str,
}

impl Inspect<'_> {
    fn name(&self, id: &str) -> String {
        self.bible.character(id).map(|c| c.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string())
    }

    fn hue(&self, id: &str) -> usize {
        self.bible.characters.iter().position(|c| c.id == id).unwrap_or_else(|| id.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)))
    }

    fn locked(&self, pointer: &str) -> bool {
        is_locked(pointer, &self.shot.locked)
    }
}

fn eid(prefix: &str, key: &str, n: u64) -> ElementId {
    ElementId::NamedInteger(SharedString::from(format!("{prefix}-{key}")), n)
}

/// One Auto chip row bound to a field of the shot.
#[allow(clippy::too_many_arguments)]
fn chips<T: Copy + PartialEq + Send + Sync + 'static>(
    v: &Inspect,
    on: &OnAsk,
    id: impl Into<SharedString>,
    options: &[(T, &'static str)],
    value: T,
    pointer: String,
    set: impl Fn(&mut Shot, T) + Send + Sync + 'static,
) -> impl IntoElement {
    let on = on.clone();
    let set = Arc::new(set);
    auto_chips(id, options.iter().copied(), value, v.locked(&pointer)).on_pick(move |pick, w, cx| {
        let p = pointer.clone();
        let ask = match pick {
            None => Ask::auto(vec![p]),
            Some(x) => {
                let set = set.clone();
                Ask::set(vec![p], move |s| set(s, x))
            }
        };
        on(ask, w, cx)
    })
}

/// A chip row bound to a field of one character's mark.
fn mark_chips<T: Copy + PartialEq + Send + Sync + 'static>(
    v: &Inspect,
    on: &OnAsk,
    character: &str,
    field: &str,
    options: &[(T, &'static str)],
    value: T,
    set: impl Fn(&mut Mark, T) + Send + Sync + 'static,
) -> impl IntoElement {
    let who = character.to_string();
    let id = format!("sh-{field}-{character}");
    chips(v, on, id, options, value, mark_ptr(character, field), move |s, x| {
        if let Some(m) = s.marks.iter_mut().find(|m| m.character == who) {
            set(m, x);
        }
    })
}

/// A quiet group of rows with a small header.
fn group(title: &str) -> Div {
    div().flex().flex_col().gap(px(space::SM)).child(div().pt(px(space::XS)).child(label_caps(title)))
}

fn note(text: impl Into<SharedString>) -> Div {
    div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_3()).whitespace_normal().child(text.into())
}

// ---------------------------------------------------------------- the inspector

/// The inspector of one shot: the column header and the scrolling body.
pub(super) fn render(v: Inspect, on: OnAsk) -> Div {
    let shot = v.shot;
    let head = column_head(&format!("Shot {}", v.number))
        .when(!shot.locked.is_empty() && !v.busy, |h| {
            let on = on.clone();
            let all: Vec<String> = shot.locked.iter().cloned().collect();
            h.child(
                btn("sh-reset-all", "All to Auto", ButtonKind::Ghost)
                    .small()
                    .icon(ph::SPARKLE)
                    .tooltip("Give every field you set back to the AI")
                    .on_click(move |_, w, cx| on(Ask::auto(all.clone()), w, cx)),
            )
        })
        .when(v.narrow, |h| {
            let on = on.clone();
            h.child(icon_button("sh-close", ph::X, "Close").small().on_click(move |_, w, cx| on(Ask::Close, w, cx)))
        });
    // While the scene is being planned the settings show dimmed and a click changes nothing
    // (the screen ignores it), like the map.
    let settings = div()
        .flex()
        .flex_col()
        .gap(px(space::LG))
        .when(v.busy, |d| d.opacity(0.5))
        .child(about(&v, &on))
        .child(framing(&v, &on))
        .child(lens_and_focus(&v, &on))
        .child(exposure(&v, &on))
        .child(light(&v, &on))
        .child(movement(&v, &on))
        .child(acting(&v, &on))
        .child(lines(&v, &on));
    let body = div()
        .id("sh-inspector")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap(px(space::LG))
        .p(px(space::MD))
        .overflow_y_scroll()
        .when(v.busy, |d| d.child(info_line(ph::HOURGLASS, LineTone::Neutral, v.busy_note)))
        .child(settings)
        .child(muted("Auto: the AI chooses it from the scene. Pick something and it stays yours when the scene is planned again."));
    div().flex().flex_col().size_full().child(head).child(body)
}

/// What the shot tells, where it comes from, and its length.
fn about(v: &Inspect, on: &OnAsk) -> Div {
    let shot = v.shot;
    let tells = match v.tells {
        Some(input) => div()
            .px(px(space::SM))
            .py(px(space::XXS))
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border_strong())
            .child(Textarea::new(input).appearance(false).bordered(false))
            .into_any_element(),
        None => div().text_size(px(size::SM)).text_color(Tokens::text_1()).child(shot.tells.clone()).into_any_element(),
    };
    let origin = match shot.origin {
        Origin::Anime => "Added for the anime",
        _ => "From the manga",
    };
    let pattern = shot.pattern.and_then(patterns::def).map(|d| d.name.replace('_', " "));
    let secs = shot.locked_s.filter(|s| s.is_finite() && *s > 0.).unwrap_or(shot.planned_s);
    let length = {
        let (on_auto, on_less, on_more) = (on.clone(), on.clone(), on.clone());
        let voiced = shot.locked_s.is_some_and(|s| s.is_finite() && s > 0.);
        div()
            .flex()
            .items_center()
            .gap(px(space::XS))
            .child(auto_chips("sh-length", Vec::<(u8, &str)>::new(), 0u8, v.locked("/planned_s")).on_pick(move |_, w, cx| on_auto(Ask::auto(vec!["/planned_s".into()]), w, cx)))
            .child(icon_button("sh-shorter", ph::MINUS, "Shorter by half a second").small().on_click(move |_, w, cx| {
                on_less(
                    Ask::set(vec!["/planned_s".into()], |s| s.planned_s = (s.planned_s - LENGTH_STEP_S).max(mv_anime::pacing::CUT_MIN_S)),
                    w,
                    cx,
                )
            }))
            .child(spec(format!("{secs:.1} s")))
            .child(icon_button("sh-longer", ph::PLUS, "Longer by half a second").small().on_click(move |_, w, cx| {
                on_more(Ask::set(vec!["/planned_s".into()], |s| s.planned_s = (s.planned_s + LENGTH_STEP_S).min(mv_anime::pacing::CUT_MAX_S)), w, cx)
            }))
            .when(voiced, |d| d.child(note("set by the voices")))
    };
    div()
        .flex()
        .flex_col()
        .gap(px(space::SM))
        .child(div().flex().flex_col().gap(px(space::XS)).child(label_caps("What it tells")).child(tells))
        .child(div().flex().flex_wrap().gap(px(space::XS)).child(spec(origin)).when_some(pattern, |d, p| d.child(spec(format!("Pattern: {p}")))))
        .child(inspector_row("Length", length))
}

fn framing(v: &Inspect, on: &OnAsk) -> Div {
    let c = &v.shot.camera;
    let height = geometry::cam_pos(c, v.place)[1];
    let place = v.place.cloned();
    let stations: Vec<(usize, String)> = v.place.map(|p| p.stations.iter().enumerate().map(|(i, s)| (i, station_label(s.name.trim(), &s.id, s.lens_mm))).collect()).unwrap_or_default();
    let ids: Vec<String> = v.place.map(|p| p.stations.iter().map(|s| s.id.clone()).collect()).unwrap_or_default();
    let at = c.station.as_ref().and_then(|s| ids.iter().position(|x| x == s)).unwrap_or(usize::MAX);
    let position = if stations.is_empty() {
        note("The place has no camera positions yet. Drag the camera on the map.").into_any_element()
    } else {
        let on = on.clone();
        let pointers: Vec<String> = POSITION.iter().map(|p| p.to_string()).collect();
        auto_chips("sh-station", stations, at, v.locked("/camera/station") || v.locked("/camera/pos"))
            .on_pick(move |pick: Option<usize>, w, cx| match pick {
                None => on(Ask::auto(pointers.clone()), w, cx),
                Some(i) => {
                    if let Some(id) = ids.get(i) {
                        on(Ask::Station(id.clone()), w, cx)
                    }
                }
            })
            .into_any_element()
    };
    group("Framing")
        .child(inspector_row("Size", chips(v, on, "sh-size", &SIZES, c.size, "/camera/size".into(), |s, x| s.camera.size = x)))
        .child(inspector_row("Angle", chips(v, on, "sh-angle", &ANGLES, c.angle, "/camera/angle".into(), |s, x| s.camera.angle = x)))
        .child(inspector_row("Height", height_row(v, on, height, place)))
        .child(inspector_row("Position", position))
        .child(note("Or drag the camera on the map; its round handle turns it."))
        .child(inspector_row("Behind", chips(v, on, "sh-background", &BACKGROUNDS, v.shot.background, "/background".into(), |s, x| s.background = x)))
}

/// The Height chips: bound to the height alone, so Auto here never undoes a move on the map.
fn height_row(v: &Inspect, on: &OnAsk, height: f32, place: Option<Place>) -> impl IntoElement {
    let on = on.clone();
    auto_chips("sh-height", HEIGHTS.iter().copied(), height_option(height), v.shot.locked.contains(HEIGHT)).on_pick(move |pick: Option<u16>, w, cx| {
        let place = place.clone();
        let ask = match pick {
            Some(cm) => Ask::set(vec![HEIGHT.into()], move |s| {
                let p = geometry::cam_pos(&s.camera, place.as_ref());
                s.camera.pos = Some([p[0], cm as f32 / 100., p[2]]);
            }),
            None => Ask::Edit { pointers: Vec::new(), set: Some(Arc::new(move |s: &mut Shot| height_auto(s, place.as_ref()))) },
        };
        on(ask, w, cx)
    })
}

fn station_label(name: &str, id: &str, lens: u16) -> String {
    let name = if name.is_empty() { id } else { name };
    if lens > 0 {
        format!("{name} · {lens} mm")
    } else {
        name.to_string()
    }
}

/// Focus targets offered: the characters in the shot, then the background.
fn focus_options(v: &Inspect) -> Vec<(FocusTarget, String)> {
    let mut out: Vec<(FocusTarget, String)> = v.shot.marks.iter().filter(|m| m.visible != Visibility::Offscreen).map(|m| (FocusTarget::Character(m.character.clone()), v.name(&m.character))).collect();
    out.push((FocusTarget::Background, "Background".into()));
    out
}

fn lens_and_focus(v: &Inspect, on: &OnAsk) -> Div {
    let c = &v.shot.camera;
    let lenses: Vec<(u16, &'static str)> = vec![(14, "14 mm"), (24, "24 mm"), (35, "35 mm"), (50, "50 mm"), (85, "85 mm"), (135, "135 mm")];
    let lens = patterns::snap_lens(c.lens_mm as f32);
    let options = focus_options(v);
    let at = options.iter().position(|(t, _)| *t == c.focus.target).unwrap_or(usize::MAX);
    let targets: Vec<FocusTarget> = options.iter().map(|(t, _)| t.clone()).collect();
    let focus = {
        let on = on.clone();
        let targets = targets.clone();
        auto_chips("sh-focus", options.iter().enumerate().map(|(i, (_, l))| (i, l.clone())).collect::<Vec<_>>(), at, v.locked("/camera/focus/target")).on_pick(move |pick: Option<usize>, w, cx| {
            let p = vec!["/camera/focus/target".to_string()];
            match pick.and_then(|i| targets.get(i).cloned()) {
                None => on(Ask::auto(p), w, cx),
                Some(t) => on(Ask::set(p, move |s| s.camera.focus.target = t.clone()), w, cx),
            }
        })
    };
    let rack = (c.movement.kind == Move::RackFocus).then(|| {
        let on = on.clone();
        let targets = targets.clone();
        let at = c.focus.rack_to.as_ref().and_then(|r| targets.iter().position(|t| t == r)).unwrap_or(usize::MAX);
        inspector_row(
            "Rack to",
            auto_chips("sh-rack", options.iter().enumerate().map(|(i, (_, l))| (i, l.clone())).collect::<Vec<_>>(), at, v.locked("/camera/focus/rack_to")).on_pick(move |pick: Option<usize>, w, cx| {
                let p = vec!["/camera/focus/rack_to".to_string()];
                match pick.and_then(|i| targets.get(i).cloned()) {
                    None => on(Ask::auto(p), w, cx),
                    Some(t) => on(Ask::set(p, move |s| s.camera.focus.rack_to = Some(t.clone())), w, cx),
                }
            }),
        )
    });
    group("Lens and focus")
        .child(inspector_row("Lens", chips(v, on, "sh-lens", &lenses, lens, "/camera/lens_mm".into(), |s, x| s.camera.lens_mm = x)))
        .child(note(lens_text(lens)))
        .child(inspector_row("Focus on", focus))
        .children(rack)
        .child(inspector_row("Depth", chips(v, on, "sh-depth", &DEPTHS, c.depth, "/camera/depth".into(), |s, x| s.camera.depth = x)))
}

fn exposure(v: &Inspect, on: &OnAsk) -> Div {
    let c = &v.shot.camera;
    group("Exposure")
        .child(inspector_row("Key", chips(v, on, "sh-key", &KEYS, c.key, "/camera/key".into(), |s, x| s.camera.key = x)))
        .child(inspector_row("Grain", chips(v, on, "sh-grain", &GRAINS, c.grain, "/camera/grain".into(), |s, x| s.camera.grain = x)))
        .child(inspector_row("Glow", chips(v, on, "sh-glow", &GLOWS, c.glow, "/camera/glow".into(), |s, x| s.camera.glow = x)))
        .child(note(ISO_NOTE))
}

fn light(v: &Inspect, on: &OnAsk) -> Div {
    let light = v.shot.camera.light;
    let shown = if light == LightSetup::Unknown { LightSetup::Auto } else { light };
    group("Light").child(inspector_row("Setup", chips(v, on, "sh-light", &LIGHTS, shown, "/camera/light".into(), |s, x| s.camera.light = x))).child(note("Its direction and colour come from the place, so every shot of the scene is lit from the same side."))
}

fn movement(v: &Inspect, on: &OnAsk) -> Div {
    let c = &v.shot.camera;
    let kind = if c.movement.kind == Move::Unknown { Move::Hold } else { c.movement.kind };
    group("Movement")
        .child(inspector_row("Move", chips(v, on, "sh-move", &MOVES, kind, "/camera/movement/kind".into(), |s, x| s.camera.movement.kind = x)))
        .when(kind != Move::Hold, |g| g.child(inspector_row("Speed", chips(v, on, "sh-speed", &SPEEDS, c.movement.speed, "/camera/movement/speed".into(), |s, x| s.camera.movement.speed = x))))
        .child(inspector_row("Motion", chips(v, on, "sh-motion", &MOTIONS, v.shot.motion, "/motion".into(), |s, x| s.motion = x)))
        .child(info_line(ph::FILM_STRIP, LineTone::Neutral, move_note(kind, v.shot.motion)))
}

fn acting(v: &Inspect, on: &OnAsk) -> Div {
    let shot = v.shot;
    let mut g = group("Acting").child(inspector_row("Mouth", chips(v, on, "sh-mouth", &MOUTHS, shot.mouth, "/mouth".into(), |s, x| s.mouth = x)));
    let missing: Vec<&String> = v.cast.iter().filter(|id| !shot.marks.iter().any(|m| &m.character == *id)).collect();
    let add = (!missing.is_empty()).then(|| {
        let items: Vec<MenuItem> = missing
            .iter()
            .map(|id| {
                let (on, id) = (on.clone(), (*id).clone());
                MenuItem::new(v.name(&id), move |w, cx| on(Ask::AddCharacter(id.clone()), w, cx))
            })
            .collect();
        menu_button("sh-add-char", ph::USER_PLUS, "Put someone from the scene into this shot", items).small().anchor(Anchor::TopRight)
    });
    if shot.marks.is_empty() {
        return g.child(div().flex().items_center().gap(px(space::SM)).child(div().flex_1().child(note("Nobody is in this shot."))).children(add));
    }
    let current = v.character.filter(|c| shot.marks.iter().any(|m| m.character == *c)).or_else(|| shot.marks.first().map(|m| m.character.as_str())).unwrap_or_default().to_string();
    let mut people = div().flex().flex_wrap().items_center().gap(px(space::XS));
    for m in &shot.marks {
        let (on, id) = (on.clone(), m.character.clone());
        let seen = m.visible != Visibility::Offscreen;
        people = people.child(
            filter_chip(eid("sh-who", &m.character, 0), v.name(&m.character), m.character == current)
                .when(!seen, |c| c.icon(ph::EYE_SLASH))
                .on_click(move |_, w, cx| on(Ask::Character(id.clone()), w, cx)),
        );
    }
    g = g.child(people.children(add));
    let Some(m) = shot.marks.iter().find(|m| m.character == current) else { return g };
    let who = m.character.clone();
    let head = div()
        .flex()
        .items_center()
        .gap(px(space::SM))
        .child(marker(v.hue(&who), &v.name(&who), 22.))
        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(v.name(&who)));
    // Strength: three dots; a click sets it (and makes the feeling the user's).
    let strength = m.emotion.strength.clamp(1, 3);
    let mut dots = div().flex().flex_none().items_center();
    for k in 1..=3u8 {
        let (on, id) = (on.clone(), who.clone());
        dots = dots.child(
            div()
                .id(eid("sh-strength", &who, k as u64))
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.))
                .cursor_pointer()
                .tooltip(tip(STRENGTHS[k as usize - 1]))
                .child(div().size(px(7.)).rounded_full().bg(if k <= strength { Tokens::accent() } else { Tokens::track() }))
                .on_click(move |_, w, cx| {
                    let id2 = id.clone();
                    on(
                        Ask::set(vec![mark_ptr(&id, "emotion")], move |s| {
                            if let Some(m) = s.marks.iter_mut().find(|m| m.character == id2) {
                                m.emotion.strength = k;
                            }
                        }),
                        w,
                        cx,
                    )
                }),
        );
    }
    let action = match v.action {
        Some(input) => div()
            .px(px(space::SM))
            .py(px(space::XXS))
            .rounded(px(radius::MD))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border_strong())
            .child(Textarea::new(input).appearance(false).bordered(false))
            .into_any_element(),
        None => note(if m.action.trim().is_empty() { "Nothing set".to_string() } else { m.action.trim().to_string() }).into_any_element(),
    };
    let action = div()
        .flex()
        .items_center()
        .gap(px(space::XS))
        .child(div().flex_1().min_w_0().child(action))
        .when(v.locked(&mark_ptr(&who, "action")), |d| {
            let (on, id) = (on.clone(), who.clone());
            d.child(icon_button(eid("sh-action-auto", &who, 0), ph::SPARKLE, "Back to Auto").small().on_click(move |_, w, cx| on(Ask::auto(vec![mark_ptr(&id, "action")]), w, cx)))
        });
    g.child(head)
        .child(inspector_row("Seen", mark_chips(v, on, &who, "visible", &SEEN, m.visible, |m, x| m.visible = x)))
        .child(inspector_row("Pose", mark_chips(v, on, &who, "pose", &POSES, m.pose, |m, x| m.pose = x)))
        .child(inspector_row("Feeling", mark_chips(v, on, &who, "emotion", &EMOTIONS, m.emotion.kind, |m, x| {
            m.emotion.kind = x;
            m.emotion.strength = m.emotion.strength.clamp(1, 3);
        })))
        .child(inspector_row("Strength", div().flex().items_center().gap(px(space::SM)).child(dots).child(note(STRENGTHS[strength as usize - 1]))))
        .child(inspector_row("Action", action))
        .child(inspector_row("Holding", holding(v, on, m)))
}

/// Props in the character's hands: a toggle per prop, and which hand for each one held.
fn holding(v: &Inspect, on: &OnAsk, m: &Mark) -> Div {
    let who = m.character.clone();
    let pointer = mark_ptr(&who, "holding");
    let props: Vec<(String, String)> = v.bible.props.iter().filter(|p| p.held || p.owner.as_deref() == Some(who.as_str()) || m.holding.iter().any(|h| h.prop == p.id)).map(|p| (p.id.clone(), if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() })).collect();
    if props.is_empty() {
        return note("The bible has no props to hold.");
    }
    let mut out = div().flex().flex_col().gap(px(space::XS));
    let mut row = div().flex().flex_wrap().gap(px(space::XS));
    if v.locked(&pointer) {
        let (on, p) = (on.clone(), pointer.clone());
        row = row.child(auto_chips(format!("sh-hold-auto-{who}"), Vec::<(u8, &str)>::new(), 0u8, true).on_pick(move |_, w, cx| on(Ask::auto(vec![p.clone()]), w, cx)));
    }
    for (pid, name) in &props {
        let held = m.holding.iter().any(|h| &h.prop == pid);
        let (on, id, pid2, p) = (on.clone(), who.clone(), pid.clone(), pointer.clone());
        row = row.child(filter_chip(eid("sh-hold", &format!("{who}-{pid}"), 0), name.clone(), held).on_click(move |_, w, cx| {
            let (id, pid) = (id.clone(), pid2.clone());
            on(
                Ask::set(vec![p.clone()], move |s| {
                    if let Some(m) = s.marks.iter_mut().find(|m| m.character == id) {
                        if m.holding.iter().any(|h| h.prop == pid) {
                            m.holding.retain(|h| h.prop != pid);
                        } else {
                            m.holding.push(Hold { prop: pid.clone(), hand: Hand::Right });
                        }
                    }
                }),
                w,
                cx,
            )
        }));
    }
    out = out.child(row);
    for h in &m.holding {
        let name = props.iter().find(|(id, _)| *id == h.prop).map(|(_, n)| n.clone()).unwrap_or_else(|| h.prop.clone());
        let at = HANDS.iter().position(|(x, _)| *x == h.hand).unwrap_or(1);
        let (on, id, pid, p) = (on.clone(), who.clone(), h.prop.clone(), pointer.clone());
        out = out.child(
            div()
                .flex()
                .items_center()
                .gap(px(space::SM))
                .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(name))
                .child(segmented(eid("sh-hand", &format!("{who}-{}", h.prop), 0), HANDS.iter().map(|(_, l)| *l), at).small().on_change(move |i, w, cx| {
                    let Some((hand, _)) = HANDS.get(*i).copied() else { return };
                    let (id, pid) = (id.clone(), pid.clone());
                    on(
                        Ask::set(vec![p.clone()], move |s| {
                            if let Some(h) = s.marks.iter_mut().find(|m| m.character == id).and_then(|m| m.holding.iter_mut().find(|h| h.prop == pid)) {
                                h.hand = hand;
                            }
                        }),
                        w,
                        cx,
                    )
                })),
        );
    }
    out
}

/// The shot's lines, each movable to the shot before or after.
fn lines(v: &Inspect, on: &OnAsk) -> Div {
    let mut g = group("Lines");
    let ls: Vec<&Line> = v.shot.lines.iter().filter_map(|id| v.lines.iter().find(|l| &l.id == id)).collect();
    if ls.is_empty() {
        return g.child(note("No lines in this shot."));
    }
    for l in ls {
        let who = match (&l.speaker, l.kind) {
            (Some(id), _) => v.name(id),
            (None, LineKind::Sfx) => "Sound".to_string(),
            (None, _) => "Narration".to_string(),
        };
        let text = if l.spoken.trim().is_empty() { l.original.trim() } else { l.spoken.trim() };
        let (on_back, on_next) = (on.clone(), on.clone());
        let (a, b) = (l.id.clone(), l.id.clone());
        g = g.child(
            div()
                .flex()
                .items_start()
                .gap(px(space::XS))
                .child(div().pt(px(2.)).child(icon_ph(ph::CHAT_CIRCLE, Tokens::text_3(), icon_px::XS)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_2()).child(who))
                        .child(div().text_size(px(size::XS)).line_height(px(16.)).text_color(Tokens::text_1()).whitespace_normal().child(format!("\u{201c}{text}\u{201d}"))),
                )
                .child(
                    icon_button(eid("sh-line-back", &l.id, 0), ph::ARROW_LEFT, "Move to the shot before")
                        .small()
                        .when(v.first, |b| b.disabled("This is the first shot"))
                        .on_click(move |_, w, cx| on_back(Ask::MoveLine { line: a.clone(), forward: false }, w, cx)),
                )
                .child(
                    icon_button(eid("sh-line-next", &l.id, 0), ph::ARROW_RIGHT, "Move to the shot after")
                        .small()
                        .when(v.last, |b| b.disabled("This is the last shot"))
                        .on_click(move |_, w, cx| on_next(Ask::MoveLine { line: b.clone(), forward: true }, w, cx)),
                ),
        );
    }
    g
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{height_auto, height_option, lens_text, lock, mark_ptr, move_note, reset, value_at, HEIGHT};
    use mv_anime::{Camera, EmotionKind, Mark, MotionKind, Move, Shot, ShotSize};

    fn shot() -> Shot {
        let mut s = Shot { id: "sh0001".into(), camera: Camera { size: ShotSize::Ms, lens_mm: 50, ..Camera::default() }, ..Shot::default() };
        s.marks.push(Mark { character: "jin".into(), pos: [1., 2.], ..Mark::default() });
        s.suggested = serde_json::to_value(&s).unwrap_or_default();
        s
    }

    #[test]
    fn a_pick_is_kept_and_auto_takes_the_suggestion_back() {
        let mut s = shot();
        s.camera.lens_mm = 135;
        lock(&mut s, "/camera/lens_mm");
        assert!(s.edited && s.locked.contains("/camera/lens_mm"));
        reset(&mut s, "/camera/lens_mm");
        assert_eq!(s.camera.lens_mm, 50);
        assert!(s.locked.is_empty());
    }

    #[test]
    fn marks_are_found_by_character() {
        let mut s = shot();
        let p = mark_ptr("jin", "pos");
        assert_eq!(p, "/marks/jin/pos");
        assert!(value_at(&s.suggested, &p).is_some());
        s.marks[0].pos = [4., 4.];
        s.marks[0].emotion.kind = EmotionKind::Fear;
        lock(&mut s, &p);
        lock(&mut s, &mark_ptr("jin", "emotion"));
        reset(&mut s, &p);
        assert_eq!(s.marks[0].pos, [1., 2.]);
        // The other field stays the user's.
        assert_eq!(s.marks[0].emotion.kind, EmotionKind::Fear);
        assert!(s.locked.contains(&mark_ptr("jin", "emotion")));
    }

    #[test]
    fn auto_without_a_suggestion_only_unlocks() {
        let mut s = shot();
        s.suggested = serde_json::Value::Null;
        s.camera.size = ShotSize::Cu;
        lock(&mut s, "/camera/size");
        reset(&mut s, "/camera/size");
        assert_eq!(s.camera.size, ShotSize::Cu);
        assert!(s.locked.is_empty());
    }

    #[test]
    fn resetting_a_parent_clears_the_locks_under_it() {
        let mut s = shot();
        lock(&mut s, "/camera/movement/kind");
        lock(&mut s, "/camera/movement/speed");
        reset(&mut s, "/camera/movement");
        assert!(s.locked.is_empty());
    }

    #[test]
    fn height_auto_keeps_a_camera_moved_on_the_map() {
        let mut s = shot();
        s.camera.pos = Some([1., 1.5, 2.]);
        s.suggested = serde_json::to_value(&s).unwrap_or_default();
        // Moved on the map, then raised by hand.
        s.camera.pos = Some([3., 3., 4.]);
        lock(&mut s, "/camera/pos");
        lock(&mut s, HEIGHT);
        height_auto(&mut s, None);
        assert_eq!(s.camera.pos, Some([3., 1.5, 4.]), "the plan's height, the map's spot");
        assert!(s.locked.contains("/camera/pos") && !s.locked.contains(HEIGHT));
        // Only raised by hand: the whole position is the plan's again.
        let mut t = shot();
        t.camera.pos = Some([1., 1.5, 2.]);
        t.suggested = serde_json::to_value(&t).unwrap_or_default();
        t.camera.pos = Some([1., 3., 2.]);
        lock(&mut t, HEIGHT);
        height_auto(&mut t, None);
        assert_eq!(t.camera.pos, Some([1., 1.5, 2.]));
        assert!(t.locked.is_empty());
    }

    #[test]
    fn camera_words_read_plainly() {
        assert!(lens_text(35).starts_with("35 mm sees 54°"));
        assert!(lens_text(135).contains("very long"));
        assert_eq!(move_note(Move::Hold, MotionKind::Hold), "The camera does not move.");
        assert!(move_note(Move::PushIn, MotionKind::Full).contains("Animate"));
        assert_eq!(height_option(1.4), 150);
        assert_eq!(height_option(0.1), 20);
        assert_eq!(height_option(f32::NAN), 150);
        assert_eq!(height_option(12.), 600);
    }
}
