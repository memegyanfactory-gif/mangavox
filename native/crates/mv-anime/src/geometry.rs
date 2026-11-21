//! Camera geometry (design 6.5, pure). Metres, plan origin at the plan centre, x right, z down
//! (south), y up; yaw 0 = north (screen up), clockwise; pitch + up; frame 36 x 20.25 mm.
//!
//! A yaw of θ points along (sin θ, −cos θ) on the plan: 0 north (−z), 90 east (+x), 180 south.

use std::collections::BTreeMap;

use crate::model::{Angle, Camera, Derived, FloorPlan, LookAt, Mark, Place, Pose, Shot};

/// Half the frame width and height in millimetres (36 x 20.25 mm, 16:9).
const HALF_W_MM: f32 = 18.;
const HALF_H_MM: f32 = 10.125;
/// The lens a camera without one uses.
pub const DEFAULT_LENS_MM: u16 = 35;
/// Eye height of a standing character of the default height (1.6 x height_m / 1.7).
pub const EYE_HEIGHT_M: f32 = 1.6;
/// How far outside the frame a point may sit and still get a screen x.
const SCREEN_MARGIN: f32 = 0.2;

/// Lens, horizon, perspective and side phrases for the prompt compiler.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraWords {
    /// "wide 24 mm: camera close, space stretched".
    pub lens: String,
    /// "the horizon crosses the characters' eyes at 48% from the top".
    pub horizon: String,
    /// "verticals stay vertical, one- or two-point perspective".
    pub perspective: String,
    /// "tilted 15°", or empty.
    pub roll: String,
}

/// A usable focal length: 0, negative or absurd values fall back to 35 mm.
fn lens(lens_mm: f32) -> f32 {
    if lens_mm.is_finite() && lens_mm >= 4. {
        lens_mm.min(2000.)
    } else {
        DEFAULT_LENS_MM as f32
    }
}

/// Unit plan direction (x, z) of a yaw.
fn dir(yaw_deg: f32) -> [f32; 2] {
    let r = yaw_deg.to_radians();
    [r.sin(), -r.cos()]
}

/// Yaw (0..360) of a plan direction.
fn yaw_of(dx: f32, dz: f32) -> f32 {
    norm_deg(dx.atan2(-dz).to_degrees())
}

/// An angle in 0..360.
fn norm_deg(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.;
    }
    let r = a.rem_euclid(360.);
    if r >= 360. {
        0.
    } else {
        r
    }
}

/// Smallest difference of two angles, 0..180.
fn angle_diff(a: f32, b: f32) -> f32 {
    let d = norm_deg(a - b);
    if d > 180. {
        360. - d
    } else {
        d
    }
}

/// (horizontal, vertical) field of view in degrees: (2·atan(18/f), 2·atan(10.125/f)).
pub fn fov_deg(lens_mm: f32) -> (f32, f32) {
    let f = lens(lens_mm);
    (2. * (HALF_W_MM / f).atan().to_degrees(), 2. * (HALF_H_MM / f).atan().to_degrees())
}

/// The station of a camera on a place, by id.
fn station<'a>(c: &Camera, place: Option<&'a Place>) -> Option<&'a crate::model::Station> {
    let id = c.station.as_deref()?;
    place?.stations.iter().find(|s| s.id == id)
}

/// The camera's position: its own, else its station's, else the plan centre at eye height.
pub fn cam_pos(c: &Camera, place: Option<&Place>) -> [f32; 3] {
    if let Some(p) = c.pos.filter(|p| p.iter().all(|v| v.is_finite())) {
        return p;
    }
    match station(c, place) {
        Some(s) => s.pos,
        None => [0., EYE_HEIGHT_M, 0.],
    }
}

/// Eye height of a character in a pose.
fn eye_height(pose: Pose) -> f32 {
    match pose {
        Pose::Sit => 1.15,
        Pose::Kneel => 0.95,
        Pose::Lie => 0.3,
        Pose::Run | Pose::Fight => 1.5,
        _ => EYE_HEIGHT_M,
    }
}

/// (yaw, pitch) from the camera position to a point.
fn aim(cam: [f32; 3], target: [f32; 3]) -> Option<(f32, f32)> {
    let (dx, dy, dz) = (target[0] - cam[0], target[1] - cam[1], target[2] - cam[2]);
    let flat = (dx * dx + dz * dz).sqrt();
    if !flat.is_finite() || (flat < 1e-3 && dy.abs() < 1e-3) {
        return None;
    }
    let pitch = dy.atan2(flat).to_degrees();
    // Straight up or down: keep a yaw of 0 (north at the top of the frame).
    let yaw = if flat < 1e-3 { 0. } else { yaw_of(dx, dz) };
    Some((yaw, pitch))
}

/// (yaw, pitch) in degrees towards what the camera looks at. A target that cannot be found
/// (a character without a mark, an unknown landmark) falls back to the station's own angles,
/// else to looking north, level.
pub fn look_dir(c: &Camera, marks: &[Mark], place: Option<&Place>) -> (f32, f32) {
    let cam = cam_pos(c, place);
    let target = match &c.look_at {
        LookAt::Angles { yaw_deg, pitch_deg } => {
            let pitch = if pitch_deg.is_finite() { pitch_deg.clamp(-90., 90.) } else { 0. };
            return (norm_deg(*yaw_deg), pitch);
        }
        LookAt::Point(p) => Some(*p).filter(|p| p.iter().all(|v| v.is_finite())),
        LookAt::Character(id) => marks.iter().find(|m| &m.character == id).map(|m| [m.pos[0], eye_height(m.pose), m.pos[1]]),
        LookAt::Landmark(id) => place.and_then(|p| p.plan.landmarks.iter().find(|l| &l.id == id)).map(|l| {
            let h = if l.h_m.is_finite() && l.h_m > 0. { l.h_m / 2. } else { 1. };
            [l.pos[0], h, l.pos[1]]
        }),
    };
    if let Some(a) = target.and_then(|t| aim(cam, t)) {
        return a;
    }
    match station(c, place) {
        Some(s) => (norm_deg(s.yaw_deg), if s.pitch_deg.is_finite() { s.pitch_deg.clamp(-90., 90.) } else { 0. }),
        None => (0., 0.),
    }
}

/// 0.5 + 0.5·tan(pitch)/tan(vfov/2); 0 = top of the frame. Below 0 or above 1 the horizon is
/// outside the frame (looking far down or up). Pitch is kept within ±89.9°.
pub fn horizon_y(pitch_deg: f32, lens_mm: f32) -> f32 {
    let pitch = if pitch_deg.is_finite() { pitch_deg.clamp(-89.9, 89.9) } else { 0. };
    let half_v = (HALF_H_MM / lens(lens_mm)).atan();
    0.5 + 0.5 * pitch.to_radians().tan() / half_v.tan()
}

/// Sign of cross(b − a, cam − a): which side of the a–b line the camera stands on. With x right
/// and z down, 1 means the camera is to the right of a→b as drawn on the plan. Swapping a and b
/// flips the sign; on the line (within 1 mm²) it is 0.
pub fn side_of_line(a: [f32; 2], b: [f32; 2], cam: [f32; 2]) -> i8 {
    let cross = (b[0] - a[0]) * (cam[1] - a[1]) - (b[1] - a[1]) * (cam[0] - a[0]);
    if !cross.is_finite() || cross.abs() < 1e-6 {
        0
    } else if cross > 0. {
        1
    } else {
        -1
    }
}

/// (depth along the view, offset to the right) of a plan point seen from `cam` at `yaw`.
fn view_space(cam: [f32; 3], yaw: f32, p: [f32; 2]) -> (f32, f32) {
    let (d, f) = ([p[0] - cam[0], p[1] - cam[2]], dir(yaw));
    // Right of the view = the view direction turned 90° clockwise.
    let r = [-f[1], f[0]];
    (d[0] * f[0] + d[1] * f[1], d[0] * r[0] + d[1] * r[1])
}

/// Screen x (0..1, 0 = left edge) of a plan point; `None` behind the camera or beyond ±0.2
/// outside the frame. Plan view only: the camera's pitch does not move a point sideways.
pub fn screen_x(cam: [f32; 3], yaw: f32, lens_mm: f32, p: [f32; 2]) -> Option<f32> {
    let (depth, right) = view_space(cam, yaw, p);
    if !depth.is_finite() || depth <= 1e-4 {
        return None;
    }
    let x = 0.5 + 0.5 * (right / depth) * lens(lens_mm) / HALF_W_MM;
    (x.is_finite() && (-SCREEN_MARGIN..=1. + SCREEN_MARGIN).contains(&x)).then_some(x)
}

/// Landmark ids hit by the ray from the camera past `mark` (±2°, widened by the landmark's own
/// size), nearest first. Empty when the mark is behind the camera or on it.
pub fn behind(cam: [f32; 3], yaw: f32, mark: [f32; 2], plan: &FloorPlan) -> Vec<String> {
    let (depth, _) = view_space(cam, yaw, mark);
    let (mx, mz) = (mark[0] - cam[0], mark[1] - cam[2]);
    let mark_dist = (mx * mx + mz * mz).sqrt();
    if !depth.is_finite() || depth <= 1e-4 || mark_dist < 1e-3 {
        return Vec::new();
    }
    let ray = yaw_of(mx, mz);
    let mut hits: Vec<(f32, String)> = plan
        .landmarks
        .iter()
        .filter_map(|l| {
            let (lx, lz) = (l.pos[0] - cam[0], l.pos[1] - cam[2]);
            let dist = (lx * lx + lz * lz).sqrt();
            if !dist.is_finite() || dist <= mark_dist {
                return None;
            }
            let half = l.size.iter().copied().filter(|v| v.is_finite()).fold(0f32, f32::max).max(0.) / 2.;
            let spread = 2. + (half / dist).atan().to_degrees();
            (angle_diff(yaw_of(lx, lz), ray) <= spread).then(|| (dist, l.id.clone()))
        })
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.into_iter().map(|(_, id)| id).collect()
}

/// The view cone on the plan: apex, left edge end, right edge end (each edge `reach_m` long).
/// A yaw that is not a number looks north.
pub fn view_cone(cam: [f32; 3], yaw: f32, lens_mm: f32, reach_m: f32) -> [[f32; 2]; 3] {
    let half = fov_deg(lens_mm).0 / 2.;
    let yaw = norm_deg(yaw);
    let reach = if reach_m.is_finite() { reach_m.max(0.) } else { 0. };
    let apex = [cam[0], cam[2]];
    let end = |a: f32| {
        let d = dir(a);
        [apex[0] + d[0] * reach, apex[1] + d[1] * reach]
    };
    [apex, end(yaw - half), end(yaw + half)]
}

/// The station within 0.75 m and 20° of yaw, if any (the nearest one).
pub fn nearest_station(place: &Place, pos: [f32; 3], yaw: f32) -> Option<String> {
    place
        .stations
        .iter()
        .filter_map(|s| {
            let d = ((s.pos[0] - pos[0]).powi(2) + (s.pos[1] - pos[1]).powi(2) + (s.pos[2] - pos[2]).powi(2)).sqrt();
            (d.is_finite() && d <= 0.75 && angle_diff(s.yaw_deg, yaw) <= 20.).then_some((d, &s.id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id.clone())
}

/// The perspective words for a pitch.
fn perspective(pitch_deg: f32) -> &'static str {
    let p = pitch_deg.abs();
    if p < 5. {
        "verticals stay vertical, one- or two-point perspective"
    } else if p <= 15. {
        "verticals lean slightly, mild three-point perspective"
    } else {
        "three-point perspective, converging verticals"
    }
}

/// Everything derived from a shot's camera and marks.
pub fn derive(shot: &Shot, place: Option<&Place>) -> Derived {
    let c = &shot.camera;
    let cam = cam_pos(c, place);
    let (yaw, pitch) = look_dir(c, &shot.marks, place);
    let lens_mm = lens(c.lens_mm as f32);

    let screen_x = shot.marks.iter().filter_map(|m| screen_x(cam, yaw, lens_mm, m.pos).map(|x| (m.character.clone(), x))).collect();

    let mut sides = BTreeMap::new();
    let mut ordered: Vec<&Mark> = shot.marks.iter().filter(|m| !m.character.is_empty()).collect();
    ordered.sort_by(|a, b| a.character.cmp(&b.character));
    for (i, a) in ordered.iter().enumerate() {
        for b in &ordered[i + 1..] {
            if a.character != b.character {
                sides.insert(format!("{}|{}", a.character, b.character), side_of_line(a.pos, b.pos, [cam[0], cam[2]]));
            }
        }
    }

    let behind = match place {
        Some(p) => shot
            .marks
            .iter()
            .map(|m| (m.character.clone(), behind(cam, yaw, m.pos, &p.plan)))
            .filter(|(_, v)| !v.is_empty())
            .collect(),
        None => BTreeMap::new(),
    };

    let station = match (c.station.as_ref(), place) {
        (Some(id), Some(p)) if p.stations.iter().any(|s| &s.id == id) && c.pos.is_none() => Some(id.clone()),
        (_, Some(p)) => nearest_station(p, cam, yaw),
        (Some(id), None) => Some(id.clone()),
        (None, None) => None,
    };

    Derived {
        hfov_deg: fov_deg(lens_mm).0,
        horizon_y: horizon_y(pitch, lens_mm),
        perspective: perspective(pitch).to_string(),
        screen_x,
        sides,
        behind,
        station,
    }
}

/// What a lens does, in words.
fn lens_words(f: u16) -> String {
    match f {
        0..=18 => format!("ultra-wide {f} mm: camera very close, space strongly stretched"),
        19..=28 => format!("wide {f} mm: camera close, space stretched"),
        29..=40 => format!("{f} mm: natural space with a little depth"),
        41..=65 => format!("normal {f} mm: perspective as the eye sees it"),
        66..=110 => format!("long {f} mm: camera far, background compressed and close behind"),
        _ => format!("telephoto {f} mm: camera very far, background flattened right behind the subject"),
    }
}

/// The angle a camera really has, from where it stands and where it looks: a shot on a station
/// 6 m up looking down is a high angle whatever word the plan carries. A Dutch angle (a roll)
/// stays; a worm's-eye or top-down view stays while the camera is low or looking steeply down.
pub fn real_angle(asked: Angle, height_m: f32, pitch_deg: f32) -> Angle {
    if asked == Angle::Dutch || !height_m.is_finite() || !pitch_deg.is_finite() {
        return asked;
    }
    if pitch_deg <= -60. || (asked == Angle::Top && pitch_deg <= -40.) {
        return Angle::Top;
    }
    if height_m <= 0.6 && (pitch_deg >= 10. || asked == Angle::Worm) {
        return Angle::Worm;
    }
    if pitch_deg <= -8. || height_m >= EYE_HEIGHT_M + 0.6 {
        return Angle::High;
    }
    if pitch_deg >= 8. || height_m <= EYE_HEIGHT_M - 0.5 {
        return Angle::Low;
    }
    Angle::Eye
}

/// The words the prompt uses for a derived camera. The camera height comes from `c.pos` when
/// set, else from its angle (low, worm: below the eyes; high, top: above). [`wording_in`] also
/// knows the camera's station.
pub fn wording(d: &Derived, c: &Camera) -> CameraWords {
    wording_at(d, c, c.pos.map(|p| p[1]))
}

/// [`wording`] with the camera's real height: its own position, else its station's on `place`.
pub fn wording_in(d: &Derived, c: &Camera, place: Option<&Place>) -> CameraWords {
    let height = if c.pos.is_some() || station(c, place).is_some() { Some(cam_pos(c, place)[1]) } else { None };
    wording_at(d, c, height)
}

fn wording_at(d: &Derived, c: &Camera, height: Option<f32>) -> CameraWords {
    let f = if c.lens_mm == 0 { DEFAULT_LENS_MM } else { c.lens_mm };
    let pct = (d.horizon_y * 100.).round();
    let level = match height.filter(|h| h.is_finite()) {
        Some(h) if (h - EYE_HEIGHT_M).abs() <= 0.15 => 0,
        Some(h) if h < EYE_HEIGHT_M => -1,
        Some(_) => 1,
        None => match c.angle {
            Angle::Low | Angle::Worm => -1,
            Angle::High | Angle::Top => 1,
            _ => 0,
        },
    };
    let horizon = match level {
        0 if (0. ..=100.).contains(&pct) => format!("the horizon crosses the characters' eyes at {pct:.0}% from the top"),
        0 if pct < 0. => "eye-level camera, the horizon above the frame".to_string(),
        0 => "eye-level camera, the horizon below the frame".to_string(),
        -1 => "low camera, horizon at their waist".to_string(),
        _ => "camera above eye level, looking down".to_string(),
    };
    let perspective = if d.perspective.is_empty() { perspective(0.).to_string() } else { d.perspective.clone() };
    let roll = if c.roll_deg.is_finite() && c.roll_deg.abs() >= 1. { format!("tilted {:.0}°", c.roll_deg.abs()) } else { String::new() };
    CameraWords { lens: lens_words(f), horizon, perspective, roll }
}
