//! Bible pictures (design 6.2): the generation specs (prompt + references) of character and
//! prop sheets, place masters, plates and the style frame, and the job that draws them.
//!
//! Every prompt follows the engine's fixed order (design 6.6): the style line word for word,
//! what each picture is for ("Image N is …: use it only for …", numbered in upload order), the
//! camera or sheet layout, the subject, props, colour and light, the keep-list, what to avoid.
//! References carry a stated job each, at most [`MAX_IDENTITY`] identity pictures and at most
//! [`SOFT_REFS`] in all (the site takes [`MAX_REFS`]; fewer, cleaner references work better).
//!
//! Places are described as measured sets: every plate names its camera station (position,
//! height, compass direction, lens), what is visible left to right from there and where the key
//! light comes from on screen, so one background painted per station can be reused by every
//! shot from that station.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use crate::bible::{parse_hex, DEFAULT_STYLE_LINE, EXTRA_FORCES};
use crate::crops::usable;
use crate::geometry;
use crate::hash::request_hash;
use crate::model::{
    BibleIndex, Camera, Card, Character, CropFraming, FloorPlan, GenRequest, GenSpec, LookAt, Place, PlaceKind, PlaceState, ProjectSettings, Prop, RefCrop, RefRole, RefSlot, Role,
    Sheet, SheetKind, Shot, SiteSlot, Station, Status, Style, Target, TakeKind, TakeMeta, TakeSource, ViewAngle,
};
use crate::queue::JobCtx;
use crate::store::Store;

/// Identity pictures per request (the model keeps likeness for up to 4 characters).
pub const MAX_IDENTITY: usize = 4;
/// References the site takes per request.
pub const MAX_REFS: usize = 14;
/// References a bible sheet sends at most ("fewer, cleaner").
pub const SOFT_REFS: usize = 8;
/// Pictures "Design options" makes on a character's master sheet.
pub const MASTER_OPTIONS: u8 = 4;

/// Whose sheet it is.
#[derive(Clone, Copy, Debug)]
pub enum Subject<'a> {
    Character(&'a Character),
    Place(&'a Place),
    Prop(&'a Prop),
}

/// `Target::Sheet { sheet }` key of a sheet kind: "master", "face", "turnaround",
/// "expressions", "outfit:<outfit id>", "grip:<prop id>", "colour_model", "back".
pub fn sheet_key(kind: &SheetKind) -> String {
    match kind {
        SheetKind::Master => "master".into(),
        SheetKind::Face => "face".into(),
        SheetKind::Turnaround => "turnaround".into(),
        SheetKind::Expressions => "expressions".into(),
        SheetKind::Outfit(id) => format!("outfit:{id}"),
        SheetKind::Grip(id) => format!("grip:{id}"),
        SheetKind::ColourModel => "colour_model".into(),
        SheetKind::Back => "back".into(),
        SheetKind::Unknown => "unknown".into(),
    }
}

/// The sheet kind of a [`sheet_key`]; `None` for a key this build does not know.
pub fn sheet_kind(key: &str) -> Option<SheetKind> {
    let key = key.trim();
    let (head, id) = match key.split_once(':') {
        Some((h, i)) => (h, Some(i.trim().to_string()).filter(|i| !i.is_empty())),
        None => (key, None),
    };
    Some(match (head.to_lowercase().as_str(), id) {
        ("master", None) => SheetKind::Master,
        ("face", None) => SheetKind::Face,
        ("turnaround", None) => SheetKind::Turnaround,
        ("expressions", None) => SheetKind::Expressions,
        ("outfit", Some(id)) => SheetKind::Outfit(id),
        ("grip", Some(id)) => SheetKind::Grip(id),
        ("colour_model" | "color_model", None) => SheetKind::ColourModel,
        ("back", None) => SheetKind::Back,
        _ => return None,
    })
}

/// The sheets of a character's pack, in the order they are made: master, face, turnaround,
/// expressions, back, colour model, one per outfit beyond the first, one grip per held prop.
pub fn character_pack(c: &Character, bible: &BibleIndex) -> Vec<SheetKind> {
    let mut out = vec![SheetKind::Master, SheetKind::Face, SheetKind::Turnaround, SheetKind::Expressions, SheetKind::Back, SheetKind::ColourModel];
    out.extend(c.outfits.iter().skip(1).map(|o| SheetKind::Outfit(o.id.clone())));
    out.extend(bible.props.iter().filter(|p| p.held && p.owner.as_deref() == Some(c.id.as_str())).map(|p| SheetKind::Grip(p.id.clone())));
    out
}

// ---------------------------------------------------------------- references

/// A reference and how the prompt names it.
struct Pic {
    slot: RefSlot,
    /// "a manga panel of Kai".
    what: String,
}

fn is_identity(r: &RefRole) -> bool {
    matches!(r, RefRole::Identity(_))
}

/// A take as a reference (`None` when its file is gone).
fn take_pic(store: &Store, take: &str, role: RefRole, slot: SiteSlot, what: String, job: String) -> Option<Pic> {
    let abs = store.take_file(take)?;
    let file = store.rel(&abs)?;
    Some(Pic { slot: RefSlot { file, hash: take.to_string(), role, job, slot, pinned: false }, what })
}

/// A card's selected picture; `approved_only` asks for an Approved card.
fn card_pic(store: &Store, card: &Card, approved_only: bool, role: RefRole, slot: SiteSlot, what: String, job: String) -> Option<Pic> {
    if card.status == Status::Rejected || (approved_only && card.status != Status::Approved) {
        return None;
    }
    take_pic(store, card.selected.as_deref()?, role, slot, what, job)
}

fn crop_pic(store: &Store, crop: &RefCrop, role: RefRole, slot: SiteSlot, what: String, job: String) -> Option<Pic> {
    take_pic(store, &crop.take, role, slot, what, job)
}

/// Usable crops, best first, preferring `framings` (in that order) and then the rest.
fn best_crops<'a>(crops: &'a [RefCrop], framings: &[CropFraming]) -> Vec<&'a RefCrop> {
    let mut v: Vec<&RefCrop> = crops.iter().filter(|c| usable(c)).collect();
    let rank = |c: &RefCrop| framings.iter().position(|f| *f == c.framing).unwrap_or(framings.len());
    v.sort_by(|a, b| u8::from(b.approved).cmp(&u8::from(a.approved)).then(rank(a).cmp(&rank(b))).then(b.score.total_cmp(&a.score)));
    v
}

/// The style frame as a reference, when one is picked.
fn style_pic(store: &Store, style: &Style) -> Option<Pic> {
    card_pic(
        store,
        &style.frame,
        false,
        RefRole::Style,
        SiteSlot::Style,
        "the style frame".into(),
        "its line art, shading, colour and painting style, not its people, place or composition".into(),
    )
}

fn sheet_card<'a>(sheets: &'a [Sheet], kind: &SheetKind) -> Option<&'a Card> {
    sheets.iter().find(|s| &s.kind == kind).map(|s| &s.card)
}

/// At most [`MAX_IDENTITY`] identity pictures and [`SOFT_REFS`] in all, each picture once.
fn limit(pics: Vec<Pic>) -> Vec<Pic> {
    let (mut out, mut ids, mut seen) = (Vec::new(), 0usize, BTreeSet::new());
    for p in pics {
        if out.len() >= SOFT_REFS.min(MAX_REFS) {
            break;
        }
        if !seen.insert(p.slot.hash.clone()) {
            continue;
        }
        if is_identity(&p.slot.role) {
            if ids >= MAX_IDENTITY {
                continue;
            }
            ids += 1;
        }
        out.push(p);
    }
    out
}

// ---------------------------------------------------------------- words

/// "vivid red", "dark blue", "light grey" for a colour.
pub(crate) fn colour_name(c: [u8; 3]) -> String {
    let (r, g, b) = (c[0] as f32 / 255., c[1] as f32 / 255., c[2] as f32 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    let d = max - min;
    let s = if d == 0. { 0. } else { d / (1. - (2. * l - 1.).abs()).max(1e-3) };
    if s < 0.12 || d < 0.06 {
        return match l {
            l if l < 0.15 => "black",
            l if l < 0.4 => "dark grey",
            l if l > 0.88 => "white",
            l if l > 0.65 => "light grey",
            _ => "grey",
        }
        .into();
    }
    let h = if max == r {
        60. * (((g - b) / d).rem_euclid(6.))
    } else if max == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    let hue = match h {
        h if h < 15. => "red",
        h if h < 40. => if l < 0.4 { "brown" } else { "orange" },
        h if h < 65. => "yellow",
        h if h < 90. => "lime green",
        h if h < 150. => "green",
        h if h < 185. => "teal",
        h if h < 205. => "cyan",
        h if h < 250. => "blue",
        h if h < 275. => "indigo",
        h if h < 300. => "violet",
        h if h < 330. => "magenta",
        h if h < 345. => "pink",
        _ => "red",
    };
    let shade = if l < 0.3 {
        "dark "
    } else if l > 0.72 {
        "light "
    } else if s > 0.65 {
        "vivid "
    } else {
        ""
    };
    format!("{shade}{hue}")
}

/// 8-point compass word of a yaw (0 north, clockwise).
fn compass(deg: f32) -> &'static str {
    const N: [&str; 8] = ["north", "north-east", "east", "south-east", "south", "south-west", "west", "north-west"];
    let d = if deg.is_finite() { deg.rem_euclid(360.) } else { 0. };
    N[(((d + 22.5) / 45.).floor() as usize) % 8]
}

/// Where a point lies on the plan, in words ("against the north wall, west of centre").
fn plan_words(pos: [f32; 2], plan: &FloorPlan) -> String {
    let (hw, hd) = (plan.w_m.max(0.1) / 2., plan.d_m.max(0.1) / 2.);
    let ew = if pos[0] < -hw / 3. {
        Some("west")
    } else if pos[0] > hw / 3. {
        Some("east")
    } else {
        None
    };
    let ns = if pos[1] < -hd / 3. {
        Some("north")
    } else if pos[1] > hd / 3. {
        Some("south")
    } else {
        None
    };
    let at_x = pos[0].abs() >= hw - 0.6;
    let at_z = pos[1].abs() >= hd - 0.6;
    match (ns, ew) {
        (Some(n), Some(e)) if at_x && at_z => format!("in the {n}-{e} corner"),
        (Some(n), e) if at_z => format!("against the {n} wall{}", e.map(|e| format!(", toward the {e}")).unwrap_or_default()),
        (n, Some(e)) if at_x => format!("against the {e} wall{}", n.map(|n| format!(", toward the {n}")).unwrap_or_default()),
        (Some(n), Some(e)) => format!("in the {n}-{e} part"),
        (Some(n), None) => format!("in the {n} part, centred"),
        (None, Some(e)) => format!("in the {e} part, centred"),
        (None, None) => "in the middle".into(),
    }
}

fn kelvin_words(k: u32) -> &'static str {
    match k {
        0..=2599 => "candle-warm orange",
        2600..=3599 => "warm golden",
        3600..=4799 => "soft warm",
        4800..=6099 => "neutral white",
        6100..=6999 => "cool overcast",
        _ => "cold blue",
    }
}

/// The light of a state; with a camera yaw, also where it falls on screen.
fn light_words(state: &PlaceState, cam_yaw: Option<f32>) -> String {
    let mut out = format!(
        "Light: {} ({}). The key light comes from the {} ({} K, {} light)",
        state.name.trim(),
        state.mood.trim(),
        compass(state.key_dir_deg),
        state.kelvin,
        kelvin_words(state.kelvin)
    );
    if let Some(yaw) = cam_yaw {
        let rel = (state.key_dir_deg - yaw).rem_euclid(360.);
        let side = match rel {
            r if !(45. ..315.).contains(&r) => ": it shines toward the camera from behind the scene, so edges glow and shadows fall toward us",
            r if r < 135. => ": it falls from screen right, shadows fall to the left",
            r if r < 225. => ": it comes from behind the camera, fronts are lit and shadows fall away from us",
            _ => ": it falls from screen left, shadows fall to the right",
        };
        out.push_str(side);
    }
    out.push_str(". A soft gradient runs from the key-light side; every light source glows.");
    out
}

/// Wind and moving elements that are the same in every shot of a place.
fn forces_words(p: &Place, cam_yaw: Option<f32>) -> String {
    let Some(f) = p.extra.get(EXTRA_FORCES) else { return String::new() };
    let mut parts = Vec::new();
    let wind = f.get("wind").and_then(Value::as_str).unwrap_or("").trim();
    if let Some(deg) = f.get("wind_deg").and_then(Value::as_f64).map(|d| d as f32) {
        let mut s = format!("Wind from the {}", compass(deg));
        if !wind.is_empty() {
            s.push_str(&format!(" ({wind})"));
        }
        if let Some(yaw) = cam_yaw {
            // The wind blows toward deg + 180; on screen right when that is east of the view.
            let to = (deg + 180. - yaw).rem_euclid(360.);
            if (20. ..160.).contains(&to) {
                s.push_str(": smoke, hair and cloth drift toward screen right");
            } else if (200. ..340.).contains(&to) {
                s.push_str(": smoke, hair and cloth drift toward screen left");
            }
        }
        parts.push(s);
    } else if !wind.is_empty() {
        parts.push(format!("Wind: {wind}"));
    }
    let moving: Vec<&str> = f.get("moving").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    if !moving.is_empty() {
        parts.push(format!("Always present: {}", moving.join(", ")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("{}.", parts.join(". "))
    }
}

fn sentence(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() || t.ends_with(['.', '!', '?']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

/// The prompt in the fixed order; empty sections are left out.
#[derive(Default)]
struct Prompt {
    style: String,
    pictures: String,
    camera: String,
    action: String,
    props: String,
    colour: String,
    keep: String,
    avoid: String,
}

impl Prompt {
    fn text(&self) -> String {
        [&self.style, &self.pictures, &self.camera, &self.action, &self.props, &self.colour, &self.keep, &self.avoid]
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// The style section: the style line word for word, then the style frame's number.
fn style_section(style: &Style, pics: &[Pic]) -> String {
    let line = if style.line.trim().is_empty() { DEFAULT_STYLE_LINE } else { style.line.trim() };
    match pics.iter().position(|p| p.slot.role == RefRole::Style) {
        Some(i) => format!("{} Match the exact style of Image {}.", sentence(line), i + 1),
        None => sentence(line),
    }
}

/// "Image N is …: use it only for …", one sentence per picture in upload order; a run of pictures
/// with the same description and job is one sentence ("Images 1, 2 and 3 are each …").
fn pictures_section(pics: &[Pic]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pics.len() {
        let mut j = i + 1;
        while j < pics.len() && pics[j].what == pics[i].what && pics[j].slot.job == pics[i].slot.job {
            j += 1;
        }
        let p = &pics[i];
        if j - i == 1 {
            out.push(format!("Image {} is {}: use it only for {}.", i + 1, p.what, p.slot.job));
        } else {
            let nums: Vec<String> = (i + 1..=j).map(|n| n.to_string()).collect();
            let (last, rest) = nums.split_last().map(|(l, r)| (l.clone(), r.join(", "))).unwrap_or_default();
            out.push(format!("Images {rest} and {last} are each {}: use them only for {}.", p.what, p.slot.job));
        }
        i = j;
    }
    out.join(" ")
}

fn avoid_section(style: &Style, extra: &str) -> String {
    let mut out = String::from("No text, no letters, no labels, no captions, no speech bubbles, no watermark, no signature.");
    if !extra.is_empty() {
        out.push(' ');
        out.push_str(extra);
    }
    let base = ["text", "speech bubbles", "watermark", "letters", "labels", "captions"];
    let more: Vec<&str> = style.negative.iter().map(|s| s.trim()).filter(|s| !s.is_empty() && !base.iter().any(|b| b.eq_ignore_ascii_case(s))).collect();
    if !more.is_empty() {
        out.push_str(&format!(" Also avoid: {}.", more.join(", ")));
    }
    out
}

// ---------------------------------------------------------------- characters

fn name_of(c: &Character) -> String {
    if c.name.trim().is_empty() {
        c.id.clone()
    } else {
        c.name.trim().to_string()
    }
}

fn look_text(c: &Character) -> String {
    let n = name_of(c);
    let mut s = n.clone();
    if !c.age.trim().is_empty() {
        s.push_str(&format!(", {}", c.age.trim()));
    }
    if c.look.height_m > 0. {
        s.push_str(&format!(", {:.2} m tall", c.look.height_m));
    }
    s.push('.');
    for (label, v) in [("Build", &c.look.build), ("Hair", &c.look.hair), ("Eyes", &c.look.eyes)] {
        if !v.trim().is_empty() {
            s.push_str(&format!(" {label}: {}", sentence(v)));
        }
    }
    s
}

fn outfit_text(c: &Character) -> String {
    if c.look.default_outfit.trim().is_empty() {
        String::new()
    } else {
        format!("Outfit: {}", sentence(&c.look.default_outfit))
    }
}

fn colour_text(c: &Character) -> String {
    match parse_hex(&c.look.signature_colour) {
        Some(rgb) => format!(
            "{}'s signature colour is {} ({}): it is the main colour of the most visible part of the outfit or hair, so {} is recognised by colour even small in a wide shot.",
            name_of(c),
            colour_name(rgb),
            c.look.signature_colour.trim().to_uppercase(),
            name_of(c)
        ),
        None => String::new(),
    }
}

fn keep_text(c: &Character) -> String {
    let keep: Vec<&str> = c.look.keep.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if keep.is_empty() {
        String::new()
    } else {
        format!("Keep exactly, on the same side: {}'s {}.", name_of(c), keep.join("; "))
    }
}

/// The approved master design as the identity root.
fn master_pic(store: &Store, c: &Character) -> Option<Pic> {
    let card = sheet_card(&c.sheets, &SheetKind::Master)?;
    card_pic(
        store,
        card,
        true,
        RefRole::Identity(c.id.clone()),
        SiteSlot::Character,
        format!("the approved character design of {}", name_of(c)),
        format!("{}'s identity: face, eyes, hair, body proportions, outfit and colours", name_of(c)),
    )
}

fn identity_crop(store: &Store, c: &Character, crop: &RefCrop) -> Option<Pic> {
    crop_pic(
        store,
        crop,
        RefRole::Identity(c.id.clone()),
        SiteSlot::Character,
        format!("a manga panel of {}", name_of(c)),
        format!("{}'s face shape, eyes, hairstyle and outfit as drawn, not its drawing style", name_of(c)),
    )
}

fn view_crop(store: &Store, c: &Character, crop: &RefCrop) -> Option<Pic> {
    let view = match crop.angle {
        ViewAngle::Back => "back",
        ViewAngle::Side => "side",
        ViewAngle::ThreeQuarter => "three-quarter",
        _ => "front",
    };
    crop_pic(
        store,
        crop,
        RefRole::Panel,
        SiteSlot::Add,
        format!("another manga panel of {} ({view} view)", name_of(c)),
        format!("what {} looks like from this side: hair, outfit details and accessories", name_of(c)),
    )
}

fn character_spec(kind: &SheetKind, c: &Character, style: &Style, store: &Store, bible: &BibleIndex, settings: &ProjectSettings) -> GenSpec {
    let name = name_of(c);
    let master = master_pic(store, c);
    let has_master = master.is_some();
    let mut pics: Vec<Pic> = Vec::new();
    let face_first = [CropFraming::Face, CropFraming::Bust, CropFraming::Full];
    let body_first = [CropFraming::Full, CropFraming::Bust, CropFraming::Face];
    let crops_face = best_crops(&c.crops, &face_first);
    let crops_body = best_crops(&c.crops, &body_first);
    let extra_views = |pics: &mut Vec<Pic>, angles: &[ViewAngle], n: usize| {
        let mut added = 0;
        for crop in crops_body.iter().filter(|x| angles.contains(&x.angle)) {
            if added >= n {
                break;
            }
            if let Some(p) = view_crop(store, c, crop) {
                pics.push(p);
                added += 1;
            }
        }
    };
    let style_ref = style_pic(store, style);

    match kind {
        SheetKind::Master => {
            // The manga crops are the identity (up to 4), two more views as details.
            for crop in crops_face.iter().take(MAX_IDENTITY) {
                pics.extend(identity_crop(store, c, crop));
            }
            pics.extend(style_ref);
            let used: BTreeSet<String> = pics.iter().map(|p| p.slot.hash.clone()).collect();
            let mut extra = 0;
            for crop in crops_body.iter().filter(|x| !used.contains(&x.take)) {
                if extra >= 2 {
                    break;
                }
                if let Some(p) = view_crop(store, c, crop) {
                    pics.push(p);
                    extra += 1;
                }
            }
        }
        _ => {
            match master {
                Some(m) => pics.push(m),
                None => {
                    for crop in crops_face.iter().take(2) {
                        pics.extend(identity_crop(store, c, crop));
                    }
                }
            }
            // A character's sheets build on the master design only, never on each other: approving
            // one sheet must never change the request of a sibling (approved earlier or later), so
            // no approved sheet turns "outdated" because another one was approved. Extra views come
            // from the manga panels instead.
            match kind {
                SheetKind::Face => {
                    let mut added = 0;
                    for crop in crops_face.iter().filter(|x| matches!(x.angle, ViewAngle::ThreeQuarter | ViewAngle::Side)) {
                        if added >= 1 {
                            break;
                        }
                        if let Some(p) = view_crop(store, c, crop) {
                            pics.push(p);
                            added += 1;
                        }
                    }
                }
                SheetKind::Turnaround => extra_views(&mut pics, &[ViewAngle::Side, ViewAngle::Back], 2),
                SheetKind::Back => extra_views(&mut pics, &[ViewAngle::Back], 2),
                SheetKind::Grip(prop_id) => {
                    if let Some(prop) = bible.prop(prop_id) {
                        let prop_sheet = sheet_card(&prop.sheets, &SheetKind::Master).and_then(|card| {
                            card_pic(
                                store,
                                card,
                                true,
                                RefRole::Prop(prop.id.clone()),
                                SiteSlot::Add,
                                format!("the design sheet of the {}", prop.name.trim()),
                                format!("the {}'s exact shape, size and colours", prop.name.trim()),
                            )
                        });
                        match prop_sheet {
                            Some(p) => pics.push(p),
                            None => {
                                for crop in best_crops(&prop.crops, &[CropFraming::Full]).into_iter().take(2) {
                                    pics.extend(crop_pic(
                                        store,
                                        crop,
                                        RefRole::Prop(prop.id.clone()),
                                        SiteSlot::Add,
                                        format!("a manga panel showing the {}", prop.name.trim()),
                                        format!("the {}'s shape and proportions, not the drawing style", prop.name.trim()),
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            pics.extend(style_ref);
        }
    }
    let pics = limit(pics);

    let from = |what: &str| if pics.iter().any(|p| is_identity(&p.slot.role)) { format!(" {what}") } else { String::new() };
    let mut pr = Prompt { style: style_section(style, &pics), pictures: pictures_section(&pics), ..Default::default() };
    let look = look_text(c);
    let outfit = outfit_text(c);
    let (aspect, variants): (&str, u8) = match kind {
        SheetKind::Master => {
            pr.camera = "Full-body character design: one character standing in a relaxed neutral A-pose, front view, the whole figure from the top of the head to the feet with a little space around it, plain white background, flat even studio lighting. One character only.".into();
            let redraw = if pics.iter().any(|p| is_identity(&p.slot.role)) {
                format!(" Redraw {name} from the manga panels as a finished TV-anime character design: keep the face shape, eye shape, hair silhouette and outfit exactly as drawn in the manga; line weight, shading and colour follow the style above.")
            } else {
                format!(" Design {name} from this description as a finished TV-anime character design.")
            };
            pr.action = format!("{look} {outfit}{redraw} {}", sentence(&c.description));
            // The panel's count follows the project's "Pictures per Generate"; the Bible's own
            // "Design options" button asks for MASTER_OPTIONS designs explicitly.
            ("3:4", 0)
        }
        SheetKind::Face => {
            pr.camera = "Head-and-shoulders model sheet: two views side by side at the same scale, front view on the left, three-quarter view on the right, neutral expression, mouth closed, eyes open, plain white background, flat even lighting, crisp line art. Exactly one face per view and no other people.".into();
            pr.action = format!("{look}{}", from(&format!("Same face, hair and accessories as {name} in the reference, nothing redesigned.")));
            ("16:9", 0)
        }
        SheetKind::Turnaround => {
            pr.camera = "Character turnaround model sheet: four full-body views side by side at identical scale and height, left to right: front, three-quarter front, side profile facing left, back. Neutral A-pose, arms slightly away from the body, feet on one baseline, plain white background, flat even lighting, thin horizontal guide lines at the top of the head, the chin, the waist and the feet. This sheet is about the body and the outfit: keep the faces small and simple.".into();
            pr.action = format!("{look} {outfit}{}", from(&format!("Same outfit and colours as {name} in the reference.")));
            ("21:9", 0)
        }
        SheetKind::Expressions => {
            pr.camera = "Expression sheet: a 4 × 2 grid of head-and-shoulders drawings, all in three-quarter view at the same scale, same hairstyle and accessories in every cell, plain white background. Top row: neutral, joy, anger, sadness. Bottom row: fear, surprise, resolve, despair. Each cell holds one face; no labels or words anywhere.".into();
            pr.action = format!("{look} Each emotion reads clearly in the eyes, brows and mouth while the face stays {name}'s.");
            ("16:9", 0)
        }
        SheetKind::Outfit(id) => {
            let o = c.outfits.iter().find(|o| &o.id == id);
            let desc = o.map(|o| format!("{}: {}", o.name.trim(), o.description.trim())).unwrap_or_else(|| id.clone());
            pr.camera = "Outfit turnaround model sheet: four full-body views side by side at identical scale: front, three-quarter front, side profile facing left, back. Neutral A-pose, plain white background, flat even lighting.".into();
            let instead = if c.look.default_outfit.trim().is_empty() { String::new() } else { format!(" instead of {}", c.look.default_outfit.trim()) };
            pr.action = format!("{look} {name} wears {}{instead}. Face, hair and body stay exactly the same.", desc.trim_end_matches('.'));
            ("21:9", 0)
        }
        SheetKind::Grip(prop_id) => {
            let prop = bible.prop(prop_id);
            let pname = prop.map(|p| p.name.trim().to_string()).unwrap_or_else(|| prop_id.clone());
            pr.camera = "Grip sheet: three poses side by side at the same scale, full body, plain white background, flat even lighting.".into();
            pr.action = format!(
                "{look} {outfit} {name} holds the {pname}: left, held ready in the right hand; centre, held in both hands; right, carried at rest. Fingers wrap fully around the grip, the contact points are clear."
            );
            pr.props = prop
                .map(|p| format!("The {pname}: {} Its size against {name}'s body stays exactly as in its design sheet.", sentence(&p.description)))
                .unwrap_or_default();
            ("16:9", 0)
        }
        SheetKind::ColourModel => {
            pr.camera = "Colour model sheet: one front view, full body, flat colours with a single hard shadow tone, and a column of plain colour swatches on the right, one per colour region (skin, skin shadow, hair, hair shadow, eyes, each piece of the outfit). Plain white background. Swatches only, no text.".into();
            pr.action = format!("{look} {outfit}");
            ("16:9", 0)
        }
        SheetKind::Back => {
            pr.camera = "Back view: one full-body figure seen exactly from behind, standing, plain white background, flat even lighting.".into();
            pr.action = format!("{look} {outfit} Show everything that is seen only from behind: the hair from the back, the back of the outfit, bags, capes, weapons on the back.");
            ("3:4", 0)
        }
        SheetKind::Unknown => {
            pr.camera = "Character model sheet on a plain white background, flat even lighting.".into();
            pr.action = format!("{look} {outfit}");
            ("16:9", 0)
        }
    };
    if !has_master && !matches!(kind, SheetKind::Master) {
        pr.action.push_str(" (No approved master design yet: follow the manga panels.)");
    }
    pr.colour = colour_text(c);
    pr.keep = keep_text(c);
    pr.avoid = avoid_section(style, "No other characters, no background scenery, no props unless named.");
    finish(pr, pics, aspect, variants, settings)
}

// ---------------------------------------------------------------- props

fn prop_spec(p: &Prop, style: &Style, store: &Store, bible: &BibleIndex, settings: &ProjectSettings) -> GenSpec {
    let name = if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() };
    let mut pics = Vec::new();
    for crop in best_crops(&p.crops, &[CropFraming::Full]).into_iter().take(3) {
        pics.extend(crop_pic(
            store,
            crop,
            RefRole::Prop(p.id.clone()),
            SiteSlot::Add,
            format!("a manga panel showing the {name}"),
            format!("the {name}'s shape, proportions and details as drawn, not the drawing style or anything else in the panel"),
        ));
    }
    pics.extend(style_pic(store, style));
    let pics = limit(pics);
    let mut pr = Prompt { style: style_section(style, &pics), pictures: pictures_section(&pics), ..Default::default() };
    pr.camera = "Prop design sheet: the object alone on a plain white background at one scale: front view, side view, top view and a close-up of its identifying detail, flat even lighting.".into();
    let states: Vec<&str> = p.states.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let states = if states.len() > 1 { format!(" Show it once in each state: {}.", states.join(", ")) } else { String::new() };
    pr.action = format!(
        "The {name}: {}{states} Drawn the way anime draws anything a character touches: outlined and cel-shaded like the characters, not painted like the backgrounds.",
        sentence(&p.description)
    );
    if let Some(owner) = p.owner.as_deref().and_then(|o| bible.character(o)) {
        pr.props = format!("It belongs to {}; its size matches {}'s hands ({:.2} m tall).", name_of(owner), name_of(owner), if owner.look.height_m > 0. { owner.look.height_m } else { 1.7 });
    }
    pr.avoid = avoid_section(style, "No people, no hands, no background scenery.");
    finish(pr, pics, "16:9", 0, settings)
}

// ---------------------------------------------------------------- places

fn place_name(p: &Place) -> String {
    if p.name.trim().is_empty() {
        p.id.clone()
    } else {
        p.name.trim().to_string()
    }
}

/// The state a plate or master is lit in: `id`, else the first, else plain daylight.
fn state_of(p: &Place, id: &str) -> PlaceState {
    p.states.iter().find(|s| s.id == id).or_else(|| p.states.first()).cloned().unwrap_or_else(|| PlaceState {
        id: "day".into(),
        name: "day".into(),
        key_dir_deg: 135.,
        kelvin: 5600,
        mood: "clear, neutral daylight".into(),
    })
}

/// Landmarks seen from a station, left to right, with their screen third and distance; and the
/// ones behind the camera.
fn visible(p: &Place, st: &Station) -> (Vec<String>, Vec<String>) {
    let (mut seen, mut behind): (Vec<(f32, String)>, Vec<String>) = (Vec::new(), Vec::new());
    let reach = p.plan.w_m.max(p.plan.d_m).max(1.);
    for l in &p.plan.landmarks {
        let name = if l.name.trim().is_empty() { l.id.clone() } else { l.name.trim().to_string() };
        // Depth along the view: behind the camera only when it is negative (a point merely off
        // to the side is neither seen nor behind).
        let yaw = st.yaw_deg.to_radians();
        let depth = (l.pos[0] - st.pos[0]) * yaw.sin() - (l.pos[1] - st.pos[2]) * yaw.cos();
        if depth.is_finite() && depth <= 0. {
            behind.push(format!("the {name}"));
            continue;
        }
        match geometry::screen_x(st.pos, st.yaw_deg, st.lens_mm as f32, l.pos) {
            Some(x) if (0. ..=1.).contains(&x) => {
                let third = if x < 1. / 3. {
                    "left"
                } else if x > 2. / 3. {
                    "right"
                } else {
                    "centre"
                };
                let d = ((l.pos[0] - st.pos[0]).powi(2) + (l.pos[1] - st.pos[2]).powi(2)).sqrt();
                let dist = if d < 0.25 * reach {
                    "near"
                } else if d > 0.6 * reach {
                    "far"
                } else {
                    "middle distance"
                };
                seen.push((x, format!("the {name} ({third}, {dist})")));
            }
            _ => {}
        }
    }
    seen.sort_by(|a, b| a.0.total_cmp(&b.0));
    (seen.into_iter().map(|(_, s)| s).collect(), behind)
}

/// The camera paragraph of a station: where it stands, how high, where it looks, the lens, the
/// perspective and the horizon (computed, as the frames' prompts are).
fn station_words(p: &Place, st: &Station) -> String {
    let plan = &p.plan;
    if st.overhead {
        // RUN1-21: plates came back as a high oblique view, and a 6 m x 20 m hallway as a wide
        // room. Say "straight down" plainly and give the plan's own proportions.
        let (w, d) = (plan.w_m.max(0.), plan.d_m.max(0.));
        let shape = if w > 0. && d > 0. {
            let ratio = if w >= d { format!("{:.1} times as wide as it is deep", w / d) } else { format!("{:.1} times as deep (north to south) as it is wide", d / w) };
            format!(" The floor is {w:.0} m wide (west to east) by {d:.0} m long (north to south): draw it at exactly these proportions, {ratio}, the whole floor in view with a little margin.")
        } else {
            " The whole floor in view with a little margin.".to_string()
        };
        return format!(
            "Seen from directly above, top-down plan view, no perspective: the camera points straight down at the floor of {} like an architect's floor plan, north at the top of the picture. Walls show only as their top edges, no wall faces, no horizon, no sky, no vanishing lines, no tilt.{shape}",
            place_name(p)
        );
    }
    let camera = Camera { station: Some(st.id.clone()), pos: Some(st.pos), look_at: LookAt::Angles { yaw_deg: st.yaw_deg, pitch_deg: st.pitch_deg }, lens_mm: st.lens_mm, ..Default::default() };
    let shot = Shot { camera: camera.clone(), ..Default::default() };
    let d = geometry::derive(&shot, Some(p));
    let words = geometry::wording(&d, &camera);
    let h = st.pos[1];
    let height = if h < 1.0 {
        format!("low, {h:.1} m above the ground")
    } else if h <= 1.8 {
        format!("at eye level, {h:.1} m high")
    } else {
        format!("high, {h:.1} m up")
    };
    let tilt = if st.pitch_deg >= 5. {
        format!(", tilted up {:.0}°", st.pitch_deg)
    } else if st.pitch_deg <= -5. {
        format!(", tilted down {:.0}°", -st.pitch_deg)
    } else {
        String::new()
    };
    let pct = (d.horizon_y * 100.).round();
    let horizon = if (0. ..=100.).contains(&pct) { format!("the horizon line at {pct:.0}% from the top of the picture") } else { "the horizon out of the picture".to_string() };
    format!(
        "Camera \"{}\" of {}: standing {}, {height}, facing {}{tilt}. Lens {}. {}, {horizon}.",
        st.name.trim(),
        place_name(p),
        plan_words([st.pos[0], st.pos[2]], plan),
        compass(st.yaw_deg),
        words.lens,
        capitalise(&words.perspective)
    )
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The fixed elements as seen from a station (or, from above, where each one stands).
fn layout_words(p: &Place, st: &Station) -> String {
    if st.overhead {
        let items: Vec<String> = p.plan.landmarks.iter().map(|l| format!("the {} {}", l.name.trim(), plan_words(l.pos, &p.plan))).collect();
        let size = format!("The place is {:.0} m wide (west to east) and {:.0} m deep (north to south).", p.plan.w_m, p.plan.d_m);
        return if items.is_empty() { size } else { format!("{size} Seen from above: {}.", items.join("; ")) };
    }
    let (seen, behind) = visible(p, st);
    let mut out = String::new();
    if !seen.is_empty() {
        out.push_str(&format!("Visible from here, left to right: {}.", seen.join(", ")));
    }
    if !behind.is_empty() {
        out.push_str(&format!(" Behind the camera, not in the picture: {}.", behind.join(", ")));
    }
    out.trim().to_string()
}

/// The station the environment master is painted from: the non-overhead station that sees the
/// most landmarks (ties: the wider lens).
pub fn master_station(p: &Place) -> Option<&Station> {
    p.stations.iter().filter(|s| !s.overhead).max_by(|a, b| {
        let (va, vb) = (visible(p, a).0.len(), visible(p, b).0.len());
        va.cmp(&vb).then(b.lens_mm.cmp(&a.lens_mm))
    })
}

/// The station opposite the master (the reverse view), if the place has one.
pub fn reverse_station(p: &Place) -> Option<&Station> {
    let m = master_station(p)?;
    p.stations
        .iter()
        .filter(|s| !s.overhead && s.id != m.id)
        .map(|s| (s, (s.yaw_deg - m.yaw_deg - 180.).rem_euclid(360.)))
        .map(|(s, d)| (s, d.min(360. - d)))
        .filter(|(_, d)| *d <= 60.)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, _)| s)
}

/// A synthetic wide station when a place has none: the south side looking north, 24 mm.
fn fallback_station(p: &Place) -> Station {
    Station { id: String::new(), name: "Wide, from the south side".into(), pos: [0., 1.5, (p.plan.d_m / 2. - 0.5).max(0.)], yaw_deg: 0., pitch_deg: 0., lens_mm: 24, overhead: false }
}

fn place_crop_pics(store: &Store, p: &Place, n: usize) -> Vec<Pic> {
    let name = place_name(p);
    best_crops(&p.crops, &[CropFraming::Full])
        .into_iter()
        .filter_map(|crop| {
            crop_pic(
                store,
                crop,
                RefRole::Panel,
                SiteSlot::Add,
                format!("a manga panel of {name}"),
                "the architecture and where the fixed elements stand, not its drawing style, its people or its camera".into(),
            )
        })
        .take(n)
        .collect()
}

fn place_kind_words(p: &Place) -> &'static str {
    if p.kind == PlaceKind::Exterior {
        "exterior"
    } else {
        "interior"
    }
}

fn master_spec(p: &Place, style: &Style, store: &Store, settings: &ProjectSettings) -> GenSpec {
    let name = place_name(p);
    let st = master_station(p).cloned().unwrap_or_else(|| fallback_station(p));
    let state = state_of(p, "");
    // Never the overhead plate: the plates are painted from this master (one-way references).
    let mut pics = place_crop_pics(store, p, 3);
    pics.extend(style_pic(store, style));
    let pics = limit(pics);
    let mut pr = Prompt { style: style_section(style, &pics), pictures: pictures_section(&pics), ..Default::default() };
    pr.camera = format!("Anime background art, establishing wide view of an empty {}. {}", place_kind_words(p), station_words(p, &st));
    pr.action = format!(
        "{name}: {} {} This picture is the master every later background of {name} is painted from: every element keeps its place, size and colour.",
        sentence(&p.description),
        layout_words(p, &st)
    );
    pr.colour = format!("{} {}", light_words(&state, Some(st.yaw_deg)), forces_words(p, Some(st.yaw_deg)));
    pr.avoid = avoid_section(style, "No people, no characters, no animals, no vehicles unless they are part of the place.");
    finish(pr, pics, &settings.aspect, 0, settings)
}

fn plate_spec_inner(p: &Place, station: &str, state: &str, style: &Style, store: &Store, settings: &ProjectSettings) -> GenSpec {
    let name = place_name(p);
    let st = p.stations.iter().find(|s| s.id == station).cloned().or_else(|| master_station(p).cloned()).unwrap_or_else(|| fallback_station(p));
    let state = state_of(p, state);
    let mut pics = Vec::new();
    let master = card_pic(
        store,
        &p.sheet,
        false,
        RefRole::PlaceView,
        SiteSlot::Add,
        format!("the master view of {name}"),
        "the place itself: architecture, materials, colours and every fixed element, kept exactly; not its camera".into(),
    );
    let has_master = master.is_some();
    pics.extend(master);
    if !st.overhead {
        if let Some(over) = p.stations.iter().find(|s| s.overhead) {
            if let Some(pl) = p.plates.iter().find(|pl| pl.station == over.id && pl.card.status == Status::Approved) {
                pics.extend(card_pic(store, &pl.card, true, RefRole::Layout, SiteSlot::Add, format!("{name} seen from straight above"), "where every element stands on the floor".into()));
            }
        }
        // The nearest approved neighbour view (same state first) keeps adjacent plates in step.
        // Only stations earlier in the place's order: references stay one-way, so redrawing a
        // plate never outdates the plate it was drawn from.
        let here = p.stations.iter().position(|s| s.id == st.id).unwrap_or(usize::MAX);
        let neighbour = p
            .plates
            .iter()
            .filter(|pl| pl.station != st.id && pl.card.status == Status::Approved)
            .filter_map(|pl| p.stations.iter().position(|s| s.id == pl.station && !s.overhead).filter(|i| *i < here).map(|i| (pl, &p.stations[i])))
            .min_by(|(pa, sa), (pb, sb)| {
                let gap = |s: &Station| {
                    let d = (s.yaw_deg - st.yaw_deg).rem_euclid(360.);
                    d.min(360. - d)
                };
                u8::from(pa.state != state.id).cmp(&u8::from(pb.state != state.id)).then(gap(sa).total_cmp(&gap(sb)))
            });
        if let Some((pl, s)) = neighbour {
            pics.extend(card_pic(
                store,
                &pl.card,
                true,
                RefRole::PlaceView,
                SiteSlot::Add,
                format!("the approved view of {name} from \"{}\"", s.name.trim()),
                "how the same place looks from a nearby camera, to match surfaces and light".into(),
            ));
        }
    }
    if !has_master {
        pics.extend(place_crop_pics(store, p, 2));
    }
    pics.extend(style_pic(store, style));
    let pics = limit(pics);
    let mut pr = Prompt { style: style_section(style, &pics), pictures: pictures_section(&pics), ..Default::default() };
    let same = if has_master { " Same place as Image 1: identical architecture, furniture, materials and colours; do not add, move or remove anything." } else { "" };
    pr.camera = format!("Anime background plate of an empty {}, to be reused by every shot from this camera.{same} {}", place_kind_words(p), station_words(p, &st));
    pr.action = format!("{name}: {} {}", sentence(&p.description), layout_words(p, &st));
    let yaw = (!st.overhead).then_some(st.yaw_deg);
    pr.colour = format!("{} {}", light_words(&state, yaw), forces_words(p, yaw));
    pr.avoid = avoid_section(style, "No people, no characters, no animals: the background only.");
    finish(pr, pics, &settings.aspect, 0, settings)
}

// ---------------------------------------------------------------- the style frame

/// The style frame is the root of every other bible picture (each sheet, master and plate uses
/// it), so it never uses a drawn picture itself: only manga panels of the hero and the place.
/// The references stay one-way and redrawing one picture can never outdate itself through a
/// loop. Which hero and place it shows does not depend on any picture's approval either.
fn style_frame_inner(style: &Style, store: &Store, bible: &BibleIndex, settings: &ProjectSettings) -> GenSpec {
    let hero = bible.characters.iter().filter(|c| c.status != Status::Rejected).fold(None::<&Character>, |best, c| match best {
        Some(b) if b.role == Role::Hero || c.role != Role::Hero => Some(b),
        _ => Some(c),
    });
    let place = bible.places.iter().filter(|p| p.status != Status::Rejected).fold(None::<&Place>, |best, p| match best {
        Some(b) if b.stations.len() >= p.stations.len() => Some(b),
        _ => Some(p),
    });
    let mut pics = Vec::new();
    if let Some(c) = hero {
        for crop in best_crops(&c.crops, &[CropFraming::Full, CropFraming::Bust, CropFraming::Face]).into_iter().take(2) {
            pics.extend(identity_crop(store, c, crop));
        }
    }
    if let Some(p) = place {
        pics.extend(place_crop_pics(store, p, 1));
    }
    let pics = limit(pics);
    let line = if style.line.trim().is_empty() { DEFAULT_STYLE_LINE } else { style.line.trim() };
    let mut pr = Prompt { style: sentence(line), pictures: pictures_section(&pics), ..Default::default() };
    pr.camera = "A finished frame from the TV anime: medium-wide shot, 35 mm lens, eye-level camera, the character on the left third, the place painted in full behind them.".into();
    let who = hero.map(|c| format!("{} {} {} stands on the left third.", look_text(c), outfit_text(c), name_of(c))).unwrap_or_else(|| "The main character stands on the left third.".into());
    let wher = place.map(|p| format!("Setting: {}, {}", place_name(p), sentence(&p.description))).unwrap_or_else(|| "Setting: a typical place of the story.".into());
    pr.action = format!(
        "{who} {wher} This frame sets the look of the whole film: line weight, cel shading, the painted background without outlines, the glow of light sources, the colour handling. Every later picture is told to match it."
    );
    let swatches: Vec<String> = style
        .palettes
        .first()
        .map(|pal| pal.swatches.iter().filter_map(|s| parse_hex(&s.hex).map(|rgb| format!("{} {} ({})", s.role, colour_name(rgb), s.hex.trim().to_uppercase()))).collect())
        .unwrap_or_default();
    if !swatches.is_empty() {
        pr.colour = format!("Palette: {}. Characters slightly more saturated than the background.", swatches.join(", "));
    }
    if let Some(c) = hero {
        pr.keep = keep_text(c);
    }
    pr.avoid = avoid_section(style, "No other characters.");
    finish(pr, pics, &settings.aspect, 0, settings)
}

// ---------------------------------------------------------------- specs

fn finish(pr: Prompt, pics: Vec<Pic>, aspect: &str, variants: u8, settings: &ProjectSettings) -> GenSpec {
    let model = if settings.picture_model.trim().is_empty() { ProjectSettings::default().picture_model } else { settings.picture_model.clone() };
    let res = if settings.picture_res.trim().is_empty() { "1K".to_string() } else { settings.picture_res.clone() };
    let aspect = if aspect.trim().is_empty() { "16:9".to_string() } else { aspect.to_string() };
    let variants = if variants == 0 { settings.variants.clamp(1, 4) } else { variants.clamp(1, 4) };
    GenSpec { model, prompt_ai: pr.text(), prompt_user: None, prompt_user_base: None, refs: pics.into_iter().map(|p| p.slot).collect(), refs_user: false, aspect, res, variants }
}

fn context(store: &Store) -> (BibleIndex, ProjectSettings) {
    (store.bible().unwrap_or_default(), store.series().map(|s| s.settings).unwrap_or_default())
}

/// The spec of one sheet (place: `SheetKind::Master` = the environment master).
pub fn spec(kind: &SheetKind, subject: Subject<'_>, style: &Style, store: &Store) -> GenSpec {
    let (bible, settings) = context(store);
    match subject {
        Subject::Character(c) => character_spec(kind, c, style, store, &bible, &settings),
        Subject::Place(p) => master_spec(p, style, store, &settings),
        Subject::Prop(p) => prop_spec(p, style, store, &bible, &settings),
    }
}

/// The empty-room plate of a place from a station in a state (no characters).
pub fn plate_spec(place: &Place, station: &str, state: &str, style: &Style, store: &Store) -> GenSpec {
    let (_, settings) = context(store);
    plate_spec_inner(place, station, state, style, store, &settings)
}

/// The style frame.
pub fn style_frame_spec(style: &Style, store: &Store) -> GenSpec {
    let (bible, settings) = context(store);
    style_frame_inner(style, store, &bible, &settings)
}

// ---------------------------------------------------------------- the job

/// The engine's spec for a target with what the user set on the card kept: their prompt, their
/// references, the size and the model.
fn spec_for(target: &Target, bible: &BibleIndex, store: &Store) -> Result<(GenSpec, String)> {
    let style = &bible.style;
    let (fresh, saved, template) = match target {
        Target::Sheet { owner, sheet } => {
            let kind = sheet_kind(sheet).ok_or_else(|| anyhow!("This kind of sheet ({sheet}) was made by a newer MangaVox. Update MangaVox to draw it."))?;
            if let Some(c) = bible.character(owner) {
                (spec(&kind, Subject::Character(c), style, store), sheet_card(&c.sheets, &kind).map(|c| c.gen.clone()), format!("nb21.sheet.{}@1", sheet_key(&kind)))
            } else if let Some(p) = bible.prop(owner) {
                (spec(&kind, Subject::Prop(p), style, store), sheet_card(&p.sheets, &kind).map(|c| c.gen.clone()), "nb21.prop@1".into())
            } else {
                bail!("That character or prop is no longer in the bible.");
            }
        }
        Target::PlaceSheet { place } => {
            let p = bible.place(place).ok_or_else(|| anyhow!("That place is no longer in the bible."))?;
            (spec(&SheetKind::Master, Subject::Place(p), style, store), Some(p.sheet.gen.clone()), "nb21.place@1".into())
        }
        Target::Plate { place, station, state } => {
            let p = bible.place(place).ok_or_else(|| anyhow!("That place is no longer in the bible."))?;
            if !p.stations.iter().any(|s| &s.id == station) {
                bail!("The camera station of this plate is no longer on {}'s floor plan.", place_name(p));
            }
            let saved = p.plates.iter().find(|pl| &pl.station == station && &pl.state == state).map(|pl| pl.card.gen.clone());
            (plate_spec(p, station, state, style, store), saved, "nb21.plate@1".into())
        }
        Target::StyleFrame => (style_frame_spec(style, store), Some(style.frame.gen.clone()), "nb21.style@1".into()),
        _ => bail!("This picture is not a bible sheet."),
    };
    let mut spec = fresh;
    if let Some(saved) = saved {
        if saved.prompt_user.as_deref().is_some_and(|p| !p.trim().is_empty()) {
            spec.prompt_user = saved.prompt_user;
            spec.prompt_user_base = saved.prompt_user_base;
        }
        if saved.refs_user {
            spec.refs = saved.refs;
            spec.refs_user = true;
        }
        if !saved.aspect.trim().is_empty() {
            spec.aspect = saved.aspect;
        }
        if !saved.model.trim().is_empty() {
            spec.model = saved.model;
        }
        if !saved.res.trim().is_empty() {
            spec.res = saved.res;
        }
    }
    Ok((spec, template))
}

/// The role label sent with a reference (a plate's contains "plate": it is uploaded larger).
fn role_label(r: &RefRole) -> String {
    match r {
        RefRole::Plate => "plate".into(),
        RefRole::Layout => "layout".into(),
        RefRole::Identity(id) => format!("identity:{id}"),
        RefRole::Style => "style".into(),
        RefRole::Expression(id) => format!("expression:{id}"),
        RefRole::Prop(id) => format!("prop:{id}"),
        RefRole::Grip(id) => format!("grip:{id}"),
        RefRole::ColourModel(id) => format!("colour_model:{id}"),
        RefRole::PlaceView => "place view".into(),
        RefRole::Panel => "panel".into(),
        RefRole::User => "user".into(),
        RefRole::Unknown => "reference".into(),
    }
}

/// Finds the card a target fills, creating the sheet or plate entry when it is new, and lets `f`
/// change it; saves the entity.
fn with_card(store: &Store, target: &Target, f: impl FnOnce(&mut Card)) -> Result<()> {
    with_card_extra(store, target, |card, _| f(card))
}

/// [`with_card`], also handing `f` the `extra` map of the entity that owns the card. The read and
/// the save run under the project's bible lock ([`Store::with_bible`]), so the Bible screen's
/// commit and another job's card update never interleave with it.
fn with_card_extra(store: &Store, target: &Target, f: impl FnOnce(&mut Card, &mut serde_json::Map<String, Value>)) -> Result<()> {
    store.with_bible(|| with_card_extra_locked(store, target, f))
}

fn with_card_extra_locked(store: &Store, target: &Target, f: impl FnOnce(&mut Card, &mut serde_json::Map<String, Value>)) -> Result<()> {
    let bible = store.bible()?;
    match target {
        Target::Sheet { owner, sheet } => {
            let kind = sheet_kind(sheet).ok_or_else(|| anyhow!("Unknown sheet {sheet}."))?;
            let entry = |sheets: &mut Vec<Sheet>| -> usize {
                match sheets.iter().position(|s| s.kind == kind) {
                    Some(i) => i,
                    None => {
                        sheets.push(Sheet { kind: kind.clone(), card: Card::default() });
                        sheets.len() - 1
                    }
                }
            };
            if let Some(mut c) = bible.character(owner).cloned() {
                let i = entry(&mut c.sheets);
                f(&mut c.sheets[i].card, &mut c.extra);
                store.save_character(&c)
            } else if let Some(mut p) = bible.prop(owner).cloned() {
                let i = entry(&mut p.sheets);
                f(&mut p.sheets[i].card, &mut p.extra);
                store.save_prop(&p)
            } else {
                bail!("That character or prop is no longer in the bible.")
            }
        }
        Target::PlaceSheet { place } => {
            let mut p = bible.place(place).cloned().ok_or_else(|| anyhow!("That place is no longer in the bible."))?;
            f(&mut p.sheet, &mut p.extra);
            store.save_place(&p)
        }
        Target::Plate { place, station, state } => {
            let mut p = bible.place(place).cloned().ok_or_else(|| anyhow!("That place is no longer in the bible."))?;
            let i = match p.plates.iter().position(|pl| &pl.station == station && &pl.state == state) {
                Some(i) => i,
                None => {
                    p.plates.push(crate::model::Plate { station: station.clone(), state: state.clone(), card: Card::default() });
                    p.plates.len() - 1
                }
            };
            f(&mut p.plates[i].card, &mut p.extra);
            store.save_place(&p)
        }
        Target::StyleFrame => {
            let mut s = bible.style.clone();
            f(&mut s.frame, &mut s.extra);
            store.save_style(&s)
        }
        _ => bail!("This picture is not a bible sheet."),
    }
}

/// Key in a bible entity's `extra`: `{take id: request hash}` of the pictures the user approved
/// as they are, though the request had changed since they were drawn.
pub const EXTRA_ACCEPTED: &str = "accepted_requests";

/// The `extra` map of the entity a bible target belongs to.
fn extra_of<'a>(bible: &'a BibleIndex, target: &Target) -> Option<&'a serde_json::Map<String, Value>> {
    match target {
        Target::Sheet { owner, .. } => bible.character(owner).map(|c| &c.extra).or_else(|| bible.prop(owner).map(|p| &p.extra)),
        Target::PlaceSheet { place } | Target::Plate { place, .. } => bible.place(place).map(|p| &p.extra),
        Target::StyleFrame => Some(&bible.style.extra),
        _ => None,
    }
}

/// The request hash the user accepted for a take (approving it as it is).
fn accepted_hash<'a>(bible: &'a BibleIndex, target: &Target, take: &str) -> Option<&'a str> {
    extra_of(bible, target)?.get(EXTRA_ACCEPTED)?.get(take)?.as_str()
}

/// Approves a bible picture's selected take and accepts it as it is: the request the engine
/// would send now is remembered with it, so an "outdated" mark clears until the prompt, the
/// references or the settings change again. The bible screens call this when the user approves.
pub fn approve(store: &Store, target: &Target) -> Result<()> {
    let bible = store.bible()?;
    let picked = card_of(&bible, target).and_then(|c| c.selected.clone()).ok_or_else(|| anyhow!("Pick a picture first, then approve it."))?;
    let now = spec_for(target, &bible, store).map(|(spec, template)| request_hash(&request_of(&spec, template))).ok();
    with_card_extra(store, target, |card, extra| {
        let take = card.selected.clone().unwrap_or(picked);
        card.selected = Some(take.clone());
        card.status = Status::Approved;
        let Some(hash) = now else { return };
        let entry = extra.entry(EXTRA_ACCEPTED.to_string()).or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(serde_json::Map::new());
        }
        if let Some(m) = entry.as_object_mut() {
            // A card accepts one request: its other pictures' entries go.
            m.retain(|k, _| !card.takes.contains(k));
            m.insert(take, Value::String(hash));
        }
    })
}

/// The card a bible target fills, if it exists.
fn card_of<'a>(bible: &'a BibleIndex, target: &Target) -> Option<&'a Card> {
    match target {
        Target::Sheet { owner, sheet } => {
            let kind = sheet_kind(sheet)?;
            bible.character(owner).map(|c| &c.sheets).or_else(|| bible.prop(owner).map(|p| &p.sheets)).and_then(|sheets| sheet_card(sheets, &kind))
        }
        Target::PlaceSheet { place } => bible.place(place).map(|p| &p.sheet),
        Target::Plate { place, station, state } => bible.place(place)?.plates.iter().find(|pl| &pl.station == station && &pl.state == state).map(|pl| &pl.card),
        Target::StyleFrame => Some(&bible.style.frame),
        _ => None,
    }
}

/// Exactly what is sent for a spec (its hash is the picture channel of design 5.5).
fn request_of(spec: &GenSpec, template: String) -> GenRequest {
    let prompt = spec.prompt_user.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| spec.prompt_ai.clone());
    GenRequest {
        template,
        model: spec.model.clone(),
        prompt,
        refs: spec.refs.iter().map(|r| (r.hash.clone(), role_label(&r.role), r.slot)).collect(),
        aspect: spec.aspect.clone(),
        res: spec.res.clone(),
    }
}

/// The spec the Sheet job would use for a bible picture now: the engine's, with the prompt,
/// references, aspect, model and size the user set on the card kept (the Generation panel shows
/// it).
pub fn current_spec(store: &Store, target: &Target) -> Result<GenSpec> {
    let bible = store.bible()?;
    Ok(spec_for(target, &bible, store)?.0)
}

/// The request the Sheet job would send for a bible picture now (sheet, place master, plate or
/// style frame).
pub fn current_request(store: &Store, target: &Target) -> Result<GenRequest> {
    let bible = store.bible()?;
    let (spec, template) = spec_for(target, &bible, store)?;
    Ok(request_of(&spec, template))
}

/// A bible picture's selected take was drawn from a different request than the one the engine
/// would send now (its prompt, references or settings changed). A card the user chose to keep
/// (`locked`), a card with nothing selected, an uploaded picture, and an approved picture whose
/// current request the user accepted ([`approve`]) are never outdated.
pub fn outdated(card: &Card, store: &Store) -> bool {
    outdated_with(card, None, store)
}

/// [`outdated`] with the bible already loaded (a screen checking every card at once).
pub fn outdated_in(card: &Card, bible: &BibleIndex, store: &Store) -> bool {
    outdated_with(card, Some(bible), store)
}

fn outdated_with(card: &Card, bible: Option<&BibleIndex>, store: &Store) -> bool {
    if card.locked {
        return false;
    }
    let Some(take) = card.selected.as_deref().and_then(|id| store.take(id)) else { return false };
    if !matches!(take.target, Target::Sheet { .. } | Target::PlaceSheet { .. } | Target::Plate { .. } | Target::StyleFrame) || take.request_hash.is_empty() {
        return false;
    }
    let loaded;
    let bible = match bible {
        Some(b) => b,
        None => match store.bible() {
            Ok(b) => {
                loaded = b;
                &loaded
            }
            Err(_) => return false,
        },
    };
    let Ok(now) = spec_for(&take.target, bible, store).map(|(spec, template)| request_hash(&request_of(&spec, template))) else { return false };
    if now == take.request_hash {
        return false;
    }
    !(card.status == Status::Approved && accepted_hash(bible, &take.target, &take.id) == Some(now.as_str()))
}

/// What a bible picture is for, in words for the list of Magnific pictures ("Bong Ju-hyuk ·
/// face sheet", "Rooftop · plate").
pub fn what_for(target: &Target, bible: &BibleIndex) -> String {
    let words = |kind: Option<SheetKind>| match kind {
        Some(SheetKind::Master) => "character design",
        Some(SheetKind::Face) => "face sheet",
        Some(SheetKind::Turnaround) => "turnaround",
        Some(SheetKind::Expressions) => "expression sheet",
        Some(SheetKind::Outfit(_)) => "outfit sheet",
        Some(SheetKind::Grip(_)) => "grip sheet",
        Some(SheetKind::ColourModel) => "colour model",
        Some(SheetKind::Back) => "back view",
        _ => "sheet",
    };
    match target {
        Target::Sheet { owner, sheet } => {
            let who = bible.character(owner).map(name_of).or_else(|| bible.prop(owner).map(|p| if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() })).unwrap_or_else(|| owner.clone());
            let kind = sheet_kind(sheet);
            let what = if bible.character(owner).is_none() && matches!(kind, Some(SheetKind::Master)) { "prop design" } else { words(kind) };
            format!("{who} \u{b7} {what}")
        }
        Target::PlaceSheet { place } => format!("{} \u{b7} place design", bible.place(place).map(place_name).unwrap_or_else(|| place.clone())),
        Target::Plate { place, .. } => format!("{} \u{b7} background plate", bible.place(place).map(place_name).unwrap_or_else(|| place.clone())),
        Target::StyleFrame => "Style frame".into(),
        _ => "Picture".into(),
    }
}

/// One picture of a target made on Magnific's website, for matching with its Creations list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeOnMagnific {
    /// Magnific's own id (the number in the picture's address on magnific.com).
    pub site_id: String,
    /// MangaVox pressed Generate (false: the person at the window had pressed it).
    pub pressed_by_app: bool,
}

/// The pictures of a bible card or a shot's frame that came from Magnific with their Magnific
/// id, newest first. Blocking (reads the card and its takes): call it off the UI thread.
pub fn made_on_magnific(store: &Store, target: &Target) -> Vec<MadeOnMagnific> {
    let takes: Vec<String> = match target {
        Target::ShotFrame { episode, shot } => store.all_shots(episode).ok().and_then(|all| all.into_values().flatten().find(|s| &s.id == shot)).map(|s| s.frame.takes).unwrap_or_default(),
        _ => store.bible().ok().and_then(|b| card_of(&b, target).map(|c| c.takes.clone())).unwrap_or_default(),
    };
    takes
        .iter()
        .rev()
        .filter_map(|id| store.take(id))
        .filter_map(|t| match t.source {
            TakeSource::MagnificWeb { site_id, .. } if !site_id.trim().is_empty() => Some(MadeOnMagnific { site_id: site_id.trim().to_string(), pressed_by_app: t.note != crate::store::PRESSED_AT_WINDOW }),
            _ => None,
        })
        .collect()
}

/// The Sheet job: draws `variants` pictures for a sheet, place master, plate or the style frame.
pub async fn run_job(cx: &JobCtx<'_>, target: &Target, variants: u8) -> Result<()> {
    let store = cx.store;
    let bible = store.bible()?;
    let (spec, template) = spec_for(target, &bible, store)?;
    let mut files: Vec<(PathBuf, RefSlot)> = Vec::new();
    for r in &spec.refs {
        let abs = store.path(&r.file);
        if !abs.is_file() {
            bail!("A reference picture is missing ({}). Remove it from the references or add it again, then generate.", r.file);
        }
        files.push((abs, r.clone()));
    }
    if files.len() > MAX_REFS {
        bail!("This picture has {} references; Magnific takes at most {MAX_REFS}. Remove some, then generate.", files.len());
    }
    let req = request_of(&spec, template);
    let channel = request_hash(&req);
    // The card keeps the spec it was drawn from (the user's prompt and references stay theirs),
    // but only while it still holds the settings this job read: settings the user saved since
    // are never overwritten by the job's older copy.
    let read_at_start = card_of(&bible, target).map(|c| c.gen.clone()).unwrap_or_default();
    let what = what_for(target, &bible);
    drop(bible);
    let saved = spec.clone();
    with_card(store, target, |card| {
        if card.gen == read_at_start {
            card.gen = saved;
        }
    })?;

    let count = variants.clamp(1, 4);
    let batch = format!("{}-{}", cx.job.id, crate::store::now_ms());
    for n in 0..count {
        if cx.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let line = format!("Drawing picture {} of {count}…", n + 1);
        (cx.progress)(n as f32 / count as f32, &line);
        let progress = cx.progress;
        let base = n as f32 / count as f32;
        let status = Box::new(move |s: &str| progress(base, s));
        // Tied to cancel like the Frame job: dropping the wait tells the page to stop at once,
        // so Cancel, Pause, Studio mode off and removing the project free the lane right away.
        let pictures = tokio::select! {
            r = cx.run.image.generate(&req, &files, 1, status) => r.map_err(anyhow::Error::new)?,
            _ = cx.cancel.cancelled() => bail!("cancelled"),
        };
        for bytes in pictures {
            let (w, h) = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_dimensions().ok())
                .ok_or_else(|| anyhow!("Magnific sent back something that is not a picture, so it was not kept. Try again."))?;
            let ext = match image::guess_format(&bytes) {
                Ok(image::ImageFormat::Jpeg) => "jpg",
                Ok(image::ImageFormat::WebP) => "webp",
                _ => "png",
            };
            // Magnific's own id of the picture and who pressed Generate, kept with the take and
            // in the list of pictures MangaVox made (never in the prompt).
            let mark = crate::store::magnific_picture_made(store, &bytes, &req.model, &what, &req.prompt).unwrap_or_default();
            let note = if mark.site_id.is_empty() || mark.pressed_by_app { String::new() } else { crate::store::PRESSED_AT_WINDOW.to_string() };
            let take = store.add_take(
                &bytes,
                ext,
                TakeMeta {
                    kind: TakeKind::Image,
                    target: target.clone(),
                    request: Some(req.clone()),
                    request_hash: channel.clone(),
                    source: TakeSource::MagnificWeb { model: req.model.clone(), res: req.res.clone(), free: true, site_id: mark.site_id },
                    w,
                    h,
                    batch: Some(batch.clone()),
                    note,
                    ..Default::default()
                },
            )?;
            with_card(store, target, |card| {
                if !card.takes.contains(&take.id) {
                    card.takes.push(take.id.clone());
                }
                if card.selected.is_none() {
                    card.selected = Some(take.id.clone());
                }
                if card.status != Status::Approved {
                    card.status = Status::NeedsReview;
                }
            })?;
        }
    }
    (cx.progress)(1., &format!("{count} picture{} ready to review", if count == 1 { "" } else { "s" }));
    Ok(())
}
