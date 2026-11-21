//! Camera geometry against hand-computed values (design 6.5): 36 x 20.25 mm frame, x right,
//! z down (south), yaw 0 = north, clockwise.

use mv_anime::geometry::{behind, cam_pos, derive, fov_deg, horizon_y, look_dir, nearest_station, screen_x, side_of_line, view_cone, wording};
use mv_anime::{Angle, Camera, FloorPlan, Landmark, LookAt, Mark, Place, Shot, Station};

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn field_of_view_per_lens() {
    // 2·atan(18/f), 2·atan(10.125/f), worked out by hand.
    for (lens, h, v) in [(14., 104.25, 71.75), (24., 73.7398, 45.7473), (35., 54.4322, 32.2688), (50., 39.5978, 22.8952), (85., 23.9132, 13.5859), (135., 15.1893, 8.5783)] {
        let (fh, fv) = fov_deg(lens);
        assert!(close(fh, h, 0.01) && close(fv, v, 0.01), "{lens} mm: {fh} {fv}");
    }
    // A camera without a lens is a 35 mm.
    assert_eq!(fov_deg(0.), fov_deg(35.));
}

#[test]
fn horizon_line() {
    // Level camera: the middle of the frame, whatever the lens.
    assert!(close(horizon_y(0., 24.), 0.5, 1e-5));
    assert!(close(horizon_y(0., 135.), 0.5, 1e-5));
    // Looking 10° up on a 50 mm: 0.5 + 0.5·tan 10° / (10.125/50) = 0.93538.
    assert!(close(horizon_y(10., 50.), 0.93538, 1e-4));
    // Looking 20° down on a 24 mm: 0.5 + 0.5·tan(−20°) / (10.125/24) = 0.06863.
    assert!(close(horizon_y(-20., 24.), 0.06863, 1e-4));
    // Looking far down: the horizon is above the frame.
    assert!(horizon_y(-60., 50.) < 0.);
}

#[test]
fn side_of_the_line_and_its_sign_flip() {
    let (a, b) = ([0., 0.], [2., 0.]);
    // cross(b−a, cam−a) = 2·2 − 0·1 = 4 for a camera south of the line.
    assert_eq!(side_of_line(a, b, [1., 2.]), 1);
    assert_eq!(side_of_line(a, b, [1., -2.]), -1);
    // Swapping the two characters flips it.
    assert_eq!(side_of_line(b, a, [1., 2.]), -1);
    assert_eq!(side_of_line(b, a, [1., -2.]), 1);
    // On the line.
    assert_eq!(side_of_line(a, b, [5., 0.]), 0);
}

#[test]
fn screen_positions() {
    // Camera 5 m south of the origin, looking north on a 50 mm: x = 0.5 + 0.5·(lateral/depth)·50/18.
    let cam = [0., 1.6, 5.];
    assert!(close(screen_x(cam, 0., 50., [0., 0.]).unwrap_or(-1.), 0.5, 1e-5));
    assert!(close(screen_x(cam, 0., 50., [1., 0.]).unwrap_or(-1.), 0.777_78, 1e-4));
    assert!(close(screen_x(cam, 0., 50., [-1., 0.]).unwrap_or(-1.), 0.222_22, 1e-4));
    // Just outside the frame but within the margin; beyond it; behind the camera.
    assert!(close(screen_x(cam, 0., 50., [2.1, 0.]).unwrap_or(-1.), 1.083_33, 1e-4));
    assert_eq!(screen_x(cam, 0., 50., [3., 0.]), None);
    assert_eq!(screen_x(cam, 0., 50., [0., 6.]), None);
    // On an 18 mm the same point sits at 0.6.
    assert!(close(screen_x(cam, 0., 18., [1., 0.]).unwrap_or(-1.), 0.6, 1e-4));
    // Looking east: south (+z) is to the right.
    assert!(close(screen_x([0., 1.6, 0.], 90., 50., [5., 1.]).unwrap_or(-1.), 0.777_78, 1e-4));
}

#[test]
fn view_cone_for_the_map() {
    // A 24 mm sees ±36.87° (tan = 18/24 = 0.75): edges end at (∓6, −8) after 10 m.
    let [apex, left, right] = view_cone([0., 1.6, 0.], 0., 24., 10.);
    assert_eq!(apex, [0., 0.]);
    assert!(close(left[0], -6., 1e-3) && close(left[1], -8., 1e-3), "{left:?}");
    assert!(close(right[0], 6., 1e-3) && close(right[1], -8., 1e-3), "{right:?}");
    // Facing east the cone turns with the camera.
    let [_, l, r] = view_cone([1., 1.6, 1.], 90., 24., 10.);
    assert!(close(l[0], 9., 1e-3) && close(l[1], -5., 1e-3), "{l:?}");
    assert!(close(r[0], 9., 1e-3) && close(r[1], 7., 1e-3), "{r:?}");
}

fn place() -> Place {
    Place {
        id: "pl_room".into(),
        plan: FloorPlan {
            w_m: 12.,
            d_m: 12.,
            landmarks: vec![
                Landmark { id: "window".into(), pos: [0., -6.], size: [2., 0.2], h_m: 2., ..Landmark::default() },
                Landmark { id: "door".into(), pos: [5., -6.], size: [1., 0.2], h_m: 2., ..Landmark::default() },
                Landmark { id: "table".into(), pos: [0., 2.], size: [1., 1.], h_m: 0.8, ..Landmark::default() },
            ],
            ..FloorPlan::default()
        },
        stations: vec![
            Station { id: "st_south".into(), pos: [0.5, 1.6, 5.], yaw_deg: 10., lens_mm: 35, ..Station::default() },
            Station { id: "st_north".into(), pos: [0., 1.6, -5.], yaw_deg: 180., pitch_deg: -10., lens_mm: 24, ..Station::default() },
        ],
        ..Place::default()
    }
}

#[test]
fn what_stands_behind_a_character() {
    let p = place();
    // From the south, the window is right behind a character at the centre; the table is in front.
    assert_eq!(behind([0., 1.6, 4.], 0., [0., 0.], &p.plan), vec!["window".to_string()]);
    assert!(behind([0., 1.6, 4.], 0., [0., 0.], &p.plan).iter().all(|id| id != "door" && id != "table"));
    // A mark behind the camera hides nothing.
    assert!(behind([0., 1.6, 4.], 0., [0., 5.], &p.plan).is_empty());
}

#[test]
fn stations_and_camera_position() {
    let p = place();
    assert_eq!(nearest_station(&p, [0., 1.6, 5.], 0.), Some("st_south".into()));
    assert_eq!(nearest_station(&p, [0., 1.6, 5.], 40.), None);
    assert_eq!(nearest_station(&p, [0., 1.6, 3.], 10.), None);
    let at_station = Camera { station: Some("st_north".into()), ..Camera::default() };
    assert_eq!(cam_pos(&at_station, Some(&p)), [0., 1.6, -5.]);
    let own = Camera { station: Some("st_north".into()), pos: Some([1., 1., 1.]), ..Camera::default() };
    assert_eq!(cam_pos(&own, Some(&p)), [1., 1., 1.]);
    assert_eq!(cam_pos(&Camera::default(), None), [0., 1.6, 0.]);
}

#[test]
fn looking_at_a_character_or_a_landmark() {
    let marks = [Mark { character: "a".into(), pos: [0., 0.], ..Mark::default() }, Mark { character: "b".into(), pos: [5., 5.], ..Mark::default() }];
    let cam = |look_at| Camera { pos: Some([0., 1.6, 5.]), look_at, ..Camera::default() };
    let (yaw, pitch) = look_dir(&cam(LookAt::Character("a".into())), &marks, None);
    assert!(close(yaw, 0., 1e-3) && close(pitch, 0., 1e-3), "{yaw} {pitch}");
    let (yaw, _) = look_dir(&cam(LookAt::Character("b".into())), &marks, None);
    assert!(close(yaw, 90., 1e-3), "{yaw}");
    // A point 5 m due west and 5 m lower: yaw 270, 45° down.
    let (yaw, pitch) = look_dir(&cam(LookAt::Point([-5., -3.4, 5.])), &marks, None);
    assert!(close(yaw, 270., 1e-3) && close(pitch, -45., 1e-3), "{yaw} {pitch}");
    let p = place();
    let (yaw, _) = look_dir(&cam(LookAt::Landmark("window".into())), &marks, Some(&p));
    assert!(close(yaw, 0., 1e-3), "{yaw}");
    // An unknown target falls back to the station's own angles.
    let st = Camera { station: Some("st_north".into()), look_at: LookAt::Character("nobody".into()), ..Camera::default() };
    assert_eq!(look_dir(&st, &marks, Some(&p)), (180., -10.));
}

fn two_shot(cam: [f32; 3], yaw: f32) -> Shot {
    Shot {
        camera: Camera { pos: Some(cam), look_at: LookAt::Angles { yaw_deg: yaw, pitch_deg: 0. }, lens_mm: 50, ..Camera::default() },
        marks: vec![Mark { character: "a".into(), pos: [-1., 0.], ..Mark::default() }, Mark { character: "b".into(), pos: [1., 0.], ..Mark::default() }],
        ..Shot::default()
    }
}

#[test]
fn derived_camera_and_the_axis_flip() {
    let p = place();
    let d = derive(&two_shot([0., 1.6, 5.], 0.), Some(&p));
    assert!(close(d.hfov_deg, 39.5978, 0.01));
    assert!(close(d.horizon_y, 0.5, 1e-5));
    assert!(close(d.screen_x["a"], 0.222_22, 1e-4) && close(d.screen_x["b"], 0.777_78, 1e-4));
    assert_eq!(d.sides["a|b"], 1);
    assert_eq!(d.station.as_deref(), Some("st_south"));
    assert_eq!(d.perspective, "verticals stay vertical, one- or two-point perspective");
    // From the other side of the a–b line the side flips and the characters swap sides on screen.
    let r = derive(&two_shot([0., 1.6, -5.], 180.), Some(&p));
    assert_eq!(r.sides["a|b"], -1);
    assert!(r.screen_x["a"] > 0.5 && r.screen_x["b"] < 0.5);
    assert_eq!(r.station.as_deref(), Some("st_north"));
}

#[test]
fn words_for_the_prompt() {
    let mut shot = two_shot([0., 1.6, 5.], 0.);
    shot.camera.lens_mm = 24;
    shot.camera.roll_deg = -15.;
    let d = derive(&shot, None);
    let w = wording(&d, &shot.camera);
    assert_eq!(w.lens, "wide 24 mm: camera close, space stretched");
    assert_eq!(w.horizon, "the horizon crosses the characters' eyes at 50% from the top");
    assert_eq!(w.perspective, "verticals stay vertical, one- or two-point perspective");
    assert_eq!(w.roll, "tilted 15°");
    shot.camera.lens_mm = 85;
    shot.camera.roll_deg = 0.;
    shot.camera.pos = Some([0., 0.5, 5.]);
    let w = wording(&derive(&shot, None), &shot.camera);
    assert_eq!(w.lens, "long 85 mm: camera far, background compressed and close behind");
    assert_eq!(w.horizon, "low camera, horizon at their waist");
    assert_eq!(w.roll, "");
    // Steep pitch: three-point perspective; no position, a high angle reads as looking down.
    shot.camera.pos = None;
    shot.camera.angle = Angle::High;
    shot.camera.look_at = LookAt::Angles { yaw_deg: 0., pitch_deg: -30. };
    let d = derive(&shot, None);
    let w = wording(&d, &shot.camera);
    assert_eq!(w.perspective, "three-point perspective, converging verticals");
    assert_eq!(w.horizon, "camera above eye level, looking down");
}

#[test]
fn a_station_camera_is_described_from_where_it_really_stands() {
    use mv_anime::geometry::{real_angle, wording_in};
    let mut place = Place { id: "pl".into(), ..Default::default() };
    place.stations = vec![Station { id: "st_high".into(), pos: [0., 6., 5.], yaw_deg: 0., pitch_deg: -30., lens_mm: 24, ..Default::default() }];
    let mut shot = two_shot([0., 1.6, 5.], 0.);
    shot.camera.pos = None;
    shot.camera.station = Some("st_high".into());
    shot.camera.angle = Angle::Eye;
    let d = derive(&shot, Some(&place));
    // The angle word says "eye", the station is 6 m up: the words follow the station.
    assert_eq!(wording_in(&d, &shot.camera, Some(&place)).horizon, "camera above eye level, looking down");
    assert_eq!(real_angle(Angle::Eye, 6., -30.), Angle::High);
    assert_eq!(real_angle(Angle::High, 1.6, 0.), Angle::Eye);
    assert_eq!(real_angle(Angle::Eye, 0.4, 20.), Angle::Worm);
    assert_eq!(real_angle(Angle::Eye, 1.0, 12.), Angle::Low);
    assert_eq!(real_angle(Angle::Dutch, 6., -30.), Angle::Dutch, "a roll stays a roll");
    assert_eq!(real_angle(Angle::Eye, 12., -90.), Angle::Top);
}
