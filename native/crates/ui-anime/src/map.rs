//! The top-down map shared by the floor-plan editor of a place (design 9.6: walls, landmarks,
//! camera stations) and the camera map of a shot (design 9.7: character marks, the camera and
//! its view cone, the 180-degree line). W2-U2.
//!
//! The owning screen keeps the data; the map shows a copy and reports every change:
//!
//! ```ignore
//! let map = cx.new(|cx| MapView::new(MapScene::floor_plan(&place), cx));
//! cx.subscribe_in(&map, window, |this, _, ev: &MapEvent, _, cx| match ev {
//!     MapEvent::Moved { item, pos, done } => { /* write pos; save when done */ }
//!     MapEvent::Turned { item, yaw_deg, done } => { /* write yaw */ }
//!     _ => {}
//! }).detach();
//! // render: .child(map.clone())   after an edit: map.update(cx, |m, cx| m.set_scene(scene, cx))
//! ```
//!
//! Pure maths (fitting, hit testing, snapping, clipping, label placement) is in `map/geom.rs`,
//! drawing in `map/paint.rs`. Drawing is a `canvas` (design 4.3); presses follow the cropper.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;
use mv_anime::{geometry, Character, FloorPlan, Landmark, Place, Shot, Station, Visibility};
use gpui_kit::prelude::FluentBuilder as _;
use mv_ui::theme::{radius, space};
use mv_ui::{size, Tokens};

mod geom;
mod paint;

pub use geom::P2;
use geom::{Rect, Shape, Target, View};

/// One line of help for a screen to show beside an editable map.
pub const HINT: &str = "Drag to move, drag the round handle to turn; Ctrl + wheel turns the selected item. Shift snaps coarser, Alt moves freely. Arrow keys nudge, [ and ] turn.";

/// Position snap while dragging (metres): normal, with Shift.
pub const MOVE_STEP_M: f32 = 0.1;
pub const COARSE_STEP_M: f32 = 0.5;
/// Turn snap (degrees): normal, with Shift. Alt turns and moves freely.
pub const TURN_STEP_DEG: f32 = 5.;
pub const COARSE_TURN_DEG: f32 = 15.;
/// How far beyond the walls a camera or a station may stand (shooting in through a window).
pub const OUTSIDE_M: f32 = 3.;

/// Room kept around the plan inside the map, in pixels (labels and markers at the walls).
const PAD_PX: f32 = 40.;
/// A press must travel this far before it becomes a drag, so a click never nudges an item.
const DRAG_START_PX: f32 = 3.;
/// Extra reach of every pickable shape.
const TOL_PX: f32 = 4.;
/// Pixels of wheel travel per turn step.
const WHEEL_NOTCH_PX: f32 = 20.;

// Marker sizes on screen (pixels): fixed, so the map stays legible at every zoom.
const CAM_R: f32 = 12.;
/// A station pin (it carries the station's number).
const STATION_R: f32 = 8.;
/// The station legend beside the map, and one of its rows.
const LEGEND_W: f32 = 216.;
const LEGEND_ROW_H: f32 = 28.;
const LEGEND_PIN: f32 = 20.;
/// In a narrow window: the legend under the map (about three rows and its title), and the
/// smallest map height that still reads.
const NARROW_LEGEND_H: f32 = 112.;
const NARROW_MAP_MIN_H: f32 = 240.;
const HANDLE_R: f32 = 6.;
const LANDMARK_MIN_PX: f32 = 12.;

/// A character marker's radius at `scale` px/m: about a person's shoulders, kept legible.
fn mark_r(scale: f32) -> f32 {
    (0.25 * scale).clamp(10., 14.)
}

// ------------------------------------------------------------------------------- scene

/// What the map is for: which items move and turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapKind {
    /// The Bible's place editor: landmarks and stations move and turn.
    #[default]
    FloorPlan,
    /// The Shots step: character marks and the camera move and turn; a click on a station asks
    /// to put the camera there ([`MapEvent::StationPicked`]).
    CameraMap,
}

impl MapKind {
    pub fn movable(self, item: &MapItem) -> bool {
        match self {
            MapKind::FloorPlan => matches!(item, MapItem::Landmark(_) | MapItem::Station(_)),
            MapKind::CameraMap => matches!(item, MapItem::Mark(_) | MapItem::Camera),
        }
    }

    pub fn turnable(self, item: &MapItem) -> bool {
        self.movable(item)
    }
}

/// One thing on the map, by id (character id, landmark id, station id).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MapItem {
    Mark(String),
    Camera,
    Station(String),
    Landmark(String),
}

/// A character standing on the plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapMark {
    /// Character id.
    pub id: String,
    pub name: String,
    /// Two letters drawn inside the marker.
    pub initials: String,
    /// The character's signature colour (0xRRGGBB); used when it reads on the current theme.
    pub colour: Option<u32>,
    /// Index into the theme's categorical hues, the fallback colour (stable per character).
    pub hue: usize,
    pub pos: P2,
    pub facing_deg: f32,
    /// Off screen in this shot: drawn faded.
    pub dimmed: bool,
}

/// The shot's camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapCamera {
    /// x, height, z.
    pub pos: [f32; 3],
    pub yaw_deg: f32,
    /// Up (positive) or down (negative) from level, degrees: a steep one shows its patch of
    /// floor instead of a view cone.
    pub pitch_deg: f32,
    pub lens_mm: u16,
    /// Shown first in its label ("3.4").
    pub label: String,
}

/// Everything the map draws. Build it with [`MapScene::floor_plan`] or [`MapScene::camera_map`]
/// and adjust fields freely (it is plain data).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapScene {
    pub kind: MapKind,
    pub plan: FloorPlan,
    pub stations: Vec<Station>,
    pub marks: Vec<MapMark>,
    pub camera: Option<MapCamera>,
    /// The two character ids the 180-degree line runs through.
    pub axis: Option<[String; 2]>,
}

impl MapScene {
    /// The place editor's map: walls, landmarks and stations of `place`.
    pub fn floor_plan(place: &Place) -> Self {
        MapScene { kind: MapKind::FloorPlan, plan: place.plan.clone(), stations: place.stations.clone(), ..Default::default() }
    }

    /// The camera map of `shot` in `place`: its marks (named and coloured from `cast`, the bible's
    /// characters in their bible order so a character keeps its colour in every shot), its camera
    /// labelled `label` ("3.4"), and the 180-degree line through the first two characters on
    /// screen. Without a place the map shows the marks and camera on an empty grid.
    pub fn camera_map(place: Option<&Place>, shot: &Shot, cast: &[Character], label: impl Into<String>) -> Self {
        let marks: Vec<MapMark> = shot
            .marks
            .iter()
            .map(|m| {
                let ix = cast.iter().position(|c| c.id == m.character);
                let ch = ix.and_then(|i| cast.get(i));
                let name = ch.map(|c| c.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| m.character.clone());
                MapMark {
                    id: m.character.clone(),
                    initials: geom::initials(&name),
                    name,
                    colour: ch.and_then(|c| geom::parse_hex(&c.look.signature_colour)),
                    hue: ix.unwrap_or_else(|| stable_index(&m.character)),
                    pos: geom::fin2(m.pos),
                    facing_deg: geom::norm_deg(m.facing_deg),
                    dimmed: m.visible == Visibility::Offscreen,
                }
            })
            .collect();
        let on_screen: Vec<&MapMark> = marks.iter().filter(|m| !m.dimmed).collect();
        let axis = match on_screen.as_slice() {
            [a, b, ..] => Some([a.id.clone(), b.id.clone()]),
            _ => None,
        };
        let pos = geometry::cam_pos(&shot.camera, place).map(geom::fin);
        let (yaw, pitch) = geometry::look_dir(&shot.camera, &shot.marks, place);
        let camera = MapCamera { pos, yaw_deg: geom::norm_deg(yaw), pitch_deg: geom::fin(pitch).clamp(-90., 90.), lens_mm: shot.camera.lens_mm, label: label.into() };
        MapScene {
            kind: MapKind::CameraMap,
            plan: place.map(|p| p.plan.clone()).unwrap_or_default(),
            stations: place.map(|p| p.stations.clone()).unwrap_or_default(),
            marks,
            camera: Some(camera),
            axis,
        }
    }

    /// A small hall with a door, a window, an altar, a statue, two stations, two characters and
    /// a camera, for the tests.
    #[cfg(test)]
    pub fn sample(kind: MapKind) -> Self {
        use mv_anime::{LandmarkKind, Wall};
        let lm = |id: &str, name: &str, kind: LandmarkKind, pos: P2, size: P2, rot_deg: f32| Landmark { id: id.into(), name: name.into(), kind, pos, size, rot_deg, h_m: 1. };
        let wall = |a: P2, b: P2| Wall { a, b, h_m: 3. };
        let plan = FloorPlan {
            w_m: 10.,
            d_m: 7.,
            walls: vec![wall([-5., -3.5], [5., -3.5]), wall([5., -3.5], [5., 3.5]), wall([5., 3.5], [-5., 3.5]), wall([-5., 3.5], [-5., -3.5])],
            landmarks: vec![
                lm("door", "Door", LandmarkKind::Door, [3., 3.5], [1.2, 0.1], 0.),
                lm("window", "Window", LandmarkKind::Window, [-5., 0.], [2., 0.1], 90.),
                lm("altar", "Altar", LandmarkKind::Furniture, [0., -2.6], [2.4, 0.9], 0.),
                lm("statue", "Statue", LandmarkKind::Feature, [-3.2, -2.4], [0.9, 0.9], 20.),
                lm("lamp", "Brazier", LandmarkKind::Light, [3.6, -2.6], [0.5, 0.5], 0.),
            ],
            north_deg: 0.,
        };
        let st = |id: &str, name: &str, pos: [f32; 3], yaw_deg: f32, lens_mm: u16| Station { id: id.into(), name: name.into(), pos, yaw_deg, pitch_deg: 0., lens_mm, overhead: false };
        let stations = vec![st("st_door", "Door", [3., 1.6, 2.6], 330., 24), st("st_side", "Side", [4.2, 1.5, -0.4], 270., 35)];
        let mk = |id: &str, name: &str, hue: usize, pos: P2, facing_deg: f32| MapMark { id: id.into(), name: name.into(), initials: geom::initials(name), colour: None, hue, pos, facing_deg, dimmed: false };
        match kind {
            MapKind::FloorPlan => MapScene { kind, plan, stations, ..Default::default() },
            MapKind::CameraMap => MapScene {
                kind,
                plan,
                stations,
                marks: vec![mk("jinwoo", "Sung Jinwoo", 0, [-0.8, 0.4], 75.), mk("song", "Song Chiyul", 1, [1., -0.4], 250.)],
                camera: Some(MapCamera { pos: [0.6, 1.4, 2.6], yaw_deg: 345., pitch_deg: 0., lens_mm: 35, label: "3.4".into() }),
                axis: Some(["jinwoo".into(), "song".into()]),
            },
        }
    }

    pub fn mark(&self, id: &str) -> Option<&MapMark> {
        self.marks.iter().find(|m| m.id == id)
    }

    pub fn landmark(&self, id: &str) -> Option<&Landmark> {
        self.plan.landmarks.iter().find(|l| l.id == id)
    }

    pub fn station(&self, id: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.id == id)
    }

    /// Where an item stands on the plan (x, z); `None` when it is not in this scene.
    pub fn pos_of(&self, item: &MapItem) -> Option<P2> {
        match item {
            MapItem::Mark(id) => self.mark(id).map(|m| m.pos),
            MapItem::Camera => self.camera.as_ref().map(|c| [c.pos[0], c.pos[2]]),
            MapItem::Station(id) => self.station(id).map(|s| [s.pos[0], s.pos[2]]),
            MapItem::Landmark(id) => self.landmark(id).map(|l| l.pos),
        }
        .map(geom::fin2)
    }

    /// Which way an item faces (yaw, 0 = up, clockwise); a landmark's turn.
    pub fn yaw_of(&self, item: &MapItem) -> Option<f32> {
        match item {
            MapItem::Mark(id) => self.mark(id).map(|m| m.facing_deg),
            MapItem::Camera => self.camera.as_ref().map(|c| c.yaw_deg),
            MapItem::Station(id) => self.station(id).map(|s| s.yaw_deg),
            MapItem::Landmark(id) => self.landmark(id).map(|l| l.rot_deg),
        }
        .map(geom::norm_deg)
    }

    /// Moves an item (a camera or station keeps its height). False when it is not in the scene.
    pub fn set_pos(&mut self, item: &MapItem, p: P2) -> bool {
        match item {
            MapItem::Mark(id) => self.marks.iter_mut().find(|m| &m.id == id).map(|m| m.pos = p).is_some(),
            MapItem::Camera => self.camera.as_mut().map(|c| (c.pos[0], c.pos[2]) = (p[0], p[1])).is_some(),
            MapItem::Station(id) => self.stations.iter_mut().find(|s| &s.id == id).map(|s| (s.pos[0], s.pos[2]) = (p[0], p[1])).is_some(),
            MapItem::Landmark(id) => self.plan.landmarks.iter_mut().find(|l| &l.id == id).map(|l| l.pos = p).is_some(),
        }
    }

    /// Turns an item. False when it is not in the scene.
    pub fn set_yaw(&mut self, item: &MapItem, yaw: f32) -> bool {
        let yaw = geom::norm_deg(yaw);
        match item {
            MapItem::Mark(id) => self.marks.iter_mut().find(|m| &m.id == id).map(|m| m.facing_deg = yaw).is_some(),
            MapItem::Camera => self.camera.as_mut().map(|c| c.yaw_deg = yaw).is_some(),
            MapItem::Station(id) => self.stations.iter_mut().find(|s| &s.id == id).map(|s| s.yaw_deg = yaw).is_some(),
            MapItem::Landmark(id) => self.plan.landmarks.iter_mut().find(|l| &l.id == id).map(|l| l.rot_deg = yaw).is_some(),
        }
    }

    pub fn contains(&self, item: &MapItem) -> bool {
        self.pos_of(item).is_some()
    }

    /// Where an item may be dragged: inside the room, or a little beyond its walls for the
    /// camera and stations. No limit while the plan has no size.
    fn limits(&self, item: &MapItem) -> Rect {
        match geom::floor(self.plan.w_m, self.plan.d_m) {
            Some(r) if matches!(item, MapItem::Camera | MapItem::Station(_)) => r.inflate(OUTSIDE_M),
            Some(r) => r,
            None => Rect::default(),
        }
    }

    /// Every point the view must show (walls, landmarks, stations, marks, camera).
    fn points(&self) -> Vec<P2> {
        let mut pts: Vec<P2> = self.plan.walls.iter().flat_map(|w| [w.a, w.b]).collect();
        pts.extend(self.plan.landmarks.iter().flat_map(|l| geom::box_corners(l.pos, l.size, l.rot_deg)));
        pts.extend(self.stations.iter().map(|s| [s.pos[0], s.pos[2]]));
        pts.extend(self.marks.iter().map(|m| m.pos));
        pts.extend(self.camera.iter().map(|c| [c.pos[0], c.pos[2]]));
        pts.into_iter().map(geom::fin2).collect()
    }

    /// The plan area the map shows.
    fn extent(&self) -> Rect {
        geom::extent(self.plan.w_m, self.plan.d_m, &self.points())
    }

    /// The two ends of the 180-degree line, when both characters are on the map apart.
    fn axis_points(&self) -> Option<(P2, P2)> {
        let [a, b] = self.axis.as_ref()?;
        let (a, b) = (self.mark(a)?.pos, self.mark(b)?.pos);
        (geom::dist(a, b) > 1e-3).then_some((a, b))
    }

    /// Nothing to draw: no plan size, no walls and no items.
    fn is_empty(&self) -> bool {
        geom::floor(self.plan.w_m, self.plan.d_m).is_none() && self.points().is_empty()
    }
}

/// A colour index that stays the same for a character id not found in the cast.
fn stable_index(id: &str) -> usize {
    id.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}

// ------------------------------------------------------------------------------ events

/// What the map reports. Positions are plan metres (x, z); yaw is degrees, 0 = up, clockwise.
/// A drag reports every step with `done: false` and the drop with `done: true` (save then);
/// keys and the wheel report single steps with `done: true`.
#[derive(Clone, Debug, PartialEq)]
pub enum MapEvent {
    /// The selection changed by a click (or Escape).
    Selected(Option<MapItem>),
    /// An item was moved.
    Moved { item: MapItem, pos: P2, done: bool },
    /// An item was turned (a mark's facing, the camera's or a station's yaw, a landmark's turn).
    Turned { item: MapItem, yaw_deg: f32, done: bool },
    /// Camera map: a station was clicked, put the camera there.
    StationPicked(String),
    /// Delete / Backspace on the selected item (never the camera); the screen decides.
    DeleteRequested(MapItem),
}

// ----------------------------------------------------------------------- interaction

/// What a press takes: an item, or the turn handle of the selected item.
#[derive(Clone, Debug, PartialEq)]
enum Pick {
    Item(MapItem),
    Handle(MapItem),
}

impl Pick {
    fn item(&self) -> &MapItem {
        match self {
            Pick::Item(i) | Pick::Handle(i) => i,
        }
    }
}

/// The snap steps for the held modifiers: (metres, degrees).
fn steps(shift: bool, alt: bool) -> (f32, f32) {
    match (shift, alt) {
        (_, true) => (0., 0.),
        (true, false) => (COARSE_STEP_M, COARSE_TURN_DEG),
        (false, false) => (MOVE_STEP_M, TURN_STEP_DEG),
    }
}

/// Where a dragged item lands: the pointer plus the grab offset, snapped, kept in its limits.
fn moved_to(limits: Rect, pointer: P2, grab: P2, step: f32) -> P2 {
    geom::clamp_in(geom::snap2(geom::add(pointer, grab), step), limits)
}

/// The yaw a turn handle dragged to `pointer` gives the item at `centre`, snapped.
fn turned_to(centre: P2, pointer: P2, step: f32) -> Option<f32> {
    geom::yaw_to(centre, pointer).map(|y| geom::snap_deg(y, step))
}

/// How far from an item's centre its turn handle sits, in pixels.
fn handle_px(scene: &MapScene, item: &MapItem, scale: f32) -> f32 {
    match item {
        MapItem::Mark(_) => mark_r(scale) + 18.,
        MapItem::Camera => CAM_R + 22.,
        MapItem::Station(_) => STATION_R + 22.,
        MapItem::Landmark(id) => scene.landmark(id).map(|l| geom::fin(l.size[0]).abs().max(geom::fin(l.size[1]).abs()) / 2. * scale).unwrap_or(0.) + 18.,
    }
}

/// The turn handle's plan point of the selected item, when it shows.
fn handle_at(scene: &MapScene, selected: Option<&MapItem>, read_only: bool, view: &View) -> Option<(MapItem, P2)> {
    let item = selected.filter(|i| !read_only && scene.kind.turnable(i))?;
    let (c, yaw) = (scene.pos_of(item)?, scene.yaw_of(item)?);
    let (sx, sy) = view.to_screen(c);
    let d = geom::dir(yaw);
    let r = handle_px(scene, item, view.scale);
    Some((item.clone(), view.to_world((sx + d[0] * r, sy + d[1] * r))))
}

/// Everything a press can take, with what it means.
fn targets(scene: &MapScene, selected: Option<&MapItem>, read_only: bool, view: &View) -> Vec<(Target, Pick)> {
    let mut out = Vec::new();
    if let Some((item, h)) = handle_at(scene, selected, read_only, view) {
        out.push((Target { shape: Shape::Disc { c: h, r_px: HANDLE_R + 2. }, layer: 0 }, Pick::Handle(item)));
    }
    // The camera and the characters share a layer, so the nearest centre takes the press: a
    // character standing next to the camera can still be picked.
    if let Some(c) = &scene.camera {
        out.push((Target { shape: Shape::Disc { c: [c.pos[0], c.pos[2]], r_px: CAM_R }, layer: 1 }, Pick::Item(MapItem::Camera)));
    }
    let r = mark_r(view.scale);
    out.extend(scene.marks.iter().map(|m| (Target { shape: Shape::Disc { c: m.pos, r_px: r }, layer: 1 }, Pick::Item(MapItem::Mark(m.id.clone())))));
    out.extend(scene.stations.iter().map(|s| (Target { shape: Shape::Disc { c: [s.pos[0], s.pos[2]], r_px: STATION_R + 2. }, layer: 3 }, Pick::Item(MapItem::Station(s.id.clone())))));
    out.extend(scene.plan.landmarks.iter().map(|l| {
        (Target { shape: Shape::Box { pos: l.pos, size: l.size, rot_deg: l.rot_deg, min_px: LANDMARK_MIN_PX }, layer: 4 }, Pick::Item(MapItem::Landmark(l.id.clone())))
    }));
    out
}

/// What is under screen point `s`.
fn pick_at(scene: &MapScene, selected: Option<&MapItem>, read_only: bool, view: &View, s: (f32, f32)) -> Option<Pick> {
    let t = targets(scene, selected, read_only, view);
    let shapes: Vec<Target> = t.iter().map(|(t, _)| *t).collect();
    geom::hit(&shapes, view, s, TOL_PX).and_then(|i| t.into_iter().nth(i)).map(|(_, p)| p)
}

/// The map's screen rectangle.
fn rect_of(b: Bounds<Pixels>) -> Rect {
    Rect::at(f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
}

/// The view that fits `scene` into `b`.
fn fitted(scene: &MapScene, b: Bounds<Pixels>) -> View {
    geom::fit(scene.extent(), rect_of(b), PAD_PX)
}

struct Drag {
    pick: Pick,
    /// The view at the press: kept while dragging so the map never rescales under the pointer.
    view: View,
    press: (f32, f32),
    /// Item position minus the pressed plan point (a move keeps where the item was taken).
    grab: P2,
    moved: bool,
    /// The scene as the screen last gave it, without this drag: what a cancelled drag goes
    /// back to.
    base: Rc<MapScene>,
}

/// The map component. See the module docs for how a screen embeds it.
pub struct MapView {
    scene: Rc<MapScene>,
    read_only: bool,
    selected: Option<MapItem>,
    hover: Option<Pick>,
    drag: Option<Drag>,
    wheel: f32,
    area: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
}

impl EventEmitter<MapEvent> for MapView {}

impl Focusable for MapView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MapView {
    pub fn new(scene: MapScene, cx: &mut Context<Self>) -> Self {
        MapView { scene: Rc::new(scene), read_only: false, selected: None, hover: None, drag: None, wheel: 0., area: Rc::new(Cell::new(Bounds::default())), focus: cx.focus_handle() }
    }

    /// Shows new data (after the screen wrote an edit, or switched place or shot). A drag in
    /// progress keeps its item where the pointer is; a selection whose item is gone is dropped.
    pub fn set_scene(&mut self, scene: MapScene, cx: &mut Context<Self>) {
        let mut scene = scene;
        if let Some(d) = self.drag.as_mut() {
            d.base = Rc::new(scene.clone());
        }
        if let Some(d) = self.drag.as_ref().filter(|d| d.moved) {
            let item = d.pick.item();
            match d.pick {
                Pick::Item(_) => {
                    if let Some(p) = self.scene.pos_of(item) {
                        scene.set_pos(item, p);
                    }
                }
                Pick::Handle(_) => {
                    if let Some(y) = self.scene.yaw_of(item) {
                        scene.set_yaw(item, y);
                    }
                }
            }
        }
        if self.drag.as_ref().is_some_and(|d| !scene.contains(d.pick.item())) {
            self.drag = None;
        }
        if self.selected.as_ref().is_some_and(|s| !scene.contains(s)) {
            self.selected = None;
        }
        if self.hover.as_ref().is_some_and(|h| !scene.contains(h.item())) {
            self.hover = None;
        }
        self.scene = Rc::new(scene);
        cx.notify();
    }

    pub fn scene(&self) -> &MapScene {
        &self.scene
    }

    /// Read-only: items can be picked and hovered but never moved or turned. A drag still open
    /// is cancelled: its item goes back to where the screen last put it, and nothing is sent.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only == read_only {
            return;
        }
        self.read_only = read_only;
        if read_only {
            self.cancel_drag();
        }
        cx.notify();
    }

    /// Drops an open drag and shows the scene as the screen last gave it.
    fn cancel_drag(&mut self) {
        if let Some(d) = self.drag.take() {
            if d.moved {
                self.scene = d.base;
            }
        }
    }

    /// Selects an item from the screen (a list row, the inspector); reports nothing back.
    pub fn select(&mut self, item: Option<MapItem>, cx: &mut Context<Self>) {
        self.selected = item.filter(|i| self.scene.contains(i));
        cx.notify();
    }

    fn set_selected(&mut self, item: Option<MapItem>, cx: &mut Context<Self>) {
        if self.selected != item {
            self.selected = item.clone();
            cx.emit(MapEvent::Selected(item));
        }
    }

    fn view(&self) -> Option<View> {
        if let Some(d) = &self.drag {
            return Some(d.view);
        }
        let b = self.area.get();
        (f32::from(b.size.width) > 1. && f32::from(b.size.height) > 1.).then(|| fitted(&self.scene, b))
    }

    fn pick(&self, view: &View, s: (f32, f32)) -> Option<Pick> {
        pick_at(&self.scene, self.selected.as_ref(), self.read_only, view, s)
    }

    fn down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        // A drag whose release never reached the map (let go outside the window) ends here, at
        // the spot it was last shown, so what is on screen is what gets saved.
        self.up(cx);
        let Some(view) = self.view() else { return };
        let s = (f32::from(ev.position.x), f32::from(ev.position.y));
        let base = self.scene.clone();
        match self.pick(&view, s) {
            Some(Pick::Handle(item)) => {
                if !self.read_only {
                    self.drag = Some(Drag { pick: Pick::Handle(item), view, press: s, grab: [0., 0.], moved: false, base });
                }
            }
            Some(Pick::Item(MapItem::Station(id))) if self.scene.kind == MapKind::CameraMap => {
                if !self.read_only {
                    cx.emit(MapEvent::StationPicked(id));
                }
            }
            Some(Pick::Item(item)) => {
                self.set_selected(Some(item.clone()), cx);
                if !self.read_only && self.scene.kind.movable(&item) {
                    if let Some(p) = self.scene.pos_of(&item) {
                        let grab = geom::sub(p, view.to_world(s));
                        self.drag = Some(Drag { pick: Pick::Item(item), view, press: s, grab, moved: false, base });
                    }
                }
            }
            None => self.set_selected(None, cx),
        }
        cx.notify();
    }

    fn drag_to(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let s = (f32::from(ev.position.x), f32::from(ev.position.y));
        let Some(d) = self.drag.as_mut() else { return };
        if !d.moved && (s.0 - d.press.0).hypot(s.1 - d.press.1) < DRAG_START_PX {
            return;
        }
        d.moved = true;
        let (pick, pointer, grab) = (d.pick.clone(), d.view.to_world(s), d.grab);
        let (step_m, step_deg) = steps(ev.modifiers.shift, ev.modifiers.alt);
        match pick {
            Pick::Item(item) => {
                let p = moved_to(self.scene.limits(&item), pointer, grab, step_m);
                if self.scene.pos_of(&item) != Some(p) && Rc::make_mut(&mut self.scene).set_pos(&item, p) {
                    cx.emit(MapEvent::Moved { item, pos: p, done: false });
                }
            }
            Pick::Handle(item) => {
                let Some(c) = self.scene.pos_of(&item) else { return };
                let Some(y) = turned_to(c, pointer, step_deg) else { return };
                if self.scene.yaw_of(&item) != Some(y) && Rc::make_mut(&mut self.scene).set_yaw(&item, y) {
                    cx.emit(MapEvent::Turned { item, yaw_deg: y, done: false });
                }
            }
        }
        cx.notify();
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if d.moved {
            match d.pick {
                Pick::Item(item) => {
                    if let Some(pos) = self.scene.pos_of(&item) {
                        cx.emit(MapEvent::Moved { item, pos, done: true });
                    }
                }
                Pick::Handle(item) => {
                    if let Some(yaw_deg) = self.scene.yaw_of(&item) {
                        cx.emit(MapEvent::Turned { item, yaw_deg, done: true });
                    }
                }
            }
        }
        cx.notify();
    }

    fn hover_at(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let pick = self.view().and_then(|v| self.pick(&v, (f32::from(pos.x), f32::from(pos.y))));
        if pick != self.hover {
            self.hover = pick;
            cx.notify();
        }
    }

    /// Ctrl + wheel over the selected item turns it (design 9.7: "scroll-wheel on the camera
    /// rotates yaw"). A plain wheel always scrolls the page, so scrolling past the map never
    /// changes anything; during a drag the page holds still (the drag's view is fixed).
    fn wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.drag.is_some() {
            cx.stop_propagation();
            return;
        }
        if self.read_only || !ev.modifiers.control {
            return;
        }
        let Some(view) = self.view() else { return };
        let item = match self.pick(&view, (f32::from(ev.position.x), f32::from(ev.position.y))) {
            Some(Pick::Item(i)) if self.selected.as_ref() == Some(&i) && self.scene.kind.turnable(&i) => i,
            _ => return,
        };
        cx.stop_propagation();
        let d = ev.delta.pixel_delta(px(WHEEL_NOTCH_PX));
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        self.wheel += if dy != 0. { dy } else { dx };
        let notches = (self.wheel / WHEEL_NOTCH_PX).trunc();
        if notches == 0. {
            return;
        }
        self.wheel -= notches * WHEEL_NOTCH_PX;
        let step = if ev.modifiers.shift { COARSE_TURN_DEG } else { TURN_STEP_DEG };
        let Some(yaw) = self.scene.yaw_of(&item) else { return };
        let y = geom::snap_deg(yaw - notches * step, step);
        if Rc::make_mut(&mut self.scene).set_yaw(&item, y) {
            cx.emit(MapEvent::Turned { item, yaw_deg: y, done: true });
        }
        cx.notify();
    }

    /// Arrow keys nudge the selected item (Shift: 0.5 m, Alt: 1 cm), [ and ] turn it, Escape lets
    /// go of it, Delete asks the screen to remove it.
    fn key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.platform {
            return;
        }
        let Some(item) = self.selected.clone() else { return };
        let editable = !self.read_only && self.drag.is_none();
        let nudge = if k.modifiers.alt { 0.01 } else if k.modifiers.shift { COARSE_STEP_M } else { MOVE_STEP_M };
        let turn = if k.modifiers.shift { COARSE_TURN_DEG } else { TURN_STEP_DEG };
        let delta = match k.key.as_str() {
            "left" => Some([-nudge, 0.]),
            "right" => Some([nudge, 0.]),
            "up" => Some([0., -nudge]),
            "down" => Some([0., nudge]),
            _ => None,
        };
        match k.key.as_str() {
            "escape" => self.set_selected(None, cx),
            _ if delta.is_some() && editable && self.scene.kind.movable(&item) => {
                let (Some(p), Some(d)) = (self.scene.pos_of(&item), delta) else { return };
                let p = geom::clamp_in(geom::snap2(geom::add(p, d), 0.001), self.scene.limits(&item));
                if Rc::make_mut(&mut self.scene).set_pos(&item, p) {
                    cx.emit(MapEvent::Moved { item, pos: p, done: true });
                }
            }
            "[" | "]" if editable && self.scene.kind.turnable(&item) => {
                let Some(yaw) = self.scene.yaw_of(&item) else { return };
                let y = geom::snap_deg(if k.key == "[" { yaw - turn } else { yaw + turn }, turn);
                if Rc::make_mut(&mut self.scene).set_yaw(&item, y) {
                    cx.emit(MapEvent::Turned { item, yaw_deg: y, done: true });
                }
            }
            "delete" | "backspace" if editable && item != MapItem::Camera => cx.emit(MapEvent::DeleteRequested(item)),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn cursor(&self) -> CursorStyle {
        if self.drag.as_ref().is_some_and(|d| d.moved) {
            return CursorStyle::ClosedHand;
        }
        match &self.hover {
            Some(Pick::Handle(_)) => CursorStyle::PointingHand,
            Some(Pick::Item(i)) if !self.read_only && self.scene.kind.movable(i) => CursorStyle::OpenHand,
            Some(Pick::Item(_)) => CursorStyle::PointingHand,
            None => CursorStyle::Arrow,
        }
    }
}

impl Render for MapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A narrow window puts the station legend under the map, so the map keeps its width.
        let narrow = crate::widgets::is_narrow(window);
        let frame = paint::Frame {
            scene: self.scene.clone(),
            selected: self.selected.clone(),
            hover: self.hover.as_ref().map(|h| (h.item().clone(), matches!(h, Pick::Handle(_)))),
            view: self.drag.as_ref().map(|d| d.view),
            read_only: self.read_only,
        };
        let area = self.area.clone();
        let legend = self.legend(narrow, cx);
        let map = div()
            .id("anime-map")
            .relative()
            .flex_1()
            .min_w_0()
            .when(!narrow, |m| m.h_full().min_h(px(200.)))
            .when(narrow, |m| m.w_full().min_h(px(NARROW_MAP_MIN_H)))
            .overflow_hidden()
            .rounded(px(radius::LG))
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .cursor(self.cursor())
            .child(canvas(move |b, _, _| area.set(b), move |b, (), window, cx| paint::paint(&frame, b, window, cx)).absolute().inset_0())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.down(ev, window, cx);
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) && this.drag.is_some() {
                    this.drag_to(ev, cx);
                } else {
                    // The button came up where the map could not see it (outside the window):
                    // the drag ends at the spot it was last shown.
                    this.up(cx);
                    this.hover_at(ev.position, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.up(cx)))
            .on_mouse_exit(cx.listener(|this, _: &MouseExitEvent, _, cx| {
                if this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| this.wheel(ev, cx)));
        div()
            .id("anime-map-frame")
            .track_focus(&self.focus)
            .flex()
            .when(narrow, |f| f.flex_col())
            .gap(px(space::SM))
            .size_full()
            .min_h(px(200.))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key(ev, cx)))
            .child(map)
            .children(legend)
    }
}

impl MapView {
    /// The stations by number beside the map (the pins carry only the number): pointing at a row
    /// names its pin on the map; a click does what a click on the pin does.
    fn legend(&self, narrow: bool, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.scene.stations.is_empty() {
            return None;
        }
        let list = div().id("anime-map-legend").flex().flex_col().gap(px(2.)).flex_none().p(px(space::XS)).overflow_y_scroll().rounded(px(radius::LG)).bg(Tokens::surface_2());
        // Beside the map, or under it at full width in a narrow window.
        let mut list = if narrow { list.w_full().h(px(NARROW_LEGEND_H)) } else { list.w(px(LEGEND_W)).h_full() };
        list = list.child(div().px(px(space::XS)).pb(px(space::XXS)).text_size(px(size::XS)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_3()).child("Camera stations"));
        for (i, s) in self.scene.stations.iter().enumerate() {
            let item = MapItem::Station(s.id.clone());
            let (sel, hov) = (self.selected.as_ref() == Some(&item), self.hover.as_ref().is_some_and(|h| *h.item() == item));
            let in_use = self.scene.camera.as_ref().is_some_and(|c| geom::dist([c.pos[0], c.pos[2]], [s.pos[0], s.pos[2]]) < 0.05);
            let name = if s.name.trim().is_empty() { s.id.trim().to_string() } else { s.name.trim().to_string() };
            let pin = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(LEGEND_PIN))
                .rounded_full()
                .border_1()
                .border_color(if sel || in_use { Tokens::accent() } else { Tokens::border_strong() })
                .bg(Tokens::surface())
                .text_size(px(size::XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if sel || in_use { Tokens::accent() } else { Tokens::text_1() })
                .child((i + 1).to_string());
            let (enter, leave, click) = (item.clone(), item.clone(), item.clone());
            list = list.child(
                div()
                    .id(("anime-map-station", i))
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .h(px(LEGEND_ROW_H))
                    .px(px(space::XS))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .when(sel || hov, |d| d.bg(Tokens::hover()))
                    .tooltip(mv_ui::tip(paint::station_text(i + 1, s)))
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                        let want = if *on { Some(Pick::Item(enter.clone())) } else { None };
                        if *on || this.hover.as_ref().is_some_and(|h| *h.item() == leave) {
                            this.hover = want;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.legend_pick(click.clone(), cx)))
                    .child(pin)
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(name))
                    .when(s.lens_mm > 0, |d| d.child(div().flex_none().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("{} mm", s.lens_mm)))),
            );
        }
        Some(list)
    }

    /// A legend row clicked: the same as a click on its pin.
    fn legend_pick(&mut self, item: MapItem, cx: &mut Context<Self>) {
        match (&item, self.scene.kind) {
            (MapItem::Station(id), MapKind::CameraMap) => {
                if !self.read_only {
                    cx.emit(MapEvent::StationPicked(id.clone()));
                }
            }
            _ => self.set_selected(Some(item), cx),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::geom::{Rect, View};
    use super::paint::footprint_corners;
    use super::{handle_at, mark_r, moved_to, pick_at, steps, turned_to, MapCamera, MapItem, MapKind, MapScene, Pick, COARSE_STEP_M, MOVE_STEP_M, OUTSIDE_M, TURN_STEP_DEG};
    use mv_anime::{Character, Look, Mark, Place, Shot, Visibility};

    fn view() -> View {
        View { scale: 50., centre: [0., 0.], screen_centre: (400., 300.) }
    }

    #[test]
    fn scene_items_move_and_turn_by_id() {
        let mut s = MapScene::sample(MapKind::CameraMap);
        let song = MapItem::Mark("song".into());
        assert_eq!(s.pos_of(&song), Some([1., -0.4]));
        assert!(s.set_pos(&song, [2., 1.]));
        assert_eq!(s.mark("song").unwrap().pos, [2., 1.]);
        assert!(s.set_yaw(&MapItem::Camera, -10.));
        assert_eq!(s.yaw_of(&MapItem::Camera), Some(350.));
        // The camera keeps its height when moved.
        assert!(s.set_pos(&MapItem::Camera, [0., 3.]));
        assert_eq!(s.camera.as_ref().unwrap().pos, [0., 1.4, 3.]);
        assert!(s.set_yaw(&MapItem::Landmark("altar".into()), 90.));
        assert_eq!(s.landmark("altar").unwrap().rot_deg, 90.);
        assert!(!s.set_pos(&MapItem::Mark("nobody".into()), [0., 0.]));
        assert!(!s.contains(&MapItem::Station("nowhere".into())));
        // Marks stay in the room; the camera may stand outside the walls.
        assert_eq!(s.limits(&song), Rect::new(-5., -3.5, 5., 3.5));
        assert_eq!(s.limits(&MapItem::Camera), Rect::new(-5. - OUTSIDE_M, -3.5 - OUTSIDE_M, 5. + OUTSIDE_M, 3.5 + OUTSIDE_M));
        assert_eq!(MapScene::default().limits(&song), Rect::default());
    }

    #[test]
    fn what_moves_depends_on_the_screen() {
        let (plan, cam) = (MapKind::FloorPlan, MapKind::CameraMap);
        let (lm, st, mk) = (MapItem::Landmark("a".into()), MapItem::Station("b".into()), MapItem::Mark("c".into()));
        assert!(plan.movable(&lm) && plan.movable(&st) && !plan.movable(&mk) && !plan.movable(&MapItem::Camera));
        assert!(cam.movable(&mk) && cam.movable(&MapItem::Camera) && !cam.movable(&lm) && !cam.movable(&st));
    }

    #[test]
    fn drags_snap_and_stay_in_bounds() {
        let room = Rect::new(-5., -3.5, 5., 3.5);
        assert_eq!(steps(false, false), (MOVE_STEP_M, TURN_STEP_DEG));
        assert_eq!(steps(true, false).0, COARSE_STEP_M);
        assert_eq!(steps(true, true), (0., 0.));
        // Taken 0.2 m left of its centre: the item keeps that offset, then snaps to 10 cm.
        assert_eq!(moved_to(room, [1.03, 0.51], [0.2, 0.], MOVE_STEP_M), [1.2, 0.5]);
        assert_eq!(moved_to(room, [9., -9.], [0., 0.], MOVE_STEP_M), [5., -3.5]);
        assert_eq!(moved_to(Rect::default(), [9.04, -9.], [0., 0.], 0.), [9.04, -9.]);
        // The handle pulled straight right of the item: yaw 90; slightly off snaps back.
        assert_eq!(turned_to([1., 1.], [3., 1.], TURN_STEP_DEG), Some(90.));
        assert_eq!(turned_to([0., 0.], [1., -0.98], 15.), Some(45.));
        assert_eq!(turned_to([0., 0.], [0., 0.], 5.), None);
    }

    #[test]
    fn presses_take_markers_and_only_the_selected_items_handle() {
        let s = MapScene::sample(MapKind::CameraMap);
        let v = view();
        let at = |p: [f32; 2]| v.to_screen(p);
        let jinwoo = MapItem::Mark("jinwoo".into());
        assert_eq!(pick_at(&s, None, false, &v, at([-0.8, 0.4])), Some(Pick::Item(jinwoo.clone())));
        assert_eq!(pick_at(&s, None, false, &v, at([0.6, 2.6])), Some(Pick::Item(MapItem::Camera)));
        assert_eq!(pick_at(&s, None, false, &v, at([4.2, -0.4])), Some(Pick::Item(MapItem::Station("st_side".into()))));
        assert_eq!(pick_at(&s, None, false, &v, at([0., -2.6])), Some(Pick::Item(MapItem::Landmark("altar".into()))));
        assert_eq!(pick_at(&s, None, false, &v, at([-1., 2.5])), None);
        // Jinwoo selected: his handle sits in his facing direction (75 degrees) and wins there.
        let (item, h) = handle_at(&s, Some(&jinwoo), false, &v).unwrap();
        assert_eq!(item, jinwoo);
        let off = mark_r(v.scale) + 18.;
        let (hx, hy) = v.to_screen(h);
        let (cx, cy) = v.to_screen([-0.8, 0.4]);
        assert!(((hx - cx).hypot(hy - cy) - off).abs() < 0.01);
        assert!(hx > cx && hy < cy, "75 degrees: right and a little up");
        assert_eq!(pick_at(&s, Some(&jinwoo), false, &v, (hx, hy)), Some(Pick::Handle(jinwoo.clone())));
        // No handle when read-only, or for an item this screen does not turn.
        assert!(handle_at(&s, Some(&jinwoo), true, &v).is_none());
        assert!(handle_at(&s, Some(&MapItem::Landmark("altar".into())), false, &v).is_none());
        assert!(handle_at(&MapScene::sample(MapKind::FloorPlan), Some(&MapItem::Landmark("altar".into())), false, &v).is_some());
    }

    #[test]
    fn a_character_beside_the_camera_can_still_be_picked() {
        let mut s = MapScene::sample(MapKind::CameraMap);
        let v = view();
        // Jinwoo a hand's width (15 px) from the camera: a press on him takes him, not the camera.
        assert!(s.set_pos(&MapItem::Mark("jinwoo".into()), [0.9, 2.6]));
        assert_eq!(pick_at(&s, None, false, &v, v.to_screen([0.9, 2.6])), Some(Pick::Item(MapItem::Mark("jinwoo".into()))));
        assert_eq!(pick_at(&s, None, false, &v, v.to_screen([0.6, 2.6])), Some(Pick::Item(MapItem::Camera)));
    }

    #[test]
    fn a_camera_looking_down_shows_its_patch_of_floor() {
        let cam = |pitch_deg: f32| MapCamera { pos: [1., 8., 2.], yaw_deg: 0., pitch_deg, lens_mm: 24, label: String::new() };
        assert!(footprint_corners(&cam(0.)).is_none(), "a level camera draws a cone");
        assert!(footprint_corners(&cam(-45.)).is_none());
        assert!(footprint_corners(&cam(80.)).is_none(), "looking up sees no floor");
        let c = footprint_corners(&cam(-90.)).unwrap_or_default();
        let mid = [c.iter().map(|p| p[0]).sum::<f32>() / 4., c.iter().map(|p| p[1]).sum::<f32>() / 4.];
        assert!((mid[0] - 1.).abs() < 1e-3 && (mid[1] - 2.).abs() < 1e-3, "straight down: under the camera");
        let across = (c[1][0] - c[0][0]).abs();
        assert!(across > 8. && across < 20., "24 mm from 8 m sees a wide patch: {across}");
    }

    #[test]
    fn a_shot_becomes_a_camera_map() {
        let ch = |id: &str, name: &str, colour: &str| Character { id: id.into(), name: name.into(), look: Look { signature_colour: colour.into(), ..Default::default() }, ..Default::default() };
        let cast = [ch("hae", "Cha Hae-In", "#E0A030"), ch("jin", "Sung Jinwoo", "not a colour")];
        let mk = |id: &str, pos: [f32; 2], visible: Visibility| Mark { character: id.into(), pos, facing_deg: -90., visible, ..Default::default() };
        let shot = Shot { marks: vec![mk("jin", [1., 0.], Visibility::Full), mk("ghost", [0., 0.], Visibility::Offscreen), mk("hae", [-1., f32::NAN], Visibility::Full)], ..Default::default() };
        let place = Place { plan: MapScene::sample(MapKind::FloorPlan).plan, ..Default::default() };
        let s = MapScene::camera_map(Some(&place), &shot, &cast, "3.4");
        assert_eq!(s.kind, MapKind::CameraMap);
        assert_eq!(s.plan.w_m, 10.);
        let jin = s.mark("jin").unwrap();
        assert_eq!((jin.name.as_str(), jin.initials.as_str(), jin.colour, jin.hue, jin.facing_deg), ("Sung Jinwoo", "SJ", None, 1, 270.));
        let hae = s.mark("hae").unwrap();
        assert_eq!((hae.colour, hae.hue, hae.pos), (Some(0xe0a030), 0, [-1., 0.]));
        let ghost = s.mark("ghost").unwrap();
        assert!(ghost.dimmed && ghost.name == "ghost");
        // The line runs through the first two characters on screen, skipping the off-screen one.
        assert_eq!(s.axis, Some(["jin".to_string(), "hae".to_string()]));
        assert_eq!(s.camera.as_ref().unwrap().label, "3.4");
        // Without a place: an empty plan, the marks still show.
        let bare = MapScene::camera_map(None, &shot, &cast, "1.1");
        assert_eq!((bare.plan.w_m, bare.marks.len()), (0., 3));
        assert!(!bare.is_empty() && MapScene::default().is_empty());
    }
}
