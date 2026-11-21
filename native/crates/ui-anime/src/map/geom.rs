//! The map's pure maths (no GPUI, unit-tested): plan metres <-> screen pixels, fitting a plan
//! into the available room, grid and scale-bar steps, snapping, hit testing, the half-plane on
//! one side of the 180-degree line, and label placement without overlaps.
//!
//! Plan convention (design 6.5): metres, origin at the plan centre, x right, z down (south);
//! yaw 0 = north = screen up, clockwise. The screen has x right, y down, so a plan point maps
//! to the screen by a uniform scale and a shift: no flip.

/// A plan point (x, z) in metres.
pub type P2 = [f32; 2];

/// An axis-aligned rectangle; used both for plan metres (x, z) and screen pixels (x, y).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// The rectangle spanning two corners, in either order.
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }

    /// The rectangle with top-left corner (x, y) and size w x h.
    pub fn at(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect::new(x, y, x + w, y + h)
    }

    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2., (self.y0 + self.y1) / 2.)
    }

    /// No area (or not finite).
    pub fn is_empty(&self) -> bool {
        !(self.w() > 0. && self.h() > 0.)
    }

    /// The two overlap by more than a touching edge.
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    /// `self` lies wholly within `o`.
    pub fn within(&self, o: &Rect) -> bool {
        self.x0 >= o.x0 && self.y0 >= o.y0 && self.x1 <= o.x1 && self.y1 <= o.y1
    }

    pub fn inflate(&self, d: f32) -> Rect {
        Rect::new(self.x0 - d, self.y0 - d, self.x1 + d, self.y1 + d)
    }

    /// Grown to take in the point (x, y).
    pub fn include(&self, x: f32, y: f32) -> Rect {
        Rect { x0: self.x0.min(x), y0: self.y0.min(y), x1: self.x1.max(x), y1: self.y1.max(y) }
    }

    /// Clockwise from the top-left.
    pub fn corners(&self) -> [P2; 4] {
        [[self.x0, self.y0], [self.x1, self.y0], [self.x1, self.y1], [self.x0, self.y1]]
    }
}

/// `v` when finite, else 0 (plan files are user data: never let a NaN reach the painter).
pub fn fin(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.
    }
}

/// [`fin`] for a point.
pub fn fin2(p: P2) -> P2 {
    [fin(p[0]), fin(p[1])]
}

// ---------------------------------------------------------------------------------- view

/// The smallest plan area a view shows, so a near-empty plan is not blown up absurdly.
pub const MIN_EXTENT_M: f32 = 4.;

/// How the plan sits on the screen: `scale` pixels per metre, plan point `centre` drawn at
/// screen point `screen_centre`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub centre: P2,
    pub screen_centre: (f32, f32),
}

impl View {
    pub fn to_screen(&self, p: P2) -> (f32, f32) {
        (self.screen_centre.0 + (p[0] - self.centre[0]) * self.scale, self.screen_centre.1 + (p[1] - self.centre[1]) * self.scale)
    }

    pub fn to_world(&self, s: (f32, f32)) -> P2 {
        [self.centre[0] + (s.0 - self.screen_centre.0) / self.scale, self.centre[1] + (s.1 - self.screen_centre.1) / self.scale]
    }

    /// A plan rectangle on the screen.
    pub fn rect_to_screen(&self, r: Rect) -> Rect {
        let (a, b) = (self.to_screen([r.x0, r.y0]), self.to_screen([r.x1, r.y1]));
        Rect::new(a.0, a.1, b.0, b.1)
    }

    /// The plan area the screen rectangle shows.
    pub fn rect_to_world(&self, r: Rect) -> Rect {
        let (a, b) = (self.to_world((r.x0, r.y0)), self.to_world((r.x1, r.y1)));
        Rect::new(a[0], a[1], b[0], b[1])
    }
}

/// The view that shows all of `world` inside `screen` with `pad` pixels on every side, centred,
/// keeping the plan's proportions. Never returns a zero, negative or non-finite scale.
pub fn fit(world: Rect, screen: Rect, pad: f32) -> View {
    let (ww, wh) = (fin(world.w()).max(MIN_EXTENT_M), fin(world.h()).max(MIN_EXTENT_M));
    let (aw, ah) = ((fin(screen.w()) - 2. * pad).max(1.), (fin(screen.h()) - 2. * pad).max(1.));
    let scale = (aw / ww).min(ah / wh);
    let scale = if scale.is_finite() && scale > 0. { scale.clamp(0.01, 10_000.) } else { 1. };
    let c = world.centre();
    let sc = screen.centre();
    View { scale, centre: fin2([c.0, c.1]), screen_centre: (fin(sc.0), fin(sc.1)) }
}

/// The plan area a map shows: the floor plan (`w` x `d` around the origin) and every item,
/// at least [`MIN_EXTENT_M`] on each side.
pub fn extent(plan_w: f32, plan_d: f32, points: &[P2]) -> Rect {
    let (w, d) = (fin(plan_w), fin(plan_d));
    let mut r = (w > 0. && d > 0.).then(|| Rect::new(-w / 2., -d / 2., w / 2., d / 2.));
    for p in points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
        r = Some(match r {
            Some(r) => r.include(p[0], p[1]),
            None => Rect::new(p[0], p[1], p[0], p[1]),
        });
    }
    let r = r.unwrap_or(Rect::new(-2., -2., 2., 2.));
    let (cx, cz) = r.centre();
    let (hw, hd) = (r.w().max(MIN_EXTENT_M) / 2., r.h().max(MIN_EXTENT_M) / 2.);
    Rect::new(cx - hw, cz - hd, cx + hw, cz + hd)
}

/// The floor rectangle of a plan, `None` when the plan has no size yet.
pub fn floor(plan_w: f32, plan_d: f32) -> Option<Rect> {
    let (w, d) = (fin(plan_w), fin(plan_d));
    (w > 0. && d > 0.).then(|| Rect::new(-w / 2., -d / 2., w / 2., d / 2.))
}

// ---------------------------------------------------------------------- grid, scale bar

/// Minimum spacing of grid lines on screen.
pub const GRID_MIN_PX: f32 = 14.;

/// The grid spacing in metres at `scale` px/m: the finest step whose lines stay
/// [`GRID_MIN_PX`] apart.
pub fn grid_step(scale: f32) -> f32 {
    const STEPS: [f32; 9] = [0.25, 0.5, 1., 2., 5., 10., 20., 50., 100.];
    STEPS.into_iter().find(|s| s * scale >= GRID_MIN_PX).unwrap_or(100.)
}

/// The spacing of the stronger grid lines for a grid of `step` metres.
pub fn major_step(step: f32) -> f32 {
    match step {
        s if s <= 0.5 => 1.,
        s if s <= 1. => 5.,
        s if s <= 5. => 10.,
        s if s <= 10. => 50.,
        _ => 100.,
    }
}

/// `v` is (close to) a multiple of `step`.
pub fn on_step(v: f32, step: f32) -> bool {
    step > 0. && ((v / step) - (v / step).round()).abs() < 1e-3
}

/// Grid line positions from `lo` to `hi` every `step`, at most `cap` of them.
pub fn grid_lines(lo: f32, hi: f32, step: f32, cap: usize) -> Vec<f32> {
    if !(step > 0.) || !(hi >= lo) || !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    let first = (lo / step).ceil() as i64;
    (0..cap as i64).map(|k| (first + k) as f32 * step).take_while(|v| *v <= hi + 1e-4).collect()
}

/// The scale bar: the longest round length (metres) that is at most `max_px` long, and its
/// length on screen.
pub fn scale_bar(scale: f32, max_px: f32) -> (f32, f32) {
    const NICE: [f32; 11] = [0.25, 0.5, 1., 2., 5., 10., 20., 50., 100., 200., 500.];
    let m = NICE.into_iter().rfind(|m| m * scale <= max_px).unwrap_or(NICE[0]);
    (m, m * scale)
}

/// "50 cm", "1 m", "2.5 m".
pub fn metres_label(m: f32) -> String {
    let m = fin(m);
    if m > 0. && m < 1. {
        format!("{} cm", (m * 100.).round())
    } else if (m - m.round()).abs() < 1e-3 {
        format!("{} m", m.round())
    } else {
        format!("{m:.1} m")
    }
}

/// "1.4 m" for a height or distance.
pub fn height_label(m: f32) -> String {
    format!("{:.1} m", fin(m))
}

// -------------------------------------------------------------------- snapping, angles

/// `v` to the nearest multiple of `step` (no snapping for a step of 0); never "-0".
pub fn snap(v: f32, step: f32) -> f32 {
    if !(step > 0.) || !v.is_finite() {
        return fin(v);
    }
    // Round twice: to the step, then to 1e-4, so 0.1 steps do not print as 0.30000001.
    (((v / step).round() * step) * 10_000.).round() / 10_000. + 0.
}

pub fn snap2(p: P2, step: f32) -> P2 {
    [snap(p[0], step), snap(p[1], step)]
}

/// An angle in [0, 360).
pub fn norm_deg(d: f32) -> f32 {
    if !d.is_finite() {
        return 0.;
    }
    let r = d.rem_euclid(360.);
    if r >= 360. - 1e-4 {
        0.
    } else {
        r + 0.
    }
}

/// An angle snapped to `step` degrees, in [0, 360).
pub fn snap_deg(d: f32, step: f32) -> f32 {
    norm_deg(snap(norm_deg(d), step))
}

/// The unit plan vector of a yaw (0 = north = up, clockwise).
pub fn dir(yaw_deg: f32) -> P2 {
    let r = fin(yaw_deg).to_radians();
    [r.sin(), -r.cos()]
}

/// The yaw that looks from `from` to `to`; `None` when they are the same point.
pub fn yaw_to(from: P2, to: P2) -> Option<f32> {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    (dx.hypot(dz) > 1e-6 && dx.is_finite() && dz.is_finite()).then(|| norm_deg(dx.atan2(-dz).to_degrees()))
}

/// `v` turned clockwise (on the plan and the screen) by `deg`.
pub fn rot(v: P2, deg: f32) -> P2 {
    let (s, c) = fin(deg).to_radians().sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

pub fn add(a: P2, b: P2) -> P2 {
    [a[0] + b[0], a[1] + b[1]]
}

pub fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}

pub fn mul(a: P2, k: f32) -> P2 {
    [a[0] * k, a[1] * k]
}

pub fn dist(a: P2, b: P2) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// `p` kept inside `r` (unchanged when `r` has no area).
pub fn clamp_in(p: P2, r: Rect) -> P2 {
    if r.is_empty() {
        return p;
    }
    [p[0].clamp(r.x0, r.x1), p[1].clamp(r.y0, r.y1)]
}

/// The four corners of a landmark-like box: centre `pos`, `size` = (width along its own x,
/// depth along its own z), turned clockwise by `rot_deg`. Order: clockwise from local top-left.
pub fn box_corners(pos: P2, size: P2, rot_deg: f32) -> [P2; 4] {
    let (hx, hz) = (fin(size[0]).abs() / 2., fin(size[1]).abs() / 2.);
    [[-hx, -hz], [hx, -hz], [hx, hz], [-hx, hz]].map(|c| add(pos, rot(c, rot_deg)))
}

/// `p` lies in the turned box, grown by `tol` metres on every side.
pub fn in_box(p: P2, pos: P2, size: P2, rot_deg: f32, tol: f32) -> bool {
    let l = rot(sub(p, pos), -rot_deg);
    l[0].abs() <= fin(size[0]).abs() / 2. + tol && l[1].abs() <= fin(size[1]).abs() / 2. + tol
}

/// `n + 1` points on the circle around `c` from yaw `a0` to yaw `a1` the short way round.
pub fn arc(c: P2, r: f32, a0: f32, a1: f32, n: usize) -> Vec<P2> {
    let delta = (a1 - a0 + 540.).rem_euclid(360.) - 180.;
    let n = n.max(1);
    (0..=n).map(|i| add(c, mul(dir(a0 + delta * i as f32 / n as f32), r))).collect()
}

// ---------------------------------------------------------------------------- hit test

/// The pickable outline of one item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A marker drawn at a fixed screen size: plan centre and radius in pixels.
    Disc { c: P2, r_px: f32 },
    /// A box on the plan (a landmark); `min_px` keeps a tiny one pickable.
    Box { pos: P2, size: P2, rot_deg: f32, min_px: f32 },
}

/// One thing a press can take. Lower layers win (handles over markers over landmarks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub shape: Shape,
    pub layer: u8,
}

/// The index of the target under screen point `s` within `tol_px`: the lowest layer first; in a
/// layer the nearest disc centre, else the smallest box (a table on a rug takes the press).
pub fn hit(targets: &[Target], view: &View, s: (f32, f32), tol_px: f32) -> Option<usize> {
    let scale = view.scale.max(1e-3);
    let mut best: Option<(u8, f32, usize)> = None;
    for (i, t) in targets.iter().enumerate() {
        let score = match t.shape {
            Shape::Disc { c, r_px } => {
                let cs = view.to_screen(c);
                let d = (cs.0 - s.0).hypot(cs.1 - s.1);
                (d <= r_px + tol_px).then_some(d)
            }
            Shape::Box { pos, size, rot_deg, min_px } => {
                let min_m = min_px / scale;
                let size = [fin(size[0]).abs().max(min_m), fin(size[1]).abs().max(min_m)];
                in_box(view.to_world(s), pos, size, rot_deg, tol_px / scale).then(|| size[0] * size[1] * scale * scale)
            }
        };
        let Some(score) = score else { continue };
        let better = match best {
            None => true,
            Some((layer, sc, _)) => t.layer < layer || (t.layer == layer && score < sc),
        };
        if better {
            best = Some((t.layer, score, i));
        }
    }
    best.map(|(_, _, i)| i)
}

// --------------------------------------------------------------------- 180-degree line

/// Twice the signed area of (a, b, p): which side of the line a->b the point is on.
pub fn cross(a: P2, b: P2, p: P2) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// The part of the convex polygon `poly` on the same side of the line through `a` and `b` as
/// `keep` (Sutherland-Hodgman against one edge). Empty when `keep` is on the line or a = b.
pub fn clip_half(poly: &[P2], a: P2, b: P2, keep: P2) -> Vec<P2> {
    let k = cross(a, b, keep);
    if !(k.abs() > 1e-6) || poly.len() < 3 {
        return Vec::new();
    }
    let side = |p: P2| cross(a, b, p) * k.signum();
    let cut = |p: P2, q: P2| {
        let (sp, sq) = (side(p), side(q));
        let t = sp / (sp - sq);
        add(p, mul(sub(q, p), t))
    };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (prev, cur) = (poly[(i + poly.len() - 1) % poly.len()], poly[i]);
        let (pin, cin) = (side(prev) >= 0., side(cur) >= 0.);
        if cin {
            if !pin {
                out.push(cut(prev, cur));
            }
            out.push(cur);
        } else if pin {
            out.push(cut(prev, cur));
        }
    }
    // A corner on the line comes out twice (as the corner and as the cut): drop repeats.
    out.dedup_by(|a, b| dist(*a, *b) < 1e-5);
    while out.len() > 1 && dist(out[0], out[out.len() - 1]) < 1e-5 {
        out.pop();
    }
    if out.len() < 3 {
        out.clear();
    }
    out
}

/// The part of the convex polygon `poly` inside the rectangle (empty when none is).
pub fn clip_to_rect(poly: &[P2], r: Rect) -> Vec<P2> {
    if r.is_empty() || poly.len() < 3 {
        return Vec::new();
    }
    let (cx, cy) = r.centre();
    let [a, b, c, d] = r.corners();
    let mut out = poly.to_vec();
    for (p, q) in [(a, b), (b, c), (c, d), (d, a)] {
        out = clip_half(&out, p, q, [cx, cy]);
        if out.is_empty() {
            break;
        }
    }
    out
}

/// The part of the segment p-q inside the rectangle, `None` when it misses.
pub fn segment_in_rect(p: P2, q: P2, r: Rect) -> Option<(P2, P2)> {
    let d = sub(q, p);
    let (mut t0, mut t1) = (0f32, 1f32);
    for (s, dv, lo, hi) in [(p[0], d[0], r.x0, r.x1), (p[1], d[1], r.y0, r.y1)] {
        if dv.abs() < 1e-9 {
            if s < lo || s > hi {
                return None;
            }
        } else {
            let (u, v) = ((lo - s) / dv, (hi - s) / dv);
            t0 = t0.max(u.min(v));
            t1 = t1.min(u.max(v));
        }
    }
    (t0 < t1).then(|| (add(p, mul(d, t0)), add(p, mul(d, t1))))
}

/// Where the endless line through `a` and `b` crosses the rectangle, `None` when it misses or
/// a = b.
pub fn line_in_rect(a: P2, b: P2, r: Rect) -> Option<(P2, P2)> {
    let d = sub(b, a);
    if d[0].hypot(d[1]) < 1e-6 || r.is_empty() {
        return None;
    }
    let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
    for (p, dv, lo, hi) in [(a[0], d[0], r.x0, r.x1), (a[1], d[1], r.y0, r.y1)] {
        if dv.abs() < 1e-9 {
            if p < lo || p > hi {
                return None;
            }
        } else {
            let (u, v) = ((lo - p) / dv, (hi - p) / dv);
            t0 = t0.max(u.min(v));
            t1 = t1.min(u.max(v));
        }
    }
    (t0 <= t1 && t0.is_finite() && t1.is_finite()).then(|| (add(a, mul(d, t0)), add(a, mul(d, t1))))
}

// ------------------------------------------------------------------------------- labels

/// A label to place: its size and the top-left corners it may take, best first.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelReq {
    pub w: f32,
    pub h: f32,
    pub spots: Vec<(f32, f32)>,
}

/// Gap kept between two labels.
pub const LABEL_GAP: f32 = 2.;

/// Places labels in order (the first wins a contested spot): each takes its first spot that
/// stays inside `area` and overlaps neither an obstacle (a marker) nor a label placed before
/// it. A label with no free spot is left out (`None`) rather than drawn over something.
pub fn place_labels(reqs: &[LabelReq], obstacles: &[Rect], area: Rect) -> Vec<Option<Rect>> {
    let mut placed: Vec<Rect> = Vec::new();
    reqs.iter()
        .map(|q| {
            let spot = q.spots.iter().map(|&(x, y)| Rect::at(x, y, q.w, q.h)).find(|r| {
                r.within(&area) && !obstacles.iter().any(|o| o.intersects(r)) && !placed.iter().any(|p| p.inflate(LABEL_GAP).intersects(r))
            });
            if let Some(r) = spot {
                placed.push(r);
            }
            spot
        })
        .collect()
}

/// The spots around a round marker of radius `r` at `c` for a `w` x `h` label: below, above,
/// right, left, then the four diagonals.
pub fn spots_around(c: (f32, f32), r: f32, w: f32, h: f32) -> Vec<(f32, f32)> {
    let g = r + 3.;
    vec![
        (c.0 - w / 2., c.1 + g),
        (c.0 - w / 2., c.1 - g - h),
        (c.0 + g + 1., c.1 - h / 2.),
        (c.0 - g - 1. - w, c.1 - h / 2.),
        (c.0 + g * 0.7, c.1 + g * 0.7),
        (c.0 - g * 0.7 - w, c.1 + g * 0.7),
        (c.0 + g * 0.7, c.1 - g * 0.7 - h),
        (c.0 - g * 0.7 - w, c.1 - g * 0.7 - h),
    ]
}

// --------------------------------------------------------------------------------- text

/// Two letters for a character marker (the same as every screen's, `widgets::initials`).
pub fn initials(name: &str) -> String {
    crate::widgets::initials(name)
}

/// "#RRGGBB" or "RRGGBB" as 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{
        arc, box_corners, clamp_in, clip_half, clip_to_rect, dir, extent, fit, floor, grid_lines, grid_step, height_label, hit, in_box, initials, line_in_rect, major_step, metres_label, norm_deg, on_step,
        parse_hex, place_labels, scale_bar, segment_in_rect, snap, snap_deg, spots_around, yaw_to, LabelReq, Rect, Shape, Target, View, P2,
    };

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn close2(a: P2, b: P2) -> bool {
        close(a[0], b[0]) && close(a[1], b[1])
    }

    #[test]
    fn world_and_screen_round_trip() {
        let v = View { scale: 40., centre: [1., -2.], screen_centre: (300., 200.) };
        assert_eq!(v.to_screen([1., -2.]), (300., 200.));
        // x right, z down: +1 m east is 40 px right, +1 m south is 40 px down.
        assert_eq!(v.to_screen([2., -1.]), (340., 240.));
        for p in [[0., 0.], [-3.5, 7.25], [12., -0.1]] {
            assert!(close2(v.to_world(v.to_screen(p)), p));
        }
        let r = v.rect_to_world(v.rect_to_screen(Rect::new(-1., -1., 2., 3.)));
        assert!(close(r.x0, -1.) && close(r.y1, 3.));
    }

    #[test]
    fn fitting_keeps_proportions_and_centres() {
        // A 10 x 5 m room in an 640 x 480 box with 20 px padding: width limits (600/10 = 60).
        let v = fit(Rect::new(-5., -2.5, 5., 2.5), Rect::at(100., 50., 640., 480.), 20.);
        assert!(close(v.scale, 60.));
        assert_eq!(v.to_screen([0., 0.]), (420., 290.));
        let (l, t) = v.to_screen([-5., -2.5]);
        assert!(close(l, 120.) && close(t, 140.), "{l} {t}");
        // A tall room: height limits.
        let v = fit(Rect::new(-2., -10., 2., 10.), Rect::at(0., 0., 400., 220.), 10.);
        assert!(close(v.scale, 10.));
        // A zero-sized screen or plan never gives a zero or NaN scale.
        for v in [fit(Rect::default(), Rect::default(), 30.), fit(Rect::new(f32::NAN, 0., 1., 1.), Rect::at(0., 0., 200., 200.), 0.)] {
            assert!(v.scale.is_finite() && v.scale > 0., "{v:?}");
            assert!(v.to_screen([1., 1.]).0.is_finite());
        }
    }

    #[test]
    fn extent_takes_the_plan_and_every_item() {
        let r = extent(8., 6., &[]);
        assert_eq!(r, Rect::new(-4., -3., 4., 3.));
        // A camera outside the room widens the view.
        let r = extent(8., 6., &[[0., 5.]]);
        assert_eq!((r.y0, r.y1), (-3., 5.));
        // No plan, one mark: at least MIN_EXTENT around it; NaN items are ignored.
        let r = extent(0., f32::NAN, &[[10., 10.], [f32::NAN, 1.]]);
        assert_eq!(r, Rect::new(8., 8., 12., 12.));
        assert_eq!(extent(0., 0., &[]), Rect::new(-2., -2., 2., 2.));
        assert_eq!(floor(4., 2.), Some(Rect::new(-2., -1., 2., 1.)));
        assert_eq!(floor(0., 2.), None);
    }

    #[test]
    fn grid_and_scale_bar_steps_stay_legible() {
        assert_eq!(grid_step(80.), 0.25);
        assert_eq!(grid_step(40.), 0.5);
        assert_eq!(grid_step(20.), 1.);
        assert_eq!(grid_step(5.), 5.);
        assert_eq!(grid_step(0.001), 100.);
        assert_eq!(major_step(0.5), 1.);
        assert_eq!(major_step(1.), 5.);
        assert!(on_step(5., 5.) && on_step(-10., 5.) && !on_step(2.5, 5.));
        assert_eq!(grid_lines(-1.2, 1.2, 0.5, 100), vec![-1., -0.5, 0., 0.5, 1.]);
        assert_eq!(grid_lines(0., 1000., 1., 10).len(), 10);
        assert!(grid_lines(1., 0., 1., 10).is_empty() && grid_lines(0., 1., 0., 10).is_empty());
        // 40 px/m, at most 120 px: 2 m (80 px).
        assert_eq!(scale_bar(40., 120.), (2., 80.));
        assert_eq!(scale_bar(300., 120.), (0.25, 75.));
        assert_eq!(scale_bar(1., 120.), (100., 100.));
        assert_eq!(metres_label(0.5), "50 cm");
        assert_eq!(metres_label(2.), "2 m");
        assert_eq!(metres_label(2.5), "2.5 m");
        assert_eq!(height_label(1.43), "1.4 m");
    }

    #[test]
    fn snapping_rounds_cleanly() {
        assert_eq!(snap(0.34, 0.1), 0.3);
        assert_eq!(snap(-0.04, 0.1), 0.);
        assert!(snap(-0.04, 0.1).is_sign_positive(), "never -0");
        assert_eq!(snap(1.26, 0.5), 1.5);
        assert_eq!(snap(1.234, 0.), 1.234);
        assert_eq!(snap(f32::NAN, 0.1), 0.);
        assert_eq!(snap_deg(358., 5.), 0.);
        assert_eq!(snap_deg(-7., 5.), 355.);
        assert_eq!(snap_deg(97., 15.), 90.);
        assert_eq!(norm_deg(720.), 0.);
        assert_eq!(norm_deg(-90.), 270.);
        assert_eq!(norm_deg(f32::INFINITY), 0.);
        let room = Rect::new(-2., -1., 2., 1.);
        assert_eq!(clamp_in([5., -3.], room), [2., -1.]);
        assert_eq!(clamp_in([5., -3.], Rect::default()), [5., -3.]);
    }

    #[test]
    fn yaw_follows_the_plan_convention() {
        // 0 = north = up (z negative), clockwise.
        assert!(close2(dir(0.), [0., -1.]));
        assert!(close2(dir(90.), [1., 0.]));
        assert!(close2(dir(180.), [0., 1.]));
        for (from, to, want) in [([0., 0.], [0., -3.], 0.), ([0., 0.], [2., 0.], 90.), ([1., 1.], [1., 4.], 180.), ([0., 0.], [-1., 0.], 270.), ([0., 0.], [1., -1.], 45.)] {
            let got = yaw_to(from, to).unwrap();
            assert!(close(got, want), "{from:?} -> {to:?}: {got}");
        }
        assert_eq!(yaw_to([1., 1.], [1., 1.]), None);
        // The door swing arc: 90 degrees the short way, ends exact.
        let a = arc([0., 0.], 1., 350., 80., 4);
        assert_eq!(a.len(), 5);
        assert!(close2(a[0], dir(350.)) && close2(a[4], dir(80.)) && close2(a[2], dir(35.)));
    }

    #[test]
    fn turned_boxes() {
        // A 2 x 1 table turned 90 degrees clockwise stands 1 wide and 2 deep.
        let c = box_corners([1., 1.], [2., 1.], 90.);
        assert!(close2(c[0], [1.5, 0.]) && close2(c[2], [0.5, 2.]), "{c:?}");
        assert!(in_box([1., 1.9], [1., 1.], [2., 1.], 90., 0.));
        assert!(!in_box([1.9, 1.], [1., 1.], [2., 1.], 90., 0.));
        assert!(in_box([1.55, 1.], [1., 1.], [2., 1.], 90., 0.1));
    }

    #[test]
    fn a_press_takes_the_handle_then_the_nearest_marker_then_the_smallest_box() {
        let v = View { scale: 50., centre: [0., 0.], screen_centre: (200., 200.) };
        let t = [
            Target { shape: Shape::Box { pos: [0., 0.], size: [4., 4.], rot_deg: 0., min_px: 12. }, layer: 4 }, // rug
            Target { shape: Shape::Box { pos: [0., 0.], size: [1., 1.], rot_deg: 0., min_px: 12. }, layer: 4 }, // table
            Target { shape: Shape::Disc { c: [0.2, 0.], r_px: 11. }, layer: 2 },                                  // mark A
            Target { shape: Shape::Disc { c: [0.5, 0.], r_px: 11. }, layer: 2 },                                  // mark B
            Target { shape: Shape::Disc { c: [0.2, -0.6], r_px: 7. }, layer: 0 },                                 // A's handle
        ];
        let s = |p: P2| v.to_screen(p);
        // Between the two marks (10 px from A, 5 px from B): the nearer, B.
        assert_eq!(hit(&t, &v, s([0.4, 0.]), 4.), Some(3));
        assert_eq!(hit(&t, &v, s([0.2, 0.]), 4.), Some(2));
        // The handle beats everything under it.
        assert_eq!(hit(&t, &v, s([0.2, -0.62]), 4.), Some(4));
        // Off the markers: the table inside the rug.
        assert_eq!(hit(&t, &v, s([-0.3, 0.3]), 4.), Some(1));
        assert_eq!(hit(&t, &v, s([1.5, 1.5]), 4.), Some(0));
        // Outside everything (beyond tolerance).
        assert_eq!(hit(&t, &v, s([2.2, 2.2]), 4.), None);
        // Tolerance: 3 px outside the rug still takes it.
        assert_eq!(hit(&t, &v, (s([2., 0.]).0 + 3., s([2., 0.]).1), 4.), Some(0));
        // A tiny landmark stays pickable through min_px (2 cm box, 12 px floor).
        let tiny = [Target { shape: Shape::Box { pos: [0., 0.], size: [0.02, 0.02], rot_deg: 0., min_px: 12. }, layer: 4 }];
        assert_eq!(hit(&tiny, &v, (205., 200.), 0.), Some(0));
        assert_eq!(hit(&[], &v, (0., 0.), 4.), None);
    }

    #[test]
    fn the_cameras_half_of_the_room() {
        let room = Rect::new(-2., -2., 2., 2.).corners();
        // Line along x = 0 (two characters facing each other north-south); camera east.
        let half = clip_half(&room, [0., -1.], [0., 1.], [1.5, 0.]);
        assert_eq!(half.len(), 4);
        assert!(half.iter().all(|p| p[0] >= -1e-4), "{half:?}");
        let area: f32 = (0..half.len()).map(|i| {
            let (a, b) = (half[i], half[(i + 1) % half.len()]);
            a[0] * b[1] - b[0] * a[1]
        }).sum::<f32>().abs() / 2.;
        assert!(close(area, 8.));
        // Camera west: the other half.
        assert!(clip_half(&room, [0., -1.], [0., 1.], [-1., 0.]).iter().all(|p| p[0] <= 1e-4));
        // A diagonal line leaves a triangle.
        assert_eq!(clip_half(&room, [-2., 2.], [2., -2.], [2., 2.]).len(), 3);
        // Camera on the line, or both characters on one spot: nothing to shade.
        assert!(clip_half(&room, [0., -1.], [0., 1.], [0., 0.5]).is_empty());
        assert!(clip_half(&room, [1., 1.], [1., 1.], [0., 0.]).is_empty());
    }

    #[test]
    fn the_180_line_runs_through_the_whole_room() {
        let r = Rect::new(-4., -3., 4., 3.);
        let (a, b) = line_in_rect([-1., 0.], [1., 0.], r).unwrap();
        assert!(close2(a, [-4., 0.]) && close2(b, [4., 0.]));
        let (a, b) = line_in_rect([0., 0.], [1., 1.], r).unwrap();
        assert!(close2(a, [-3., -3.]) && close2(b, [3., 3.]));
        assert_eq!(line_in_rect([0., 5.], [1., 5.], r), None);
        assert_eq!(line_in_rect([1., 1.], [1., 1.], r), None);
    }

    #[test]
    fn labels_never_overlap() {
        let area = Rect::at(0., 0., 400., 300.);
        let marker = Rect::at(90., 90., 20., 20.);
        let req = |c: (f32, f32)| LabelReq { w: 40., h: 16., spots: spots_around(c, 10., 40., 16.) };
        // Two markers side by side: the second label cannot take "below" (the first did), so it
        // goes above.
        let out = place_labels(&[req((100., 100.)), req((110., 100.))], &[marker], area);
        let (a, b) = (out[0].unwrap(), out[1].unwrap());
        assert!(!a.intersects(&b) && !a.intersects(&marker) && !b.intersects(&marker));
        assert_eq!(a, Rect::at(80., 113., 40., 16.));
        assert!(b.y1 <= 100.);
        // A marker in the corner: only the spots inside the map.
        let out = place_labels(&[req((5., 5.))], &[], area);
        assert!(out[0].unwrap().within(&area));
        // Nowhere to go: left out instead of drawn over.
        let tiny = Rect::at(0., 0., 30., 30.);
        assert_eq!(place_labels(&[req((15., 15.))], &[], tiny), vec![None]);
    }

    #[test]
    fn marker_text() {
        assert_eq!(initials("Sung Jinwoo"), "SJ");
        assert_eq!(initials("jinwoo"), "Ji");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("cha_hae-in"), "CH");
        assert_eq!(initials("성진우"), "성진");
        assert_eq!(parse_hex("#3A6FF2"), Some(0x3a6ff2));
        assert_eq!(parse_hex("3a6ff2"), Some(0x3a6ff2));
        assert_eq!(parse_hex("#3A6FF"), None);
        assert_eq!(parse_hex("blue"), None);
    }

    #[test]
    fn a_view_cone_stops_at_the_walls() {
        let room = Rect::new(-2., -2., 2., 2.);
        // A cone from the middle reaching far past the room is cut at the walls.
        let cut = clip_to_rect(&[[0., 0.], [-10., -20.], [10., -20.]], room);
        assert!(cut.len() >= 3);
        assert!(cut.iter().all(|p| p[0] >= -2.0001 && p[0] <= 2.0001 && p[1] >= -2.0001 && p[1] <= 2.0001));
        assert!(clip_to_rect(&[[5., 5.], [6., 5.], [5., 6.]], room).is_empty(), "wholly outside: nothing");
        // An edge from inside to far outside ends on the wall; one that misses is dropped.
        let (p, q) = segment_in_rect([0., 0.], [0., -10.], room).unwrap();
        assert_eq!((p, q), ([0., 0.], [0., -2.]));
        assert!(segment_in_rect([5., 5.], [6., 9.], room).is_none());
    }
}
