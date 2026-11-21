//! The place editor of the Bible step (design 9.6) [bible]: drag landmarks and stations, turn a
//! station or a landmark, lens chips, add / remove, size steppers, and the light states with the
//! direction their key light comes from. Paints through `crate::map`. W3-S2.
//!
//! The editor keeps a copy of the place for smooth drags and hands every finished change to the
//! Bible screen as a [`PlaceOp`] (a function on the place), which the screen applies to a fresh
//! read of the file and saves; the screen then hands the saved place back with [`PlanEditor::set_place`].
//! Every edit also locks its part of the place (`/plan`, `/stations`, `/states`), so building the
//! bible again never undoes it.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::bible::{overhead_station, MAX_STATIONS};
use mv_anime::{Landmark, LandmarkKind, Place, PlaceState, Station};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{btn, confirm_dialog, icon_button, icon_ph, menu_button, ph, size, tag, tip, toast, ButtonKind, MenuItem, Toast, Tokens, Tone};

use crate::map::{MapEvent, MapItem, MapScene, MapView, HINT};
use crate::widgets::{count, info_line, inspector_row, label_caps, label_row, spec, LineTone};

/// Height of the map (and its station legend) in the editor: the room drawn large.
const MAP_H: f32 = 440.;
/// The least the map box may be: the shared map needs this to put its legend under the map at
/// narrow widths.
const MAP_MIN_H: f32 = 360.;
/// The lenses a station can carry (the engine snaps to these).
pub(super) const LENSES: [u16; 6] = [14, 24, 35, 50, 85, 135];
/// Step of the room size steppers (metres).
const ROOM_STEP: f32 = 0.5;
/// Step of the landmark size steppers (metres).
const PIECE_STEP: f32 = 0.1;
/// Step of a station's height (metres).
const HEIGHT_STEP: f32 = 0.1;
const ROOM_MIN: f32 = 2.;
const ROOM_MAX: f32 = 200.;
/// Light states a place can gain, with their colour temperature and mood.
const STATES: [(&str, &str, u32, &str); 5] = [
    ("day", "Day", 5600, "clear, neutral daylight"),
    ("dusk", "Dusk", 3200, "low warm sun, long shadows"),
    ("night", "Night", 7500, "cool moonlight, deep shadows"),
    ("dawn", "Dawn", 4000, "soft pale first light"),
    ("indoor", "Indoor", 3000, "warm lamps, soft falloff"),
];

/// A change to a place, applied by the Bible screen to the saved file.
pub type PlaceOp = Arc<dyn Fn(&mut Place) + Send + Sync>;
/// Hands a finished change to the screen: what failed reads ("The station could not be moved").
type OnEdit = Rc<dyn Fn(&'static str, PlaceOp, &mut Window, &mut App)>;

/// A place change as a [`PlaceOp`].
pub(super) fn pop(f: impl Fn(&mut Place) + Send + Sync + 'static) -> PlaceOp {
    Arc::new(f)
}

// ---------------------------------------------------------------- pure parts

/// The eight compass words for a direction in degrees (0 = north, clockwise).
pub(super) fn compass_word(deg: f32) -> &'static str {
    const WORDS: [&str; 8] = ["north", "north-east", "east", "south-east", "south", "south-west", "west", "north-west"];
    let d = if deg.is_finite() { deg.rem_euclid(360.) } else { 0. };
    WORDS[((d + 22.5) / 45.) as usize % 8]
}

/// The compass point nearest `deg` (0, 45, … 315).
fn compass_snap(deg: f32) -> f32 {
    let d = if deg.is_finite() { deg.rem_euclid(360.) } else { 0. };
    (((d + 22.5) / 45.).floor() * 45.).rem_euclid(360.)
}

/// A landmark kind in words.
pub(super) fn kind_label(k: LandmarkKind) -> &'static str {
    match k {
        LandmarkKind::Door => "Door",
        LandmarkKind::Window => "Window",
        LandmarkKind::Wall => "Wall",
        LandmarkKind::Furniture => "Furniture",
        LandmarkKind::Prop => "Object",
        LandmarkKind::Feature => "Feature",
        LandmarkKind::Light => "Light",
        LandmarkKind::Unknown => "Item",
    }
}

/// Size (w, d) and height of a new landmark of a kind, in metres.
fn default_size(k: LandmarkKind) -> ([f32; 2], f32) {
    match k {
        LandmarkKind::Door => ([1.0, 0.15], 2.1),
        LandmarkKind::Window => ([1.2, 0.15], 1.2),
        LandmarkKind::Wall => ([3.0, 0.2], 2.6),
        LandmarkKind::Furniture => ([1.4, 0.8], 0.9),
        LandmarkKind::Prop => ([0.5, 0.5], 0.8),
        LandmarkKind::Light => ([0.4, 0.4], 2.4),
        LandmarkKind::Feature | LandmarkKind::Unknown => ([1.0, 1.0], 1.0),
    }
}

/// `prefix` plus the first free number ("lm_3").
fn unique_id<'a>(prefix: &str, taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: BTreeSet<&str> = taken.into_iter().collect();
    (1..).map(|n| format!("{prefix}{n}")).find(|id| !taken.contains(id.as_str())).unwrap_or_else(|| format!("{prefix}x"))
}

/// A new landmark of `kind` at the centre of the plan ("Door 2").
fn new_landmark(place: &Place, kind: LandmarkKind) -> Landmark {
    let id = unique_id("lm_", place.plan.landmarks.iter().map(|l| l.id.as_str()));
    let n = place.plan.landmarks.iter().filter(|l| l.kind == kind).count() + 1;
    let (size, h_m) = default_size(kind);
    Landmark { id, name: format!("{} {n}", kind_label(kind)), kind, pos: [0., 0.], size, rot_deg: 0., h_m }
}

/// A new station on the south side of the plan looking north, at eye height.
fn new_station(place: &Place) -> Station {
    let id = unique_id("st_", place.stations.iter().map(|s| s.id.as_str()));
    let n = place.stations.iter().filter(|s| !s.overhead).count() + 1;
    let z = (place.plan.d_m.max(ROOM_MIN) / 2. - 0.5).max(0.);
    Station { id, name: format!("Camera {n}"), pos: [0., 1.5, z], yaw_deg: 0., pitch_deg: 0., lens_mm: 35, overhead: false }
}

/// Resizes the room to `w` x `d` metres, scaling the walls, landmarks and stations with it so the
/// room keeps its layout. An overhead station rises with the room.
fn resize_plan(place: &mut Place, w: f32, d: f32) {
    let (w, d) = (w.clamp(ROOM_MIN, ROOM_MAX), d.clamp(ROOM_MIN, ROOM_MAX));
    let (ow, od) = (place.plan.w_m, place.plan.d_m);
    let fx = if ow > 0.01 { w / ow } else { 1. };
    let fz = if od > 0.01 { d / od } else { 1. };
    for wall in &mut place.plan.walls {
        wall.a = [wall.a[0] * fx, wall.a[1] * fz];
        wall.b = [wall.b[0] * fx, wall.b[1] * fz];
    }
    for l in &mut place.plan.landmarks {
        l.pos = [l.pos[0] * fx, l.pos[1] * fz];
    }
    for s in &mut place.stations {
        s.pos[0] *= fx;
        s.pos[2] *= fz;
        if s.overhead {
            s.pos[1] = w.max(d);
        }
    }
    place.plan.w_m = w;
    place.plan.d_m = d;
}

/// One decimal, without a trailing ".0" for whole numbers ("12 m", "1.5 m").
fn metres(v: f32) -> String {
    let r = (v * 10.).round() / 10.;
    if (r - r.round()).abs() < 0.05 {
        format!("{} m", r.round() as i64)
    } else {
        format!("{r:.1} m")
    }
}

/// Rounds to the step's precision, so repeated steps never drift (0.30000001).
fn step(v: f32, by: f32, lo: f32, hi: f32) -> f32 {
    (((v + by) / PIECE_STEP).round() * PIECE_STEP).clamp(lo, hi)
}

/// A light state from the list of known ones, or a plain one.
fn new_state(id: &str) -> PlaceState {
    match STATES.iter().find(|(k, ..)| *k == id) {
        Some((k, name, kelvin, mood)) => PlaceState { id: (*k).into(), name: (*name).into(), key_dir_deg: 135., kelvin: *kelvin, mood: (*mood).into() },
        None => PlaceState { id: id.into(), name: id.into(), key_dir_deg: 135., kelvin: 5600, mood: String::new() },
    }
}

/// A selectable option chip (lens, compass point): accent tonal when on.
pub(super) fn opt_chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(26.))
        .px(px(9.))
        .rounded(px(radius::SM))
        .border_1()
        .text_size(px(size::SM))
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|d| {
            if on {
                d.bg(Tokens::accent_soft()).border_color(Tokens::accent_border()).text_color(Tokens::accent()).font_weight(FontWeight::MEDIUM)
            } else {
                d.border_color(Tokens::border()).text_color(Tokens::text_2()).hover(|s| s.bg(Tokens::hover()).text_color(Tokens::text_1()))
            }
        })
        .active(|s| s.bg(Tokens::pressed()))
        .child(label.into())
}

// ---------------------------------------------------------------- the editor

/// The name box of the picked item.
struct NameBox {
    item: MapItem,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub struct PlanEditor {
    place: Place,
    map: Entity<MapView>,
    picked: Option<MapItem>,
    read_only: bool,
    on_edit: OnEdit,
    name: Option<NameBox>,
    /// A drag is in progress: a saved place arriving now (a job wrote meanwhile) waits, so the
    /// item under the pointer never jumps back; the drag's own save brings the place up to date.
    dragging: bool,
}

impl PlanEditor {
    pub fn new(place: &Place, read_only: bool, on_edit: impl Fn(&'static str, PlaceOp, &mut Window, &mut App) + 'static, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let map = cx.new(|cx| {
            let mut m = MapView::new(MapScene::floor_plan(place), cx);
            m.set_read_only(read_only, cx);
            m
        });
        cx.subscribe_in(&map, window, |this: &mut PlanEditor, _, ev: &MapEvent, window, cx| this.on_map(ev, window, cx)).detach();
        PlanEditor { place: place.clone(), map, picked: None, read_only, on_edit: Rc::new(on_edit), name: None, dragging: false }
    }

    /// The place as saved (after an edit here, a job, or another place opening).
    pub fn set_place(&mut self, place: &Place, read_only: bool, cx: &mut Context<Self>) {
        if self.dragging && place.id == self.place.id {
            return;
        }
        if place.id != self.place.id {
            self.dragging = false;
            self.picked = None;
            self.name = None;
            self.map.update(cx, |m, cx| m.select(None, cx));
        }
        if *place != self.place || read_only != self.read_only {
            self.place = place.clone();
            self.read_only = read_only;
            let scene = MapScene::floor_plan(place);
            self.map.update(cx, |m, cx| {
                m.set_scene(scene, cx);
                m.set_read_only(read_only, cx);
            });
            if self.picked.as_ref().is_some_and(|p| !self.exists(p)) {
                self.picked = None;
                self.name = None;
            }
            cx.notify();
        }
    }

    fn exists(&self, item: &MapItem) -> bool {
        match item {
            MapItem::Station(id) => self.place.stations.iter().any(|s| &s.id == id),
            MapItem::Landmark(id) => self.place.plan.landmarks.iter().any(|l| &l.id == id),
            _ => false,
        }
    }

    /// Applies `op` here at once (so the map follows) and hands it to the screen to save.
    fn edit(&mut self, what: &'static str, op: PlaceOp, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        op(&mut self.place);
        let scene = MapScene::floor_plan(&self.place);
        self.map.update(cx, |m, cx| m.set_scene(scene, cx));
        let f = self.on_edit.clone();
        window.defer(cx, move |w, cx| f(what, op, w, cx));
        cx.notify();
    }

    fn on_map(&mut self, ev: &MapEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (what, op, done): (&'static str, PlaceOp, bool) = match ev {
            MapEvent::Selected(item) => {
                self.picked = item.clone();
                cx.notify();
                return;
            }
            MapEvent::DeleteRequested(item) => {
                self.remove(item.clone(), window, cx);
                return;
            }
            MapEvent::Moved { item: MapItem::Landmark(id), pos, done } => {
                let (id, pos) = (id.clone(), *pos);
                ("The landmark could not be moved", pop(move |p: &mut Place| {
                    if let Some(l) = p.plan.landmarks.iter_mut().find(|l| l.id == id) {
                        l.pos = pos;
                    }
                    p.locked.insert("/plan".into());
                }), *done)
            }
            MapEvent::Turned { item: MapItem::Landmark(id), yaw_deg, done } => {
                let (id, yaw) = (id.clone(), *yaw_deg);
                ("The landmark could not be turned", pop(move |p: &mut Place| {
                    if let Some(l) = p.plan.landmarks.iter_mut().find(|l| l.id == id) {
                        l.rot_deg = yaw;
                    }
                    p.locked.insert("/plan".into());
                }), *done)
            }
            MapEvent::Moved { item: MapItem::Station(id), pos, done } => {
                let (id, pos) = (id.clone(), *pos);
                ("The camera station could not be moved", pop(move |p: &mut Place| {
                    if let Some(s) = p.stations.iter_mut().find(|s| s.id == id) {
                        (s.pos[0], s.pos[2]) = (pos[0], pos[1]);
                    }
                    p.locked.insert("/stations".into());
                }), *done)
            }
            MapEvent::Turned { item: MapItem::Station(id), yaw_deg, done } => {
                let (id, yaw) = (id.clone(), *yaw_deg);
                ("The camera station could not be turned", pop(move |p: &mut Place| {
                    if let Some(s) = p.stations.iter_mut().find(|s| s.id == id) {
                        s.yaw_deg = yaw;
                    }
                    p.locked.insert("/stations".into());
                }), *done)
            }
            _ => return,
        };
        if self.read_only {
            self.dragging = false;
            return;
        }
        self.dragging = !done;
        if done {
            self.edit(what, op, window, cx);
        } else {
            // A drag in progress: follow it here, save on release.
            op(&mut self.place);
            cx.notify();
        }
    }

    fn pick(&mut self, item: Option<MapItem>, cx: &mut Context<Self>) {
        self.picked = item.clone();
        self.map.update(cx, |m, cx| m.select(item, cx));
        cx.notify();
    }

    // ------------------------------------------------------------ add and remove

    fn add_landmark(&mut self, kind: LandmarkKind, window: &mut Window, cx: &mut Context<Self>) {
        let l = new_landmark(&self.place, kind);
        let id = l.id.clone();
        self.edit(
            "The landmark could not be added",
            pop(move |p: &mut Place| {
                if !p.plan.landmarks.iter().any(|x| x.id == l.id) {
                    p.plan.landmarks.push(l.clone());
                }
                p.locked.insert("/plan".into());
            }),
            window,
            cx,
        );
        self.pick(Some(MapItem::Landmark(id)), cx);
    }

    fn add_station(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.place.stations.len() >= MAX_STATIONS {
            toast::show(cx, Toast::info(format!("A place has {MAX_STATIONS} camera stations at most. Remove one first.")));
            return;
        }
        let s = new_station(&self.place);
        let id = s.id.clone();
        self.edit(
            "The camera station could not be added",
            pop(move |p: &mut Place| {
                if !p.stations.iter().any(|x| x.id == s.id) {
                    p.stations.push(s.clone());
                }
                p.locked.insert("/stations".into());
            }),
            window,
            cx,
        );
        self.pick(Some(MapItem::Station(id)), cx);
    }

    fn add_overhead(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = overhead_station(&self.place.plan);
        let id = s.id.clone();
        self.edit(
            "The overhead station could not be added",
            pop(move |p: &mut Place| {
                if !p.stations.iter().any(|x| x.overhead) {
                    let mut s = s.clone();
                    if p.stations.iter().any(|x| x.id == s.id) {
                        s.id = unique_id("st_overhead_", p.stations.iter().map(|x| x.id.as_str()));
                    }
                    p.stations.push(s);
                }
                p.locked.insert("/stations".into());
            }),
            window,
            cx,
        );
        self.pick(Some(MapItem::Station(id)), cx);
    }

    /// Removes a light state with every background plate drawn for it; asks first when it has
    /// any (approved plates included), like removing a camera station.
    fn remove_state(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.place.states.len() <= 1 {
            return;
        }
        let Some(st) = self.place.states.iter().find(|s| s.id == id) else { return };
        let plates = self.place.plates.iter().filter(|pl| pl.state == st.id).count();
        let raw = if st.name.trim().is_empty() { st.id.trim().to_string() } else { st.name.trim().to_string() };
        let remove = move |this: &mut PlanEditor, w: &mut Window, cx: &mut Context<PlanEditor>| {
            let id = id.clone();
            this.edit(
                "The light state could not be removed",
                pop(move |p: &mut Place| {
                    if p.states.len() > 1 {
                        p.states.retain(|s| s.id != id);
                        p.plates.retain(|pl| pl.state != id);
                    }
                    p.locked.insert("/states".into());
                }),
                w,
                cx,
            );
        };
        if plates == 0 {
            remove(self, window, cx);
            return;
        }
        let mut chars = raw.chars();
        let name = chars.next().map(|c| c.to_uppercase().chain(chars).collect::<String>()).unwrap_or_else(|| "this light state".into());
        let weak = cx.entity().downgrade();
        let remove = Rc::new(remove);
        let body = if plates == 1 { "Its background plate goes with it, even if approved. This cannot be undone.".to_string() } else { format!("Its {plates} background plates go with it, approved ones too. This cannot be undone.") };
        confirm_dialog(format!("Remove {name}?"), body, "Remove", true)
            .icon(ph::TRASH)
            .on_confirm(move |w, cx| {
                let remove = remove.clone();
                let _ = weak.update(cx, |this, cx| remove(this, w, cx));
            })
            .open(window, cx);
    }

    fn remove(&mut self, item: MapItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        match item {
            MapItem::Landmark(id) => {
                let name = self.place.plan.landmarks.iter().find(|l| l.id == id).map(|l| l.name.clone()).unwrap_or_default();
                self.picked = None;
                self.edit(
                    "The landmark could not be removed",
                    pop(move |p: &mut Place| {
                        p.plan.landmarks.retain(|l| l.id != id);
                        p.locked.insert("/plan".into());
                    }),
                    window,
                    cx,
                );
                toast::show(cx, Toast::success(if name.trim().is_empty() { "Landmark removed.".to_string() } else { format!("{} removed.", name.trim()) }));
            }
            MapItem::Station(id) => {
                let Some(st) = self.place.stations.iter().find(|s| s.id == id).cloned() else { return };
                if st.overhead {
                    toast::show(cx, Toast::info("The overhead station stays: every place needs one for its floor-plan view."));
                    return;
                }
                let plates = self.place.plates.iter().filter(|p| p.station == id).count();
                let body = if plates == 1 {
                    "Its background plate goes with it. Shots that stood on it keep their camera where it was.".to_string()
                } else if plates > 1 {
                    format!("Its {plates} background plates go with it. Shots that stood on it keep their camera where it was.")
                } else {
                    "Shots that stood on it keep their camera where it was.".to_string()
                };
                let weak = cx.entity().downgrade();
                confirm_dialog(format!("Remove {}?", if st.name.trim().is_empty() { "this camera station" } else { st.name.trim() }), body, "Remove", true)
                    .icon(ph::TRASH)
                    .on_confirm(move |w, cx| {
                        let id = id.clone();
                        let _ = weak.update(cx, |this, cx| {
                            this.picked = None;
                            this.edit(
                                "The camera station could not be removed",
                                pop(move |p: &mut Place| {
                                    p.stations.retain(|s| s.id != id);
                                    p.plates.retain(|pl| pl.station != id);
                                    p.locked.insert("/stations".into());
                                }),
                                w,
                                cx,
                            );
                        });
                    })
                    .open(window, cx);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ the name box

    fn ensure_name(&mut self, item: &MapItem, value: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(b) = self.name.as_ref().filter(|b| &b.item == item) {
            let focused = b.input.read(cx).focus_handle(cx).contains_focused(window, cx);
            if !focused && b.input.read(cx).value().to_string() != value {
                let input = b.input.clone();
                let v = value.to_string();
                input.update(cx, |s, cx| s.set_value(v, window, cx));
            }
            return b.input.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let v = value.to_string();
        input.update(cx, |s, cx| s.set_value(v, window, cx));
        let it = item.clone();
        let sub = cx.subscribe_in(&input, window, move |this: &mut PlanEditor, input, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let text = input.read(cx).value().split_whitespace().collect::<Vec<_>>().join(" ");
                this.rename(&it, text, window, cx);
            }
        });
        self.name = Some(NameBox { item: item.clone(), input: input.clone(), _sub: sub });
        input
    }

    fn rename(&mut self, item: &MapItem, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        match item.clone() {
            MapItem::Station(id) => {
                if self.place.stations.iter().any(|s| s.id == id && s.name.trim() != text) {
                    self.edit(
                        "The station could not be renamed",
                        pop(move |p: &mut Place| {
                            if let Some(s) = p.stations.iter_mut().find(|s| s.id == id) {
                                s.name = text.clone();
                            }
                            p.locked.insert("/stations".into());
                        }),
                        window,
                        cx,
                    );
                }
            }
            MapItem::Landmark(id) => {
                if self.place.plan.landmarks.iter().any(|l| l.id == id && l.name.trim() != text) {
                    self.edit(
                        "The landmark could not be renamed",
                        pop(move |p: &mut Place| {
                            if let Some(l) = p.plan.landmarks.iter_mut().find(|l| l.id == id) {
                                l.name = text.clone();
                            }
                            p.locked.insert("/plan".into());
                        }),
                        window,
                        cx,
                    );
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- view

/// "−  value  +" for a number.
fn stepper(id: &'static str, value: String, minus: Option<Box<dyn Fn(&mut Window, &mut App)>>, plus: Option<Box<dyn Fn(&mut Window, &mut App)>>) -> Div {
    let dec = icon_button((id, 0usize), ph::MINUS, "Less").small();
    let dec = match minus {
        Some(f) => dec.on_click(move |_, w, cx| f(w, cx)),
        None => dec.disabled("Locked"),
    };
    let inc = icon_button((id, 1usize), ph::PLUS, "More").small();
    let inc = match plus {
        Some(f) => inc.on_click(move |_, w, cx| f(w, cx)),
        None => inc.disabled("Locked"),
    };
    div().flex().items_center().gap(px(space::XS)).child(dec).child(div().min_w(px(56.)).flex().justify_center().child(spec(value))).child(inc)
}

impl PlanEditor {
    /// A stepper whose buttons apply `f(value ± by)` through an edit.
    fn num(&self, id: &'static str, value: f32, by: f32, lo: f32, hi: f32, what: &'static str, f: fn(&mut Place, f32), cx: &mut Context<Self>) -> Div {
        let make = |delta: f32| -> Option<Box<dyn Fn(&mut Window, &mut App)>> {
            if self.read_only {
                return None;
            }
            let weak = cx.entity().downgrade();
            let next = step(value, delta, lo, hi);
            Some(Box::new(move |w: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.edit(what, pop(move |p: &mut Place| f(p, next)), w, cx));
            }))
        };
        stepper(id, metres(value), make(-by), make(by))
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only;
        let kinds = [LandmarkKind::Door, LandmarkKind::Window, LandmarkKind::Furniture, LandmarkKind::Prop, LandmarkKind::Feature, LandmarkKind::Light, LandmarkKind::Wall];
        let items: Vec<MenuItem> = kinds
            .into_iter()
            .map(|k| {
                let weak = cx.entity().downgrade();
                MenuItem::new(kind_label(k), move |w, cx| {
                    let _ = weak.update(cx, |this, cx| this.add_landmark(k, w, cx));
                })
            })
            .collect();
        let full = self.place.stations.len() >= MAX_STATIONS;
        let station = btn("fp-add-station", "Add camera station", ButtonKind::Ghost).small().icon(ph::VIDEO_CAMERA);
        let station = if ro {
            station.disabled("This place is locked")
        } else if full {
            station.disabled(format!("A place has {MAX_STATIONS} camera stations at most"))
        } else {
            station.tooltip("A new camera position; drag it where you want it").on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.add_station(w, cx)))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(label_caps("Floor plan"))
            .child(spec(format!("{} × {}", metres(self.place.plan.w_m), metres(self.place.plan.d_m))))
            .child(spec(count(self.place.stations.len(), "camera station")))
            .child(div().flex_1())
            .when(!ro, |d| d.child(menu_button("fp-add-landmark", ph::PLUS, "Add a door, window, piece of furniture or light", items).label("Add landmark").small()))
            .child(station)
    }

    fn inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only;
        let picked = self.picked.clone();
        let body = match picked {
            Some(MapItem::Station(id)) => match self.place.stations.iter().find(|s| s.id == id).cloned() {
                Some(s) => self.station_inspector(&s, window, cx),
                None => self.room_inspector(cx),
            },
            Some(MapItem::Landmark(id)) => match self.place.plan.landmarks.iter().find(|l| l.id == id).cloned() {
                Some(l) => self.landmark_inspector(&l, window, cx),
                None => self.room_inspector(cx),
            },
            _ => self.room_inspector(cx),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::surface_2())
            .child(body)
            .when(ro, |d| d.child(info_line(ph::LOCK_SIMPLE, LineTone::Neutral, "Locked or being rebuilt: the plan can be looked at, not changed.")))
    }

    fn room_inspector(&self, cx: &mut Context<Self>) -> Div {
        let (w, d) = (self.place.plan.w_m, self.place.plan.d_m);
        let has_overhead = self.place.stations.iter().any(|s| s.overhead);
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(label_row("The room", "Click a landmark or station to change it"))
            .child(inspector_row("Width", self.num("fp-w", w, ROOM_STEP, ROOM_MIN, ROOM_MAX, "The room could not be resized", |p, v| {
                let d = p.plan.d_m;
                resize_plan(p, v, d);
                p.locked.insert("/plan".into());
            }, cx)))
            .child(inspector_row("Depth", self.num("fp-d", d, ROOM_STEP, ROOM_MIN, ROOM_MAX, "The room could not be resized", |p, v| {
                let w = p.plan.w_m;
                resize_plan(p, w, v);
                p.locked.insert("/plan".into());
            }, cx)))
            .when(!has_overhead && !self.read_only, |c| {
                c.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(info_line(ph::WARNING, LineTone::Warning, "Every place needs an overhead station: its plate shows the whole room from above."))
                        .child(div().child(btn("fp-add-overhead", "Add the overhead station", ButtonKind::Secondary).small().icon(ph::PLUS).on_click(cx.listener(|this, _: &ClickEvent, w, cx| this.add_overhead(w, cx))))),
                )
            })
    }

    fn station_inspector(&mut self, s: &Station, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only;
        let item = MapItem::Station(s.id.clone());
        let name = self.ensure_name(&item, s.name.trim(), window, cx);
        let mut lenses = div().flex().flex_wrap().gap(px(space::XS));
        for (i, mm) in LENSES.into_iter().enumerate() {
            let on = s.lens_mm == mm;
            let id = s.id.clone();
            lenses = lenses.child(opt_chip(("fp-lens", i), format!("{mm}"), on).when(!ro && !on, |c| {
                c.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                    let id = id.clone();
                    this.edit(
                        "The lens could not be changed",
                        pop(move |p: &mut Place| {
                            if let Some(s) = p.stations.iter_mut().find(|s| s.id == id) {
                                s.lens_mm = mm;
                            }
                            p.locked.insert("/stations".into());
                        }),
                        w,
                        cx,
                    );
                }))
            }));
        }
        let height = s.pos[1];
        let sid = s.id.clone();
        let height_row = {
            let make = |delta: f32| -> Option<Box<dyn Fn(&mut Window, &mut App)>> {
                if ro {
                    return None;
                }
                let weak = cx.entity().downgrade();
                let next = step(height, delta, 0.1, 60.);
                let id = sid.clone();
                Some(Box::new(move |w: &mut Window, cx: &mut App| {
                    let id = id.clone();
                    let _ = weak.update(cx, |this, cx| {
                        this.edit(
                            "The height could not be changed",
                            pop(move |p: &mut Place| {
                                if let Some(s) = p.stations.iter_mut().find(|s| s.id == id) {
                                    s.pos[1] = next;
                                }
                                p.locked.insert("/stations".into());
                            }),
                            w,
                            cx,
                        )
                    });
                }))
            };
            stepper("fp-height", metres(height), make(-HEIGHT_STEP), make(HEIGHT_STEP))
        };
        let looks = if s.overhead { "Straight down at the whole room".to_string() } else { format!("Looks {} ({:.0}°)", compass_word(s.yaw_deg), s.yaw_deg.rem_euclid(360.)) };
        let remove = btn("fp-remove-station", "Remove station", ButtonKind::DangerGhost).small().icon(ph::TRASH);
        let remove = if ro {
            remove.disabled("This place is locked")
        } else if s.overhead {
            remove.disabled("Every place keeps its overhead station")
        } else {
            let it = item.clone();
            remove.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.remove(it.clone(), w, cx)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(icon_ph(ph::VIDEO_CAMERA, Tokens::accent(), icon_px::SM))
                    .child(label_caps("Camera station"))
                    .when(s.overhead, |d| d.child(tag("Overhead", Tone::Info)))
                    .child(div().flex_1())
                    .child(icon_button("fp-done", ph::X, "Back to the room").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pick(None, cx)))),
            )
            .child(inspector_row("Name", name_box(&name, ro)))
            .child(inspector_row("Lens", lenses.child(div().pt(px(5.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("mm"))))
            .child(inspector_row("Height", height_row))
            .child(inspector_row("Facing", div().pt(px(5.)).text_size(px(size::SM)).text_color(Tokens::text_2()).child(looks)))
            .child(div().child(remove))
    }

    fn landmark_inspector(&mut self, l: &Landmark, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only;
        let item = MapItem::Landmark(l.id.clone());
        let name = self.ensure_name(&item, l.name.trim(), window, cx);
        let kinds = [LandmarkKind::Door, LandmarkKind::Window, LandmarkKind::Wall, LandmarkKind::Furniture, LandmarkKind::Prop, LandmarkKind::Feature, LandmarkKind::Light];
        let mut kind_row = div().flex().flex_wrap().gap(px(space::XS));
        for (i, k) in kinds.into_iter().enumerate() {
            let on = l.kind == k;
            let id = l.id.clone();
            kind_row = kind_row.child(opt_chip(("fp-kind", i), kind_label(k), on).when(!ro && !on, |c| {
                c.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                    let id = id.clone();
                    this.edit(
                        "The landmark could not be changed",
                        pop(move |p: &mut Place| {
                            if let Some(l) = p.plan.landmarks.iter_mut().find(|l| l.id == id) {
                                l.kind = k;
                            }
                            p.locked.insert("/plan".into());
                        }),
                        w,
                        cx,
                    );
                }))
            }));
        }
        let dims = |which: usize, id_s: &'static str, value: f32, cx: &mut Context<Self>| -> Div {
            let make = |delta: f32| -> Option<Box<dyn Fn(&mut Window, &mut App)>> {
                if ro {
                    return None;
                }
                let weak = cx.entity().downgrade();
                let next = step(value, delta, PIECE_STEP, 50.);
                let id = l.id.clone();
                Some(Box::new(move |w: &mut Window, cx: &mut App| {
                    let id = id.clone();
                    let _ = weak.update(cx, |this, cx| {
                        this.edit(
                            "The landmark could not be resized",
                            pop(move |p: &mut Place| {
                                if let Some(l) = p.plan.landmarks.iter_mut().find(|l| l.id == id) {
                                    match which {
                                        0 => l.size[0] = next,
                                        1 => l.size[1] = next,
                                        _ => l.h_m = next,
                                    }
                                }
                                p.locked.insert("/plan".into());
                            }),
                            w,
                            cx,
                        )
                    });
                }))
            };
            stepper(id_s, metres(value), make(-PIECE_STEP), make(PIECE_STEP))
        };
        let width = dims(0, "fp-lw", l.size[0], cx);
        let depth = dims(1, "fp-ld", l.size[1], cx);
        let height = dims(2, "fp-lh", l.h_m, cx);
        let remove = btn("fp-remove-landmark", "Remove landmark", ButtonKind::DangerGhost).small().icon(ph::TRASH);
        let remove = if ro {
            remove.disabled("This place is locked")
        } else {
            let it = item.clone();
            remove.on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.remove(it.clone(), w, cx)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(label_caps("Landmark"))
                    .child(div().flex_1())
                    .child(icon_button("fp-done", ph::X, "Back to the room").small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pick(None, cx)))),
            )
            .child(inspector_row("Name", name_box(&name, ro)))
            .child(inspector_row("Kind", kind_row))
            .child(inspector_row("Width", width))
            .child(inspector_row("Depth", depth))
            .child(inspector_row("Height", height))
            .child(div().child(remove))
    }

    fn lights(&self, cx: &mut Context<Self>) -> Div {
        let ro = self.read_only;
        let mut rows = div().flex().flex_col().gap(px(space::SM));
        for (k, st) in self.place.states.iter().enumerate() {
            let mut dirs = div().flex().flex_wrap().gap(px(space::XXS));
            let current = compass_snap(st.key_dir_deg);
            for (i, word) in ["N", "NE", "E", "SE", "S", "SW", "W", "NW"].into_iter().enumerate() {
                let deg = i as f32 * 45.;
                let on = (current - deg).abs() < 0.5;
                let id = st.id.clone();
                dirs = dirs.child(opt_chip(SharedString::from(format!("fp-dir-{k}-{i}")), word, on).when(!ro && !on, |c| {
                    c.tooltip(tip(format!("Light from the {}", compass_word(deg)))).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| {
                        let id = id.clone();
                        this.edit(
                            "The light could not be changed",
                            pop(move |p: &mut Place| {
                                if let Some(s) = p.states.iter_mut().find(|s| s.id == id) {
                                    s.key_dir_deg = deg;
                                }
                                p.locked.insert("/states".into());
                            }),
                            w,
                            cx,
                        );
                    }))
                }));
            }
            let name = if st.name.trim().is_empty() { st.id.clone() } else { st.name.trim().to_string() };
            let mut cap = name.chars();
            let name = cap.next().map(|c| c.to_uppercase().chain(cap).collect::<String>()).unwrap_or_default();
            let detail = if st.mood.trim().is_empty() { format!("{} K", st.kelvin) } else { format!("{} K · {}", st.kelvin, st.mood.trim()) };
            let removable = !ro && self.place.states.len() > 1;
            let sid = st.id.clone();
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(icon_ph(if st.id == "night" { ph::MOON } else { ph::SUN }, Tokens::warning(), icon_px::SM))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Light from the {} · {detail}", compass_word(st.key_dir_deg))))
                            .when(removable, |d| {
                                d.child(icon_button(SharedString::from(format!("fp-state-x-{k}")), ph::X, "Remove this light state and its plates").small().on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.remove_state(sid.clone(), w, cx))))
                            }),
                    )
                    .child(dirs),
            );
        }
        let missing: Vec<&(&str, &str, u32, &str)> = STATES.iter().filter(|(id, ..)| !self.place.states.iter().any(|s| s.id == *id)).collect();
        let add = (!ro && !missing.is_empty()).then(|| {
            let items: Vec<MenuItem> = missing
                .iter()
                .map(|(id, name, ..)| {
                    let weak = cx.entity().downgrade();
                    let id = (*id).to_string();
                    MenuItem::new(*name, move |w, cx| {
                        let st = new_state(&id);
                        let _ = weak.update(cx, |this, cx| {
                            this.edit(
                                "The light state could not be added",
                                pop(move |p: &mut Place| {
                                    if !p.states.iter().any(|s| s.id == st.id) {
                                        p.states.push(st.clone());
                                    }
                                    p.locked.insert("/states".into());
                                }),
                                w,
                                cx,
                            )
                        });
                    })
                })
                .collect();
            menu_button("fp-add-state", ph::PLUS, "Add a light state", items).label("Add light").small()
        });
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(div().flex().items_center().gap(px(space::SM)).child(label_caps("Light")).child(div().flex_1()).children(add))
            .child(rows)
    }
}

/// The name box in the inspector's frame.
fn name_box(input: &Entity<InputState>, read_only: bool) -> Div {
    div()
        .px(px(space::SM))
        .rounded(px(radius::MD))
        .bg(Tokens::surface())
        .border_1()
        .border_color(Tokens::border_strong())
        .when(read_only, |d| d.opacity(0.6))
        .child(Input::new(input).appearance(false).bordered(false).disabled(read_only))
}

impl Render for PlanEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toolbar = self.toolbar(cx);
        let inspector = self.inspector(window, cx);
        let lights = self.lights(cx);
        // The map draws its own frame and the numbered station legend beside it.
        // Never squeezed below the height the map needs to put its legend underneath at narrow
        // widths; it is taller when there is room (the place body scrolls).
        let map = div().flex_none().w_full().h(px(MAP_H)).min_h(px(MAP_MIN_H)).child(self.map.clone());
        let hint = if self.read_only { "Locked: the plan can be looked at, not changed." } else { HINT };
        div()
            .flex()
            .flex_col()
            .gap(px(space::MD))
            .child(toolbar)
            .child(map)
            .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child(hint))
            .child(inspector)
            .child(lights)
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{compass_snap, compass_word, metres, new_landmark, new_state, new_station, resize_plan, step, unique_id};
    use mv_anime::{FloorPlan, Landmark, LandmarkKind, Place, Station, Wall};

    fn room() -> Place {
        Place {
            id: "pl_hall".into(),
            plan: FloorPlan {
                w_m: 10.,
                d_m: 8.,
                walls: vec![Wall { a: [-5., -4.], b: [5., -4.], h_m: 3. }],
                landmarks: vec![Landmark { id: "lm_1".into(), name: "Door 1".into(), kind: LandmarkKind::Door, pos: [2., 4.], ..Default::default() }],
                north_deg: 0.,
            },
            stations: vec![
                Station { id: "st_1".into(), pos: [5., 1.5, 4.], ..Default::default() },
                Station { id: "st_overhead".into(), pos: [0., 10., 0.], overhead: true, pitch_deg: -90., ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn compass_words() {
        assert_eq!(compass_word(0.), "north");
        assert_eq!(compass_word(44.), "north-east");
        assert_eq!(compass_word(135.), "south-east");
        assert_eq!(compass_word(-90.), "west");
        assert_eq!(compass_word(359.), "north");
        assert_eq!(compass_word(f32::NAN), "north");
        assert_eq!(compass_snap(130.), 135.);
        assert_eq!(compass_snap(350.), 0.);
    }

    #[test]
    fn new_items_get_free_ids_and_plain_names() {
        let p = room();
        assert_eq!(unique_id("lm_", ["lm_1", "lm_3"]), "lm_2");
        let l = new_landmark(&p, LandmarkKind::Door);
        assert_eq!(l.id, "lm_2");
        assert_eq!(l.name, "Door 2");
        assert!(l.size[0] > 0. && l.h_m > 0.);
        let s = new_station(&p);
        assert_eq!(s.id, "st_2");
        assert_eq!(s.name, "Camera 2");
        assert_eq!(s.lens_mm, 35);
        assert!(!s.overhead);
        assert!(s.pos[2] > 0., "starts on the south side, looking north");
    }

    #[test]
    fn resizing_keeps_the_layout() {
        let mut p = room();
        resize_plan(&mut p, 20., 4.);
        assert_eq!((p.plan.w_m, p.plan.d_m), (20., 4.));
        assert_eq!(p.plan.walls[0].a, [-10., -2.]);
        assert_eq!(p.plan.landmarks[0].pos, [4., 2.]);
        assert_eq!((p.stations[0].pos[0], p.stations[0].pos[2]), (10., 2.));
        assert_eq!(p.stations[0].pos[1], 1.5, "a camera keeps its height");
        assert_eq!(p.stations[1].pos[1], 20., "the overhead rises with the room");
        resize_plan(&mut p, 0., 1_000.);
        assert_eq!((p.plan.w_m, p.plan.d_m), (2., 200.));
    }

    #[test]
    fn numbers_read_plainly_and_steps_do_not_drift() {
        assert_eq!(metres(12.), "12 m");
        assert_eq!(metres(1.5), "1.5 m");
        assert_eq!(metres(0.3000001), "0.3 m");
        assert_eq!(step(0.2, 0.1, 0.1, 5.), 0.3);
        assert_eq!(step(0.1, -0.1, 0.1, 5.), 0.1);
        let mut v = 0.;
        for _ in 0..10 {
            v = step(v, 0.1, 0., 5.);
        }
        assert_eq!(v, 1.);
    }

    #[test]
    fn known_light_states_come_with_their_light() {
        let night = new_state("night");
        assert_eq!(night.name, "Night");
        assert_eq!(night.kelvin, 7500);
        assert!(!night.mood.is_empty());
        let odd = new_state("ruined");
        assert_eq!(odd.name, "ruined");
    }
}
