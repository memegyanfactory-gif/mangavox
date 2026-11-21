//! Draws a [`MapScene`] on the map's canvas: floor and grid, the camera's side of the
//! 180-degree line, walls and openings, landmarks, stations, view cones, character marks, the
//! camera, selection and hover, the turn handle, labels (placed so none overlap) and the scale
//! bar. Every colour is a theme token; markers keep a fixed pixel size so the map reads at any
//! zoom.
use std::rc::Rc;

use gpui_kit::*;
use mv_anime::{geometry, Landmark, LandmarkKind, Station};
use mv_ui::theme::{contrast, radius};
use mv_ui::{Hue, Tokens};

use super::geom::{self, LabelReq, Rect, View, P2};
use super::{fitted, handle_at, mark_r, rect_of, MapCamera, MapItem, MapKind, MapMark, MapScene, CAM_R, HANDLE_R, STATION_R};

/// What one frame draws (a snapshot of the view's state).
pub(super) struct Frame {
    pub scene: Rc<MapScene>,
    pub selected: Option<MapItem>,
    /// The hovered item, and whether the pointer is on its turn handle.
    pub hover: Option<(MapItem, bool)>,
    /// The view frozen by a drag; `None` = fit the scene to the canvas.
    pub view: Option<View>,
    pub read_only: bool,
}

const LABEL_PX: f32 = 11.;
const LABEL_LINE: f32 = 14.;
const LABEL_PAD_X: f32 = 5.;
const LABEL_H: f32 = 16.;
const SCALE_BAR_MAX_PX: f32 = 110.;
/// Margin of the corner furniture (scale bar) and labels from the map's edge.
const EDGE: f32 = 10.;

/// The number in a station pin.
const PIN_PX: f32 = 10.;
const PIN_LINE: f32 = 12.;
/// A landmark's label is cut to this many characters (with "…") unless it is selected or hovered.
const LANDMARK_CHARS: usize = 16;
/// Below this zoom (px per metre) only the selected or hovered landmark is named.
const LANDMARK_LABEL_MIN_SCALE: f32 = 14.;

/// Label priorities: lower is placed first and wins contested room.
const P_FOCUS: u8 = 0;
const P_CAMERA: u8 = 1;
const P_MARK: u8 = 2;
const P_LANDMARK: u8 = 5;
const P_AXIS: u8 = 7;

/// A station as the legend and its hover label name it: "3 · by the bed, looking east · 35 mm".
pub(super) fn station_text(n: usize, s: &Station) -> String {
    let name = if s.name.trim().is_empty() { s.id.trim() } else { s.name.trim() };
    if s.lens_mm > 0 {
        format!("{n} · {name} · {} mm", s.lens_mm)
    } else {
        format!("{n} · {name}")
    }
}

/// A landmark's label: whole when it has the focus, else cut to [`LANDMARK_CHARS`].
fn landmark_text(name: &str, focus: bool) -> String {
    let name = name.trim();
    if focus || name.chars().count() <= LANDMARK_CHARS {
        return name.to_string();
    }
    let cut: String = name.chars().take(LANDMARK_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A camera pitched further than this (degrees) from level shows its patch of floor (looking
/// down) or nothing (looking up) instead of a view cone.
pub(super) const STEEP_PITCH_DEG: f32 = 60.;

/// The four corners of the floor a camera looking down sees, its top edge toward the camera's
/// facing; `None` when the camera does not look down steeply or has no lens.
pub(super) fn footprint_corners(cam: &MapCamera) -> Option<[P2; 4]> {
    let pitch = geom::fin(cam.pitch_deg).clamp(-90., 90.);
    if pitch >= -STEEP_PITCH_DEG || cam.lens_mm == 0 {
        return None;
    }
    let height = geom::fin(cam.pos[1]).max(0.05);
    let down = pitch.abs().to_radians();
    // Along the view to the floor, and how far ahead of the camera the frame's centre lands.
    let reach = height / down.sin();
    let ahead = height / down.tan();
    let (h_fov, v_fov) = geometry::fov_deg(cam.lens_mm as f32);
    let across = 2. * reach * (h_fov / 2.).to_radians().tan();
    let along = 2. * reach * (v_fov / 2.).to_radians().tan();
    if !(across.is_finite() && along.is_finite() && across > 0. && along > 0.) {
        return None;
    }
    let centre = geom::add(geom::fin2([cam.pos[0], cam.pos[2]]), geom::mul(geom::dir(cam.yaw_deg), ahead));
    Some(geom::box_corners(centre, [across, along], geom::fin(cam.yaw_deg)))
}

pub(super) fn paint(f: &Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let screen = rect_of(bounds);
    if screen.w() < 2. || screen.h() < 2. {
        return;
    }
    let view = f.view.unwrap_or_else(|| fitted(&f.scene, bounds));
    window.with_content_mask(Some(ContentMask { bounds }), |window| Painter { f, view, screen, labels: Vec::new(), obstacles: Vec::new() }.run(window, cx));
}

/// One label waiting for placement.
struct Lab {
    line: ShapedLine,
    prio: u8,
    w: f32,
    spots: Vec<(f32, f32)>,
}

struct Painter<'a> {
    f: &'a Frame,
    view: View,
    screen: Rect,
    labels: Vec<Lab>,
    /// Markers on screen, which labels must not cover.
    obstacles: Vec<Rect>,
}

impl Painter<'_> {
    fn run(mut self, window: &mut Window, cx: &mut App) {
        let scene = self.f.scene.clone();
        let visible = self.view.rect_to_world(self.screen);
        let floor = geom::floor(scene.plan.w_m, scene.plan.d_m);
        let room = floor.unwrap_or(visible);

        // Floor and grid.
        if let Some(fl) = floor {
            window.paint_quad(fill(bounds_of(self.view.rect_to_screen(fl)), Hsla::from(Tokens::surface())));
        }
        self.grid(window, room);

        // The camera's half of the room, under everything else.
        let axis = scene.axis_points();
        if let (Some((a, b)), Some(cam)) = (axis, scene.camera.as_ref()) {
            let half = geom::clip_half(&room.corners(), a, b, [cam.pos[0], cam.pos[2]]);
            self.poly_fill(window, &half, Tokens::accent().opacity(0.07));
        }
        if let Some(fl) = floor {
            window.paint_quad(quad(bounds_of(self.view.rect_to_screen(fl)), px(0.), Hsla::from(Tokens::base().alpha(0.)), px(1.), Hsla::from(Tokens::border_strong()), BorderStyle::Solid));
        }

        // Landmarks under the walls, then the walls, then the openings cut into them.
        for l in scene.plan.landmarks.iter().filter(|l| !is_opening(l)) {
            self.landmark(window, l);
        }
        self.walls(window);
        for l in scene.plan.landmarks.iter().filter(|l| is_opening(l)) {
            self.opening(window, l);
        }

        // View cones: the selected station's on the floor plan, the shot camera's.
        if let (MapKind::FloorPlan, Some(MapItem::Station(id))) = (scene.kind, &self.f.selected) {
            if let Some(s) = scene.station(id) {
                self.cone(window, s.pos, s.yaw_deg, s.lens_mm, visible, (0.07, 0.45));
            }
        }
        if let Some(c) = &scene.camera {
            // A camera looking steeply down sees a patch of floor under it, not a wedge across
            // the room; one looking steeply up sees no floor at all.
            let pitch = geom::fin(c.pitch_deg);
            if pitch < -STEEP_PITCH_DEG {
                self.footprint(window, c, visible, (0.10, 0.6));
            } else if pitch <= STEEP_PITCH_DEG {
                self.cone(window, c.pos, c.yaw_deg, c.lens_mm, visible, (0.10, 0.6));
            }
        }

        // The 180-degree line across the whole room.
        if let Some((a, b)) = axis {
            if let Some((p, q)) = geom::line_in_rect(a, b, room) {
                let mut pb = PathBuilder::stroke(px(1.5)).dash_array(&[px(6.), px(4.)]);
                pb.move_to(self.pt(p));
                pb.line_to(self.pt(q));
                if let Ok(path) = pb.build() {
                    window.paint_path(path, Hsla::from(Tokens::warning()));
                }
                let end = self.view.to_screen(geom::add(q, geom::mul(geom::sub(p, q), 0.06)));
                self.label(window, "180° line", false, end, 4., false, P_AXIS);
            }
        }

        for (i, s) in scene.stations.iter().enumerate() {
            self.station(window, cx, i + 1, s);
        }
        for m in &scene.marks {
            self.mark(window, cx, m);
        }
        if let Some(c) = &scene.camera {
            self.camera(window, c);
        }
        self.handle(window);
        self.scale_bar(window, cx);
        self.draw_labels(window, cx);

        if scene.is_empty() {
            let line = shape(window, "No floor plan yet", mv_ui::size::SM, FontWeight::NORMAL, Tokens::text_3());
            let (x, y) = self.screen.centre();
            let _ = line.paint(point(px(x - f32::from(line.width) / 2.), px(y - 9.)), px(18.), TextAlign::Left, None, window, cx);
        }
    }

    fn pt(&self, p: P2) -> Point<Pixels> {
        let (x, y) = self.view.to_screen(p);
        point(px(x), px(y))
    }

    fn is_selected(&self, item: &MapItem) -> bool {
        self.f.selected.as_ref() == Some(item)
    }

    fn is_hovered(&self, item: &MapItem) -> bool {
        self.f.hover.as_ref().is_some_and(|(h, _)| h == item)
    }

    fn poly_fill(&self, window: &mut Window, pts: &[P2], c: Rgba) {
        let pts: Vec<Point<Pixels>> = pts.iter().map(|p| self.pt(*p)).collect();
        poly_fill(window, &pts, c);
    }

    fn poly_line(&self, window: &mut Window, pts: &[P2], closed: bool, w: f32, c: Rgba) {
        let pts: Vec<Point<Pixels>> = pts.iter().map(|p| self.pt(*p)).collect();
        poly_line(window, &pts, closed, w, c);
    }

    /// Hairlines every grid step inside `area` (plan metres), stronger on the major steps.
    fn grid(&self, window: &mut Window, area: Rect) {
        let step = geom::grid_step(self.view.scale);
        let major = geom::major_step(step);
        let r = self.view.rect_to_screen(area);
        let (minor_c, major_c) = (Hsla::from(Tokens::border().opacity(0.55)), Hsla::from(Tokens::border()));
        for x in geom::grid_lines(area.x0, area.x1, step, 600) {
            let sx = self.view.to_screen([x, 0.]).0.round();
            let c = if geom::on_step(x, major) { major_c } else { minor_c };
            window.paint_quad(fill(Bounds::new(point(px(sx), px(r.y0)), gpui_kit::size(px(1.), px(r.h()))), c));
        }
        for z in geom::grid_lines(area.y0, area.y1, step, 600) {
            let sy = self.view.to_screen([0., z]).1.round();
            let c = if geom::on_step(z, major) { major_c } else { minor_c };
            window.paint_quad(fill(Bounds::new(point(px(r.x0), px(sy)), gpui_kit::size(px(r.w()), px(1.))), c));
        }
    }

    fn wall_px(&self) -> f32 {
        (0.15 * self.view.scale).clamp(2.5, 7.)
    }

    fn walls(&self, window: &mut Window) {
        let w = self.wall_px();
        let c = Tokens::text_2();
        for wall in &self.f.scene.plan.walls {
            let (a, b) = (geom::fin2(wall.a), geom::fin2(wall.b));
            self.poly_line(window, &[a, b], false, w, c);
            // Square ends so corners meet without notches.
            for p in [a, b] {
                let (x, y) = self.view.to_screen(p);
                window.paint_quad(fill(Bounds::new(point(px(x - w / 2.), px(y - w / 2.)), gpui_kit::size(px(w), px(w))), Hsla::from(c)));
            }
        }
    }

    /// A door or window: a gap in the wall, then a door's leaf and swing into the room, or a
    /// window's double line.
    fn opening(&mut self, window: &mut Window, l: &Landmark) {
        let item = MapItem::Landmark(l.id.clone());
        let (pos, rot) = (geom::fin2(l.pos), geom::fin(l.rot_deg));
        let width = geom::fin(l.size[0]).abs().max(0.3);
        let thick = (self.wall_px() + 2.) / self.view.scale;
        let gap = geom::box_corners(pos, [width, thick], rot);
        self.poly_fill(window, &gap, Tokens::surface());
        let along = geom::rot([1., 0.], rot);
        // Across the wall, pointing into the room (toward the plan centre).
        let mut into = geom::rot([0., 1.], rot);
        if into[0] * -pos[0] + into[1] * -pos[1] < 0. {
            into = geom::mul(into, -1.);
        }
        let sel = self.is_selected(&item);
        let line_c = if sel { Tokens::accent() } else { Tokens::text_2() };
        let (left, right) = (geom::sub(pos, geom::mul(along, width / 2.)), geom::add(pos, geom::mul(along, width / 2.)));
        if l.kind == LandmarkKind::Door {
            let leaf = geom::add(left, geom::mul(into, width));
            self.poly_line(window, &[left, leaf], false, 1.5, line_c);
            if let (Some(a0), Some(a1)) = (geom::yaw_to(left, leaf), geom::yaw_to(left, right)) {
                self.poly_line(window, &geom::arc(left, width, a0, a1, 16), false, 1., Tokens::text_3());
            }
            let half = geom::mul(into, thick / 2.);
            for p in [left, right] {
                self.poly_line(window, &[geom::sub(p, half), geom::add(p, half)], false, 1.5, Tokens::text_2());
            }
        } else {
            for s in [-0.22, 0.22] {
                let o = geom::mul(into, thick * s);
                self.poly_line(window, &[geom::add(left, o), geom::add(right, o)], false, 1., line_c);
            }
            self.poly_line(window, &gap, true, 1., Tokens::text_3());
        }
        self.outline_state(window, &item, &gap);
        // Right beside the gap in the wall (the label reaches past the wall's thickness, not
        // past the opening's width, so it stays next to its window or door).
        let reach = thick * self.view.scale / 2. + 3.;
        self.landmark_label(window, &item, &l.name, sel, self.view.to_screen(pos), reach, false);
    }

    /// A landmark's name: cut short unless it has the focus, left out when the plan is drawn
    /// too small, placed after the markers so it never covers one.
    #[allow(clippy::too_many_arguments)]
    fn landmark_label(&mut self, window: &mut Window, item: &MapItem, name: &str, sel: bool, at: (f32, f32), reach: f32, inside: bool) {
        let focus = sel || self.is_hovered(item);
        if !focus && self.view.scale < LANDMARK_LABEL_MIN_SCALE {
            return;
        }
        self.label(window, &landmark_text(name, focus), sel, at, reach, inside, if focus { P_FOCUS } else { P_LANDMARK });
    }

    fn landmark(&mut self, window: &mut Window, l: &Landmark) {
        let item = MapItem::Landmark(l.id.clone());
        let (pos, rot) = (geom::fin2(l.pos), geom::fin(l.rot_deg));
        let size = [geom::fin(l.size[0]).abs().max(0.1), geom::fin(l.size[1]).abs().max(0.1)];
        let corners = geom::box_corners(pos, size, rot);
        let c = self.view.to_screen(pos);
        let (wpx, hpx) = (size[0] * self.view.scale, size[1] * self.view.scale);
        match l.kind {
            LandmarkKind::Light => {
                let r = (wpx.max(hpx) / 2.).clamp(5., 18.);
                disc(window, c, r, Tokens::warning_soft(), Some((1.5, Tokens::warning())), BorderStyle::Solid);
                disc(window, c, 2.5, Tokens::warning(), None, BorderStyle::Solid);
            }
            LandmarkKind::Wall => self.poly_fill(window, &corners, Tokens::text_2()),
            _ => {
                self.poly_fill(window, &corners, Tokens::surface_3());
                self.poly_line(window, &corners, true, 1., Tokens::border_strong());
                // A short tick in from the front edge (its own north) shows which way it faces.
                let front = geom::add(pos, geom::rot([0., -size[1] / 2.], rot));
                let tick = (6. / self.view.scale).min(size[1] / 2.);
                let inner = geom::add(pos, geom::rot([0., -size[1] / 2. + tick], rot));
                self.poly_line(window, &[front, inner], false, 1., Tokens::text_3());
            }
        }
        let sel = self.is_selected(&item);
        if sel {
            self.poly_fill(window, &corners, Tokens::accent().opacity(0.12));
        }
        self.outline_state(window, &item, &corners);
        let inside = wpx.min(hpx) >= LABEL_H + 4. && l.kind != LandmarkKind::Light;
        let reach = if l.kind == LandmarkKind::Light { (wpx.max(hpx) / 2.).clamp(5., 18.) + 2. } else { (wpx.max(hpx) / 2.).min(40.) + 2. };
        self.landmark_label(window, &item, &l.name, sel, c, reach, inside);
    }

    /// The selection or hover outline of a landmark.
    fn outline_state(&self, window: &mut Window, item: &MapItem, corners: &[P2]) {
        if self.is_selected(item) {
            self.poly_line(window, corners, true, 2., Tokens::accent());
        } else if self.is_hovered(item) {
            self.poly_line(window, corners, true, 1.5, Tokens::text_3());
        }
    }

    /// A view cone from `geometry::view_cone`, reaching the edge of the map; `alpha` = (fill, edges).
    fn cone(&self, window: &mut Window, pos: [f32; 3], yaw: f32, lens_mm: u16, visible: Rect, alpha: (f32, f32)) {
        let pos = pos.map(geom::fin);
        let apex = [pos[0], pos[2]];
        let reach = visible.corners().iter().map(|c| geom::dist(*c, apex)).fold(0f32, f32::max).max(1.);
        let [a, l, r] = geometry::view_cone(pos, geom::fin(yaw), lens_mm as f32, reach).map(geom::fin2);
        if geom::dist(l, r) < 1e-4 || geom::dist(a, l) < 1e-4 {
            return; // no lens, or no cone to draw
        }
        // The cone stops at the walls (and the camera's own spot when it stands outside), so it
        // never spills over the rest of the map and its labels.
        let bounds = geom::floor(self.f.scene.plan.w_m, self.f.scene.plan.d_m).map(|room| room.include(apex[0], apex[1])).unwrap_or(visible);
        let tri = geom::clip_to_rect(&[a, l, r], bounds);
        if tri.len() < 3 {
            return;
        }
        self.poly_fill(window, &tri, Tokens::accent().opacity(alpha.0));
        // The two edges from the camera, as far as they stay in the room.
        for side in [l, r] {
            if let Some((p, q)) = geom::segment_in_rect(a, side, bounds) {
                self.poly_line(window, &[p, q], false, 1., Tokens::accent().opacity(alpha.1));
            }
        }
    }

    /// The patch of floor a camera looking steeply down sees: the frame's shape on the floor,
    /// its top edge toward where the camera faces.
    fn footprint(&self, window: &mut Window, cam: &MapCamera, visible: Rect, alpha: (f32, f32)) {
        let Some(corners) = footprint_corners(cam) else { return };
        let bounds = geom::floor(self.f.scene.plan.w_m, self.f.scene.plan.d_m).unwrap_or(visible);
        let patch = geom::clip_to_rect(&corners, bounds);
        if patch.len() < 3 {
            return;
        }
        self.poly_fill(window, &patch, Tokens::accent().opacity(alpha.0));
        self.poly_line(window, &patch, true, 1., Tokens::accent().opacity(alpha.1));
    }

    /// A camera station: a small numbered pin with a direction tick. Its full name shows only
    /// while it is selected or hovered (the legend beside the map lists them all), so station
    /// names never pile up on the plan.
    fn station(&mut self, window: &mut Window, cx: &mut App, n: usize, s: &Station) {
        let item = MapItem::Station(s.id.clone());
        let p = geom::fin2([s.pos[0], s.pos[2]]);
        let c = self.view.to_screen(p);
        let plan = self.f.scene.kind == MapKind::FloorPlan;
        let in_use = self.f.scene.camera.as_ref().is_some_and(|cam| geom::dist([cam.pos[0], cam.pos[2]], p) < 0.05);
        let (sel, hov) = (self.is_selected(&item), self.is_hovered(&item));
        let line_c = if sel || in_use {
            Tokens::accent()
        } else if plan || hov {
            Tokens::text_2()
        } else {
            Tokens::text_3()
        };
        // The direction tick first, so the pin sits over its root.
        let d = geom::dir(s.yaw_deg);
        let tick = [point(px(c.0 + d[0] * STATION_R), px(c.1 + d[1] * STATION_R)), point(px(c.0 + d[0] * (STATION_R + 7.)), px(c.1 + d[1] * (STATION_R + 7.)))];
        poly_line(window, &tick, false, 1.5, line_c);
        let bg = if sel { Tokens::surface().blend(Tokens::accent_soft()) } else { Tokens::surface() };
        disc(window, c, STATION_R, bg, Some((1.5, line_c)), BorderStyle::Solid);
        let num = shape(window, &n.to_string(), PIN_PX, FontWeight::SEMIBOLD, if sel || in_use { Tokens::accent() } else { Tokens::text_1() });
        let w = f32::from(num.width);
        let _ = num.paint(point(px(c.0 - w / 2.), px(c.1 - PIN_LINE / 2.)), px(PIN_LINE), TextAlign::Left, None, window, cx);
        if sel {
            ring(window, c, STATION_R + 4., 2., Tokens::accent());
        } else if hov {
            ring(window, c, STATION_R + 3., 1.5, Tokens::text_3());
        }
        self.obstacles.push(Rect::new(c.0 - STATION_R, c.1 - STATION_R, c.0 + STATION_R, c.1 + STATION_R));
        if sel || hov {
            self.label(window, &station_text(n, s), sel, c, STATION_R + 2., false, P_FOCUS);
        }
    }

    fn mark(&mut self, window: &mut Window, cx: &mut App, m: &MapMark) {
        let item = MapItem::Mark(m.id.clone());
        let c = self.view.to_screen(geom::fin2(m.pos));
        let r = mark_r(self.view.scale);
        let col = mark_colour(m);
        let a = if m.dimmed { 0.5 } else { 1. };
        let (sel, hov) = (self.is_selected(&item), self.is_hovered(&item));
        // Facing tick, then the disc over its root.
        let d = geom::dir(m.facing_deg);
        let tick = [point(px(c.0 + d[0] * (r - 1.)), px(c.1 + d[1] * (r - 1.))), point(px(c.0 + d[0] * (r + 7.)), px(c.1 + d[1] * (r + 7.)))];
        poly_line(window, &tick, false, 2.5, col.opacity(a));
        let style = if m.dimmed { BorderStyle::Dashed } else { BorderStyle::Solid };
        disc(window, c, r, Tokens::surface().blend(col.opacity(0.24 * a)), Some((2., col.opacity(a))), style);
        let ini = shape(window, &m.initials, if r >= 12. { mv_ui::size::XS } else { 10. }, FontWeight::SEMIBOLD, Tokens::text_1().opacity(a));
        let w = f32::from(ini.width);
        let _ = ini.paint(point(px(c.0 - w / 2.), px(c.1 - LABEL_LINE / 2.)), px(LABEL_LINE), TextAlign::Left, None, window, cx);
        if sel {
            ring(window, c, r + 4., 2., Tokens::accent());
        } else if hov {
            ring(window, c, r + 3., 1.5, Tokens::text_3());
        }
        self.obstacles.push(Rect::new(c.0 - r - 1., c.1 - r - 1., c.0 + r + 1., c.1 + r + 1.));
        let name = if m.name.trim().is_empty() { m.id.as_str() } else { m.name.as_str() };
        self.label(window, name, sel, c, r + 1., false, if sel || hov { P_FOCUS } else { P_MARK });
    }

    fn camera(&mut self, window: &mut Window, cam: &MapCamera) {
        let item = MapItem::Camera;
        let c = self.view.to_screen(geom::fin2([cam.pos[0], cam.pos[2]]));
        let d = geom::dir(cam.yaw_deg);
        let n = [-d[1], d[0]];
        let at = |k: f32, s: f32| point(px(c.0 + d[0] * k + n[0] * s), px(c.1 + d[1] * k + n[1] * s));
        let tri = [at(CAM_R, 0.), at(-CAM_R * 0.6, CAM_R * 0.75), at(-CAM_R * 0.6, -CAM_R * 0.75)];
        poly_fill(window, &tri, Tokens::accent());
        poly_line(window, &tri, true, 1.5, Tokens::surface());
        let (sel, hov) = (self.is_selected(&item), self.is_hovered(&item));
        if sel {
            ring(window, c, CAM_R + 4., 2., Tokens::accent());
        } else if hov {
            ring(window, c, CAM_R + 3., 1.5, Tokens::text_3());
        }
        self.obstacles.push(Rect::new(c.0 - CAM_R, c.1 - CAM_R, c.0 + CAM_R, c.1 + CAM_R));
        let mut parts: Vec<String> = Vec::new();
        if !cam.label.trim().is_empty() {
            parts.push(cam.label.trim().to_string());
        }
        if cam.lens_mm > 0 {
            parts.push(format!("{} mm", cam.lens_mm));
        }
        parts.push(geom::height_label(cam.pos[1]));
        self.label(window, &parts.join(" · "), true, c, CAM_R + 2., false, if sel || hov { P_FOCUS } else { P_CAMERA });
    }

    /// The selected item's turn handle: a dotted stem from the item and a round knob.
    fn handle(&mut self, window: &mut Window) {
        let Some((item, h)) = handle_at(&self.f.scene, self.f.selected.as_ref(), self.f.read_only, &self.view) else { return };
        let Some(centre) = self.f.scene.pos_of(&item) else { return };
        let (c, hs) = (self.view.to_screen(centre), self.view.to_screen(h));
        let on = self.f.hover.as_ref().is_some_and(|(i, handle)| *handle && *i == item);
        let mut pb = PathBuilder::stroke(px(1.5)).dash_array(&[px(3.), px(3.)]);
        pb.move_to(point(px(c.0), px(c.1)));
        pb.line_to(point(px(hs.0), px(hs.1)));
        if let Ok(path) = pb.build() {
            window.paint_path(path, Hsla::from(Tokens::accent().opacity(0.8)));
        }
        disc(window, hs, HANDLE_R, if on { Tokens::accent() } else { Tokens::surface() }, Some((2., Tokens::accent())), BorderStyle::Solid);
        self.obstacles.push(Rect::new(hs.0 - HANDLE_R, hs.1 - HANDLE_R, hs.0 + HANDLE_R, hs.1 + HANDLE_R));
    }

    /// Bottom-left: a bar of a round length with end ticks, and that length.
    fn scale_bar(&mut self, window: &mut Window, cx: &mut App) {
        let (m, len) = geom::scale_bar(self.view.scale, SCALE_BAR_MAX_PX);
        let text = shape(window, &geom::metres_label(m), LABEL_PX, FontWeight::MEDIUM, Tokens::text_2());
        let tw = f32::from(text.width);
        let (x, y) = (self.screen.x0 + EDGE, self.screen.y1 - EDGE - 22.);
        let plate = Rect::at(x, y, len + tw + 25., 22.);
        pill(window, plate);
        let (bx, by) = (x + 8., y + 11.);
        let c = Hsla::from(Tokens::text_2());
        window.paint_quad(fill(Bounds::new(point(px(bx), px(by - 0.75)), gpui_kit::size(px(len), px(1.5))), c));
        for ex in [bx, bx + len - 1.5] {
            window.paint_quad(fill(Bounds::new(point(px(ex), px(by - 4.)), gpui_kit::size(px(1.5), px(8.))), c));
        }
        let _ = text.paint(point(px(bx + len + 7.), px(y + (22. - LABEL_LINE) / 2.)), px(LABEL_LINE), TextAlign::Left, None, window, cx);
        self.obstacles.push(plate);
    }

    /// Queues a label for the marker at screen point `at` reaching `reach` px: tried below,
    /// above, right, left and the diagonals (first centred on it when `inside`).
    #[allow(clippy::too_many_arguments)]
    fn label(&mut self, window: &mut Window, text: &str, strong: bool, at: (f32, f32), reach: f32, inside: bool, prio: u8) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let colour = if strong { Tokens::text_1() } else { Tokens::text_2() };
        let line = shape(window, text, LABEL_PX, if strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }, colour);
        let w = f32::from(line.width) + 2. * LABEL_PAD_X;
        let mut spots = Vec::with_capacity(9);
        if inside {
            spots.push((at.0 - w / 2., at.1 - LABEL_H / 2.));
        }
        spots.extend(geom::spots_around(at, reach, w, LABEL_H));
        self.labels.push(Lab { line, prio, w, spots });
    }

    fn draw_labels(&mut self, window: &mut Window, cx: &mut App) {
        let mut labs = std::mem::take(&mut self.labels);
        labs.sort_by_key(|l| l.prio);
        let reqs: Vec<LabelReq> = labs.iter().map(|l| LabelReq { w: l.w, h: LABEL_H, spots: l.spots.clone() }).collect();
        let placed = geom::place_labels(&reqs, &self.obstacles, self.screen.inflate(-4.));
        for (lab, r) in labs.iter().zip(placed) {
            let Some(r) = r else { continue };
            pill(window, r);
            let _ = lab.line.paint(point(px(r.x0 + LABEL_PAD_X), px(r.y0 + (LABEL_H - LABEL_LINE) / 2.)), px(LABEL_LINE), TextAlign::Left, None, window, cx);
        }
    }
}

/// Doors and windows are cut into the walls rather than standing on the floor.
fn is_opening(l: &Landmark) -> bool {
    matches!(l.kind, LandmarkKind::Door | LandmarkKind::Window)
}

/// A character's colour: the signature colour when it reads on the floor in this theme, else
/// the theme's categorical hue for the character.
fn mark_colour(m: &MapMark) -> Rgba {
    match m.colour.map(rgb) {
        Some(c) if contrast(c, Tokens::surface()) >= 2.2 => c,
        _ => Tokens::hue(Hue::ALL[m.hue % Hue::ALL.len()]),
    }
}

fn bounds_of(r: Rect) -> Bounds<Pixels> {
    Bounds::new(point(px(r.x0), px(r.y0)), gpui_kit::size(px(r.w()), px(r.h())))
}

/// A label's backdrop: a small rounded plate so text reads over the grid, cone and shading.
fn pill(window: &mut Window, r: Rect) {
    window.paint_quad(quad(bounds_of(r), px(radius::XS), Hsla::from(Tokens::surface().opacity(0.94)), px(1.), Hsla::from(Tokens::border()), BorderStyle::Solid));
}

fn poly_fill(window: &mut Window, pts: &[Point<Pixels>], c: Rgba) {
    if pts.len() < 3 {
        return;
    }
    let mut b = PathBuilder::fill();
    b.add_polygon(pts, true);
    if let Ok(path) = b.build() {
        window.paint_path(path, Hsla::from(c));
    }
}

fn poly_line(window: &mut Window, pts: &[Point<Pixels>], closed: bool, w: f32, c: Rgba) {
    if pts.len() < 2 {
        return;
    }
    let mut b = PathBuilder::stroke(px(w));
    b.add_polygon(pts, closed);
    if let Ok(path) = b.build() {
        window.paint_path(path, Hsla::from(c));
    }
}

/// A filled circle (a fully rounded quad: crisp at every size), with an optional border.
fn disc(window: &mut Window, c: (f32, f32), r: f32, bg: Rgba, border: Option<(f32, Rgba)>, style: BorderStyle) {
    let b = Bounds::new(point(px(c.0 - r), px(c.1 - r)), gpui_kit::size(px(2. * r), px(2. * r)));
    let (w, bc) = border.unwrap_or((0., bg));
    window.paint_quad(quad(b, px(r), Hsla::from(bg), px(w), Hsla::from(bc), style));
}

/// An empty circle outline.
fn ring(window: &mut Window, c: (f32, f32), r: f32, w: f32, col: Rgba) {
    let b = Bounds::new(point(px(c.0 - r), px(c.1 - r)), gpui_kit::size(px(2. * r), px(2. * r)));
    window.paint_quad(quad(b, px(r), Hsla::from(Tokens::base().alpha(0.)), px(w), Hsla::from(col), BorderStyle::Solid));
}

/// One line of text in the window's UI font.
fn shape(window: &mut Window, text: &str, size: f32, weight: FontWeight, color: Rgba) -> ShapedLine {
    let mut font = window.text_style().font();
    font.weight = weight;
    let run = TextRun { len: text.len(), font, color: Hsla::from(color), ..Default::default() };
    window.text_system().shape_line(SharedString::from(text.to_string()), px(size), &[run], None)
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{landmark_text, station_text, LANDMARK_CHARS};
    use mv_anime::Station;

    #[test]
    fn stations_are_numbered_and_landmark_names_stay_short() {
        let s = Station { id: "st_door".into(), name: "at the door, looking in".into(), lens_mm: 35, ..Default::default() };
        assert_eq!(station_text(3, &s), "3 · at the door, looking in · 35 mm");
        let bare = Station { id: "st_x".into(), ..Default::default() };
        assert_eq!(station_text(1, &bare), "1 · st_x");
        assert_eq!(landmark_text("Window", false), "Window");
        let long = landmark_text("Convenience store frontage with green sign", false);
        assert!(long.ends_with('…') && long.chars().count() <= LANDMARK_CHARS);
        assert_eq!(landmark_text("Convenience store frontage with green sign", true), "Convenience store frontage with green sign", "whole when it has the focus");
    }
}
