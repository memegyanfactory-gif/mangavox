//! Reference crops from the manga's full-resolution pages (design 6.2): face, bust and full
//! framings scored by face size and sharpness. Only crops two signals agree on (the LLM pick
//! and the panel listing) or the user approved are used as references; the others are offered to
//! the user as suggestions (`agreed == 1`, not approved).
//!
//! Signals for a character: the LLM's `best_panels` (kept in `Character.extra`, see
//! `bible::EXTRA_BEST_PANELS`) and Studio's Read listing the character in the panel. Where the
//! Read has no face box for the character at all, whole panels are cut instead.
//!
//! Cutting is two passes: every candidate is measured (pages decoded once each), the best are
//! chosen for score and variety, and only those are encoded and stored as takes
//! (`TakeSource::MangaCrop`), so a long chapter never fills the take store with rejects.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, Result};
use image::{imageops::FilterType, RgbImage};
use mv_recap::{ChapterRead, ChapterRef};
use serde_json::Value;

use crate::bible::EXTRA_BEST_PANELS;
use crate::model::{Character, CropFraming, RefCrop, SourceRef, Target, TakeKind, TakeMeta, TakeSource, ViewAngle};
use crate::queue::JobCtx;
use crate::source;
use crate::store::Store;

/// Best crops kept per character.
pub const KEEP: usize = 12;
/// Crops smaller than this on their short side are too small to be references.
pub const MIN_SIDE: u32 = 96;
/// A crop grows towards this short side when the page has the pixels.
pub const GOOD_SIDE: u32 = 512;
const JPEG_QUALITY: u8 = 92;
/// Panel crops kept per place or prop.
const KEEP_PANELS: usize = 6;

/// A crop the engine may send as a reference: the user approved it or two signals agree.
pub fn usable(c: &RefCrop) -> bool {
    c.approved || c.agreed >= 2
}

/// One crop to cut: where, how framed, how it was found.
#[derive(Clone, Debug)]
struct Want {
    at: SourceRef,
    framing: CropFraming,
    angle: ViewAngle,
    /// Page-relative boxes `[x0, y0, x1, y1]`.
    face: Option<[f32; 4]>,
    panel: Option<[f32; 4]>,
    agreed: u8,
    /// A place panel: how much of it the people take (0 = nobody, 1 = all faces).
    people: Option<f32>,
}

/// A measured crop: its pixel box on the page and its score.
#[derive(Clone, Debug)]
struct Measured {
    want: Want,
    rect: [u32; 4],
    score: f32,
    /// [`not_a_picture`] of the crop.
    text: f32,
}

/// The version of the crop scoring. A bible stamped with an older one (or none) is scored again
/// locally ([`crate::bible::upgrade_crops`]): version 2 scores lettering and blank frames down and
/// keeps people panels off place covers.
pub const SCORING_VERSION: u64 = 2;
/// `Style.extra` key holding the [`SCORING_VERSION`] the bible's crops were scored with.
pub const EXTRA_SCORING: &str = "crop_scoring";
/// `Place.extra` key: the take ids of the place's crops that may be its cover (panels that show
/// the place: few or small people, no lettering).
pub const EXTRA_COVERS: &str = "cover_panels";
/// A place panel is a cover when the people's faces take at most this share of it.
const COVER_PEOPLE_MAX: f32 = 0.02;

/// A new cut's crop and whether its picture was in the project before this cut (cut once, then
/// dropped by the user when it is no longer in the entity's crops).
pub type Cut = (RefCrop, bool);

fn valid_box(b: &[f32; 4]) -> bool {
    b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1] && b[2] > 0. && b[3] > 0. && b[0] < 1. && b[1] < 1.
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn word<T: serde::de::DeserializeOwned + PartialEq>(s: &str, unknown: T, fallback: T) -> T {
    match serde_json::from_value::<T>(Value::String(s.trim().to_lowercase().replace([' ', '-'], "_"))) {
        Ok(v) if v != unknown => v,
        _ => fallback,
    }
}

/// Cuts, scores and stores (as takes) the best crops of a character from the episode's reads.
pub fn pick(store: &Store, character: &Character, reads: &[(ChapterRef, ChapterRead)]) -> Vec<RefCrop> {
    pick_cut(store, character, reads).into_iter().map(|(c, _)| c).collect()
}

/// [`pick`], telling for each crop whether its picture was in the project before.
pub fn pick_cut(store: &Store, character: &Character, reads: &[(ChapterRef, ChapterRead)]) -> Vec<Cut> {
    let mut names: BTreeSet<String> = character.aliases.iter().map(|a| norm(a)).collect();
    names.insert(norm(&character.name));
    names.remove("");

    // Signal 1: the Read lists the character in the panel. Signal 2: the AI picked the panel.
    struct Cand {
        at: SourceRef,
        face: Option<[f32; 4]>,
        panel: Option<[f32; 4]>,
        listed: bool,
        llm: Option<(ViewAngle, Option<CropFraming>)>,
    }
    let mut cands: BTreeMap<(String, usize, usize), Cand> = BTreeMap::new();
    let mut any_face = false;
    for (chref, read) in reads {
        for page in &read.pages {
            for (k, panel) in page.panels.iter().enumerate() {
                let Some(seen) = panel.characters.iter().find(|s| names.contains(&norm(&s.name))) else { continue };
                let face = seen.face.as_ref().map(|b| b.0).filter(valid_box);
                any_face |= face.is_some();
                cands.insert(
                    (chref.chapter_id.clone(), page.index, k),
                    Cand {
                        at: SourceRef { chapter_id: chref.chapter_id.clone(), page: page.index, panel: Some(k) },
                        face,
                        panel: panel.bbox.as_ref().map(|b| b.0).filter(valid_box),
                        listed: true,
                        llm: None,
                    },
                );
            }
        }
    }
    let picks = character.extra.get(EXTRA_BEST_PANELS).and_then(Value::as_array).cloned().unwrap_or_default();
    for p in &picks {
        let refs = crate::bible::panel_refs(Some(&Value::Array(vec![p.clone()])));
        let Some(at) = refs.into_iter().next() else { continue };
        let Some(k) = at.panel else { continue };
        let Some(panel) = crate::bible::panel_of(reads, &at) else { continue };
        let angle = word(p.get("angle").and_then(Value::as_str).unwrap_or(""), ViewAngle::Unknown, ViewAngle::Front);
        let framing = p.get("framing").and_then(Value::as_str).map(|f| word(f, CropFraming::Unknown, CropFraming::Face));
        let e = cands.entry((at.chapter_id.clone(), at.page, k)).or_insert_with(|| Cand {
            at: at.clone(),
            face: None,
            panel: panel.bbox.as_ref().map(|b| b.0).filter(valid_box),
            listed: false,
            llm: None,
        });
        e.llm = Some((angle, framing));
    }

    let mut wants = Vec::new();
    for c in cands.values() {
        // Listed panels count when they have a face box, or when the Read has none for this
        // character anywhere (then whole panels stand in).
        if c.llm.is_none() && c.face.is_none() && any_face {
            continue;
        }
        let agreed = u8::from(c.listed) + u8::from(c.llm.is_some());
        let angle = c.llm.map(|l| l.0).unwrap_or(ViewAngle::Front);
        let mut framings = Vec::new();
        if c.face.is_some() {
            framings.extend([CropFraming::Face, CropFraming::Bust]);
            if matches!(c.llm, Some((_, Some(CropFraming::Full)))) {
                framings.push(CropFraming::Full);
            }
        } else {
            framings.push(CropFraming::Full);
        }
        for framing in framings {
            wants.push(Want { at: c.at.clone(), framing, angle, face: c.face, panel: c.panel, agreed, people: None });
        }
    }
    let measured = measure(reads, wants);
    let chosen = choose(measured, KEEP);
    store_crops(store, &character.id, reads, chosen).into_iter().map(|(c, existed, _)| (c, existed)).collect()
}

/// Whole-panel crops of a place or prop: `picks` are the panels with how many signals agree on
/// each (1 = the AI only, 2 = a second, independent signal). At most 6, best first.
pub fn panel_crops(store: &Store, owner: &str, picks: &[(SourceRef, u8)], reads: &[(ChapterRef, ChapterRead)]) -> Vec<RefCrop> {
    panel_cut(store, owner, picks, reads, false).0.into_iter().map(|(c, _)| c).collect()
}

/// Whole-panel crops of a place: as [`panel_crops`], but a panel full of people scores low (a
/// place's picture shows the place), and the take ids of the panels fit to be its cover: people
/// take little of them ([`COVER_PEOPLE_MAX`]) and they are not lettering.
pub fn place_crops(store: &Store, owner: &str, picks: &[(SourceRef, u8)], reads: &[(ChapterRef, ChapterRead)]) -> (Vec<Cut>, Vec<String>) {
    panel_cut(store, owner, picks, reads, true)
}

/// Prop crops with the "was it in the project before" flag.
pub fn prop_crops(store: &Store, owner: &str, picks: &[(SourceRef, u8)], reads: &[(ChapterRef, ChapterRead)]) -> Vec<Cut> {
    panel_cut(store, owner, picks, reads, false).0
}

/// How much of a panel the people in it take: their face boxes over the panel's box; a person
/// the Read saw without a face box counts as a medium figure.
fn people_share(panel: &mv_recap::Panel) -> f32 {
    let area = |b: [f32; 4]| ((b[2] - b[0]).max(0.) * (b[3] - b[1]).max(0.)).max(0.);
    let whole = panel.bbox.as_ref().map(|b| b.0).filter(valid_box).map(area).filter(|a| *a > 1e-4).unwrap_or(1.);
    let faces: f32 = panel.characters.iter().map(|c| c.face.as_ref().map(|b| b.0).filter(valid_box).map(area).map(|a| a / whole).unwrap_or(0.03)).sum();
    faces.clamp(0., 1.)
}

fn panel_cut(store: &Store, owner: &str, picks: &[(SourceRef, u8)], reads: &[(ChapterRef, ChapterRead)], place: bool) -> (Vec<Cut>, Vec<String>) {
    let mut seen = BTreeSet::new();
    let wants: Vec<Want> = picks
        .iter()
        .filter(|(at, _)| at.panel.is_some() && seen.insert((at.chapter_id.clone(), at.page, at.panel)))
        .filter_map(|(at, agreed)| {
            let panel = crate::bible::panel_of(reads, at)?;
            Some(Want {
                at: at.clone(),
                framing: CropFraming::Full,
                angle: ViewAngle::Front,
                face: None,
                panel: panel.bbox.as_ref().map(|b| b.0).filter(valid_box),
                agreed: (*agreed).clamp(1, 2),
                people: place.then(|| people_share(panel)),
            })
        })
        .collect();
    let measured = measure(reads, wants);
    let chosen = choose(measured, KEEP_PANELS);
    let made = store_crops(store, owner, reads, chosen);
    let covers = made.iter().filter(|(_, _, cover)| place && *cover).map(|(c, _, _)| c.take.clone()).collect();
    (made.into_iter().map(|(c, existed, _)| (c, existed)).collect(), covers)
}

/// A crop the user stands behind: approved, or a picture they uploaded (it has no manga page).
/// Only these are ever chosen as a card's cover: the lettering and people detectors are not
/// sure enough to pick a cover on their own.
pub fn accepted(c: &RefCrop) -> bool {
    c.approved || c.source.chapter_id.is_empty()
}

/// The best accepted crop of a list, `preferred` framings first, then the score.
fn best_accepted<'a>(crops: &'a [RefCrop], preferred: &[CropFraming]) -> Option<&'a RefCrop> {
    crops.iter().filter(|c| accepted(c)).max_by(|a, b| preferred.contains(&a.framing).cmp(&preferred.contains(&b.framing)).then(a.score.total_cmp(&b.score)))
}

/// A place's cover crop: the best one the user accepted (approved or uploaded), else `None`, and
/// the screen shows a neutral tile. An unreviewed panel crop is never chosen by itself (it may
/// be lettering or a crowd); it stays a candidate the user can accept.
pub fn place_cover(place: &crate::model::Place) -> Option<&RefCrop> {
    best_accepted(&place.crops, &[CropFraming::Full])
}

/// The picture file a place's card shows: its approved master sheet, else an approved plate,
/// else the crop the user accepted ([`place_cover`]); `None` = the neutral tile.
pub fn place_cover_file(store: &Store, place: &crate::model::Place) -> Option<std::path::PathBuf> {
    let approved = |card: &crate::model::Card| (card.status == crate::model::Status::Approved).then(|| card.selected.as_deref()).flatten().and_then(|id| store.take_file(id));
    approved(&place.sheet)
        .or_else(|| place.plates.iter().find_map(|p| approved(&p.card)))
        .or_else(|| place_cover(place).map(|c| store.path(&c.file)).filter(|p| p.is_file()))
}

/// A character's cover picture, strict and predictable: (a) the best crop the user accepted
/// (approved or uploaded; face and bust framings first), else (b) a face from Studio's character
/// brain when the character is linked to it (those are real face crops), else `None`, and the
/// screen shows the initials tile. An unreviewed crop is never chosen by itself.
pub fn character_cover(store: &Store, c: &Character, brain: &mv_recap::Brain) -> Option<std::path::PathBuf> {
    if let Some(p) = best_accepted(&c.crops, &[CropFraming::Face, CropFraming::Bust]).map(|r| store.path(&r.file)).filter(|p| p.is_file()) {
        return Some(p);
    }
    let id = c.brain_id.as_deref().filter(|b| !b.trim().is_empty())?;
    brain.characters.iter().find(|b| b.id == id)?.faces.iter().find(|f| f.is_file()).cloned()
}

/// The page file of a source position.
fn page_path(reads: &[(ChapterRef, ChapterRead)], at: &SourceRef) -> Option<std::path::PathBuf> {
    let (chref, _) = reads.iter().find(|(c, _)| c.chapter_id == at.chapter_id)?;
    mv_recap::list_pages(&chref.pages_dir).into_iter().nth(at.page)
}

fn load_page(path: &std::path::Path) -> Option<RgbImage> {
    match image::open(path) {
        Ok(img) => Some(img.to_rgb8()),
        Err(e) => {
            mv_core::logging::global().warn("anime", &format!("crops: could not open the page {}: {e}", path.display()));
            None
        }
    }
}

/// Every want grouped by page, so each page is decoded once.
fn by_page<T>(items: Vec<T>, at: impl Fn(&T) -> &SourceRef) -> BTreeMap<(String, usize), Vec<T>> {
    let mut out: BTreeMap<(String, usize), Vec<T>> = BTreeMap::new();
    for it in items {
        let a = at(&it);
        out.entry((a.chapter_id.clone(), a.page)).or_default().push(it);
    }
    out
}

/// The pixel box of a crop on a `w` × `h` page, or `None` when it would be too small.
fn rect(want: &Want, w: u32, h: u32) -> Option<[u32; 4]> {
    let (wf, hf) = (w as f32, h as f32);
    let px = |b: [f32; 4]| [b[0].clamp(0., 1.) * wf, b[1].clamp(0., 1.) * hf, b[2].clamp(0., 1.) * wf, b[3].clamp(0., 1.) * hf];
    let panel = want.panel.map(px).unwrap_or([0., 0., wf, hf]);
    let r = match (want.framing, want.face.map(px)) {
        (CropFraming::Face, Some(f)) => {
            let (cx, cy) = ((f[0] + f[2]) / 2., (f[1] + f[3]) / 2.);
            let mut side = 1.6 * (f[2] - f[0]).max(f[3] - f[1]);
            if side < GOOD_SIDE as f32 {
                side = (side * 1.6).min(GOOD_SIDE as f32);
            }
            let side = side.min(wf).min(hf);
            square(cx, cy, side, wf, hf)
        }
        (CropFraming::Bust, Some(f)) => {
            let (fw, fh) = (f[2] - f[0], f[3] - f[1]);
            let (mut bw, mut bh) = (3. * fw, 4. * fh);
            let short = bw.min(bh);
            if short < GOOD_SIDE as f32 && short > 0. {
                let k = (GOOD_SIDE as f32 / short).min(1.3);
                bw *= k;
                bh *= k;
            }
            let cx = (f[0] + f[2]) / 2.;
            // A little headroom above the hair: 0.6 of a face height (scaled with the box).
            let top = f[1] - 0.15 * bh;
            let r = [cx - bw / 2., top, cx + bw / 2., top + bh];
            // Clamped to the panel (a bust never shows the next panel), then to the page.
            [r[0].max(panel[0]), r[1].max(panel[1]), r[2].min(panel[2]), r[3].min(panel[3])]
        }
        _ => panel,
    };
    let r = [r[0].clamp(0., wf), r[1].clamp(0., hf), r[2].clamp(0., wf), r[3].clamp(0., hf)];
    let (x0, y0, x1, y1) = (r[0].floor() as u32, r[1].floor() as u32, r[2].ceil() as u32, r[3].ceil() as u32);
    let (x1, y1) = (x1.min(w), y1.min(h));
    (x1 > x0 && y1 > y0 && (x1 - x0).min(y1 - y0) >= MIN_SIDE).then_some([x0, y0, x1, y1])
}

/// A square of `side` around (cx, cy), shifted to stay on the page.
fn square(cx: f32, cy: f32, side: f32, w: f32, h: f32) -> [f32; 4] {
    let x0 = (cx - side / 2.).clamp(0., (w - side).max(0.));
    let y0 = (cy - side / 2.).clamp(0., (h - side).max(0.));
    [x0, y0, x0 + side, y0 + side]
}

/// Sharpness 0..1: the variance of a 3×3 Laplacian on the grey crop (at most 256 px), squashed.
fn sharpness(img: &RgbImage, r: [u32; 4]) -> f32 {
    let crop = image::imageops::crop_imm(img, r[0], r[1], r[2] - r[0], r[3] - r[1]).to_image();
    let (w, h) = crop.dimensions();
    let scale = 256. / w.max(h).max(1) as f32;
    let small = if scale < 1. {
        image::imageops::resize(&crop, ((w as f32 * scale) as u32).max(3), ((h as f32 * scale) as u32).max(3), FilterType::Triangle)
    } else {
        crop
    };
    let grey = image::DynamicImage::ImageRgb8(small).to_luma8();
    let (w, h) = grey.dimensions();
    if w < 3 || h < 3 {
        return 0.;
    }
    let at = |x: u32, y: u32| grey.get_pixel(x, y).0[0] as f32;
    let (mut sum, mut sq, mut n) = (0f64, 0f64, 0f64);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = 4. * at(x, y) - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1);
            sum += l as f64;
            sq += (l * l) as f64;
            n += 1.;
        }
    }
    let var = (sq / n - (sum / n).powi(2)).max(0.) as f32;
    var / (var + 100.)
}

/// From this share of [`not_a_picture`] on, a crop is never used without the user's approval.
pub const TEXT_LIMIT: f32 = 0.5;

/// How much a crop is lettering or nothing rather than a picture, 0..1: a speech bubble or
/// caption box is mostly paper white with ink strokes and almost no mid-tones; a blank or black
/// frame is mostly one dark tone. Sharp lettering scores high on sharpness, so without this a
/// bubble would beat a face.
pub fn not_a_picture(img: &RgbImage, r: [u32; 4]) -> f32 {
    let crop = image::imageops::crop_imm(img, r[0], r[1], r[2].saturating_sub(r[0]).max(1), r[3].saturating_sub(r[1]).max(1)).to_image();
    let (w, h) = crop.dimensions();
    let scale = 128. / w.max(h).max(1) as f32;
    let small = if scale < 1. { image::imageops::resize(&crop, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1), FilterType::Triangle) } else { crop };
    let (mut white, mut dark, mut mid, mut n) = (0f32, 0f32, 0f32, 0f32);
    for p in small.pixels() {
        let [r, g, b] = p.0;
        let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
        let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        if luma >= 225. && hi - lo < 30 {
            white += 1.;
        } else if luma < 50. {
            dark += 1.;
        } else {
            mid += 1.;
        }
        n += 1.;
    }
    if n == 0. {
        return 1.;
    }
    let (white, dark, mid) = (white / n, dark / n, mid / n);
    let lettering = ((white - 0.4) / 0.35).clamp(0., 1.) * ((0.35 - mid) / 0.25).clamp(0., 1.);
    let blank = ((dark - 0.6) / 0.3).clamp(0., 1.);
    lettering.max(blank)
}

/// Decodes each page once and scores every crop on it:
/// √(face area px) × sharpness × the framing's weight (whole panels without a face weigh less),
/// lowered by how much of it is lettering or blank ([`not_a_picture`]); such a crop also needs
/// the user's approval to be used, whatever the signals say.
fn measure(reads: &[(ChapterRef, ChapterRead)], wants: Vec<Want>) -> Vec<Measured> {
    let mut out = Vec::new();
    for ((chapter, page), wants) in by_page(wants, |w| &w.at) {
        let at = SourceRef { chapter_id: chapter, page, panel: None };
        let Some(img) = page_path(reads, &at).and_then(|p| load_page(&p)) else { continue };
        let (w, h) = img.dimensions();
        for mut want in wants {
            let Some(r) = rect(&want, w, h) else { continue };
            let text = not_a_picture(&img, r);
            if text >= TEXT_LIMIT {
                want.agreed = want.agreed.min(1);
            }
            let area = match want.face {
                Some(f) => ((f[2] - f[0]) * w as f32 * (f[3] - f[1]) * h as f32).max(1.),
                None => ((r[2] - r[0]) as f32 * (r[3] - r[1]) as f32 * 0.15).max(1.),
            };
            let weight = match want.framing {
                CropFraming::Face => 1.,
                CropFraming::Bust => 0.9,
                _ if want.face.is_some() => 0.7,
                _ => 0.6,
            };
            // A place's panel shows the place: the more the people take of it, the lower.
            let place = want.people.map(|p| 1. - 0.8 * (p / 0.2).clamp(0., 1.)).unwrap_or(1.);
            let score = area.sqrt() * (0.25 + 0.75 * sharpness(&img, r)) * weight * (1. - 0.95 * text) * place;
            out.push(Measured { want, rect: r, score, text });
        }
    }
    out
}

/// The `keep` best crops: agreed ones first, then by score with a penalty for repeating a page or
/// an (angle, framing) already chosen, so the set has variety.
fn choose(mut all: Vec<Measured>, keep: usize) -> Vec<Measured> {
    all.sort_by(|a, b| b.want.agreed.cmp(&a.want.agreed).then(b.score.total_cmp(&a.score)));
    let mut chosen: Vec<Measured> = Vec::new();
    while chosen.len() < keep && !all.is_empty() {
        let effective = |m: &Measured| {
            let same_page = chosen.iter().filter(|c| c.want.at.chapter_id == m.want.at.chapter_id && c.want.at.page == m.want.at.page).count();
            let same_kind = chosen.iter().filter(|c| c.want.angle == m.want.angle && c.want.framing == m.want.framing).count();
            m.score * 0.7f32.powi(same_page as i32) * 0.85f32.powi(same_kind as i32)
        };
        let best_agreed = all.iter().map(|m| m.want.agreed).max().unwrap_or(0);
        let Some(i) = all
            .iter()
            .enumerate()
            .filter(|(_, m)| m.want.agreed == best_agreed)
            .max_by(|a, b| effective(a.1).total_cmp(&effective(b.1)))
            .map(|(i, _)| i)
        else {
            break;
        };
        chosen.push(all.remove(i));
    }
    chosen
}

/// Encodes the chosen crops (pages decoded once more) and stores them as takes: each crop, whether
/// its picture was in the project before, and whether it may be a place's cover.
fn store_crops(store: &Store, owner: &str, reads: &[(ChapterRef, ChapterRead)], chosen: Vec<Measured>) -> Vec<(RefCrop, bool, bool)> {
    let mut made: Vec<(usize, (RefCrop, bool, bool))> = Vec::new();
    let indexed: Vec<(usize, Measured)> = chosen.into_iter().enumerate().collect();
    // Which pictures this owner had before this cut (asked before anything is stored: two
    // framings of one panel may encode to the same picture).
    let mut before: BTreeMap<String, bool> = BTreeMap::new();
    for ((chapter, page), items) in by_page(indexed, |(_, m)| &m.want.at) {
        let at = SourceRef { chapter_id: chapter, page, panel: None };
        let Some(img) = page_path(reads, &at).and_then(|p| load_page(&p)) else { continue };
        for (i, m) in items {
            match save_one(store, owner, &img, &m, &mut before) {
                Ok((c, existed)) => {
                    let cover = m.want.people.is_some_and(|p| p <= COVER_PEOPLE_MAX) && m.text < TEXT_LIMIT;
                    made.push((i, (c, existed, cover)))
                }
                Err(e) => mv_core::logging::global().warn("anime", &format!("crops: {e:#}")),
            }
        }
    }
    made.sort_by_key(|(i, _)| *i);
    let mut seen = BTreeSet::new();
    made.into_iter().map(|(_, c)| c).filter(|c| seen.insert(c.0.take.clone())).collect()
}

fn framing_word(f: CropFraming) -> &'static str {
    match f {
        CropFraming::Face => "face",
        CropFraming::Bust => "bust",
        _ => "panel",
    }
}

fn save_one(store: &Store, owner: &str, img: &RgbImage, m: &Measured, before: &mut BTreeMap<String, bool>) -> Result<(RefCrop, bool)> {
    let r = m.rect;
    let crop = image::imageops::crop_imm(img, r[0], r[1], r[2] - r[0], r[3] - r[1]).to_image();
    let (w, h) = crop.dimensions();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .encode_image(&crop)
        .map_err(|e| anyhow!("Could not encode a reference crop: {e}"))?;
    // The same box of the same page encodes to the same bytes, so the same take id.
    // (Another character may share the picture: only a cut for this owner counts.)
    let id = crate::hash::sha256_hex(&bytes);
    let existed = *before.entry(id.clone()).or_insert_with(|| store.take(&id).is_some_and(|t| t.target == Target::Crop { owner: owner.to_string() }));
    let at = &m.want.at;
    let note = format!("{} crop from {} page {} panel {}", framing_word(m.want.framing), at.chapter_id, at.page + 1, at.panel.map(|k| k + 1).unwrap_or(0));
    let take = store.add_take(
        &bytes,
        "jpg",
        TakeMeta { kind: TakeKind::Image, target: Target::Crop { owner: owner.to_string() }, source: TakeSource::MangaCrop(at.clone()), w, h, note, ..Default::default() },
    )?;
    let crop = RefCrop { file: take.file, take: take.id, source: at.clone(), angle: m.want.angle, framing: m.want.framing, score: m.score, approved: false, agreed: m.want.agreed };
    Ok((crop, existed))
}

/// Crops after a new cut: the user's approved ones stay (first), the new ones replace the old
/// suggestions; when the new cut found nothing, the old crops stay as they were.
pub fn merge_crops(old: &[RefCrop], new: Vec<RefCrop>) -> Vec<RefCrop> {
    if new.is_empty() {
        return old.to_vec();
    }
    let mut out: Vec<RefCrop> = old.iter().filter(|c| c.approved).cloned().collect();
    for c in new {
        if !out.iter().any(|o| o.take == c.take) {
            out.push(c);
        }
    }
    out
}

/// Crops after a local re-cut: every crop the user approved or uploaded stays exactly as it is
/// (first); a crop that is still a suggestion takes its new score; a picture that was cut before
/// and is no longer in the list was dropped by the user, so it stays out; the rest of the new cut
/// joins. When the re-cut found nothing, the crops stay as they were.
pub fn merge_cut(old: &[RefCrop], new: Vec<Cut>) -> Vec<RefCrop> {
    if new.is_empty() {
        return old.to_vec();
    }
    let mut out: Vec<RefCrop> = old.iter().filter(|c| c.approved).cloned().collect();
    for (c, existed) in new {
        let had = old.iter().any(|o| o.take == c.take);
        if out.iter().any(|o| o.take == c.take) || (existed && !had) {
            continue;
        }
        out.push(c);
    }
    out
}

/// The re-scoring of an existing bible is due: its crops were scored before [`SCORING_VERSION`].
pub fn scoring_outdated(style: &crate::model::Style) -> bool {
    style.extra.get(EXTRA_SCORING).and_then(Value::as_u64).unwrap_or(1) < SCORING_VERSION
}

/// The Read of every chapter of every episode of the project (each chapter once).
pub(crate) fn series_reads(store: &Store) -> Result<Vec<(ChapterRef, ChapterRead)>> {
    let series = store.series()?;
    let mut out: Vec<(ChapterRef, ChapterRead)> = Vec::new();
    for ep in &series.episodes {
        let episode = store.episode(&ep.id)?;
        for pick in &episode.chapters {
            if out.iter().any(|(c, _)| c.chapter_id == pick.chapter_id) {
                continue;
            }
            if let Some(read) = source::read(pick, &series.manga_title) {
                out.push((source::chapter_ref(pick, &series.manga_title), read));
            }
        }
    }
    Ok(out)
}

/// The Crops job for one character.
pub async fn run_job(cx: &JobCtx<'_>, character: &str) -> Result<()> {
    let bible = cx.store.bible()?;
    let c = bible.character(character).cloned().ok_or_else(|| anyhow!("That character is no longer in the bible."))?;
    let reads = series_reads(cx.store)?;
    if reads.is_empty() {
        anyhow::bail!("No chapter of this project has been read yet. Read the chapters in the Manga step first.");
    }
    (cx.progress)(0.1, &format!("Cutting reference pictures of {}…", c.name));
    let store = cx.store.clone();
    let id = c.id.clone();
    let new = tokio::task::spawn_blocking(move || pick_cut(&store, &c, &reads)).await.map_err(|_| anyhow!("Cutting the reference pictures stopped unexpectedly. Try again."))?;
    if cx.cancel.is_cancelled() {
        anyhow::bail!("cancelled");
    }
    let found = new.len();
    // Read, merge and save under the project's bible lock, so the Bible screen's commit or a
    // sheet job saving this character at the same moment is never lost.
    let cur = cx.store.with_bible(|| -> Result<_> {
        let mut cur = cx.store.bible()?.character(&id).cloned().ok_or_else(|| anyhow!("That character is no longer in the bible."))?;
        cur.crops = merge_cut(&cur.crops, new);
        cx.store.save_character(&cur)?;
        Ok(cur)
    })?;
    let ready = cur.crops.iter().filter(|c| usable(c)).count();
    (cx.progress)(1., &format!("{found} reference pictures of {}; {ready} ready to use, the others wait for your approval", cur.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{not_a_picture, TEXT_LIMIT};
    use image::{Rgb, RgbImage};

    fn whole(img: &RgbImage) -> [u32; 4] {
        [0, 0, img.width(), img.height()]
    }

    #[test]
    fn bubbles_captions_and_black_frames_are_not_pictures() {
        // A speech bubble: paper white with black letter strokes.
        let mut bubble = RgbImage::from_pixel(200, 200, Rgb([252, 252, 250]));
        for y in (40..160).step_by(12) {
            for x in 30..170 {
                if (x / 5) % 2 == 0 {
                    for dy in 0..4 {
                        bubble.put_pixel(x, y + dy, Rgb([15, 15, 15]));
                    }
                }
            }
        }
        assert!(not_a_picture(&bubble, whole(&bubble)) >= TEXT_LIMIT, "a bubble is lettering");
        // A black frame.
        let black = RgbImage::from_pixel(160, 160, Rgb([8, 8, 10]));
        assert!(not_a_picture(&black, whole(&black)) >= TEXT_LIMIT, "a black frame is nothing");
        // A coloured face: mid-tones everywhere.
        let mut face = RgbImage::from_pixel(160, 160, Rgb([235, 190, 160]));
        for y in 0..50 {
            for x in 0..160 {
                face.put_pixel(x, y, Rgb([90, 60, 40]));
            }
        }
        assert!(not_a_picture(&face, whole(&face)) < 0.1, "a face is a picture");
        // A bright scene with some white sky is still a picture.
        let mut sky = RgbImage::from_pixel(160, 160, Rgb([250, 250, 250]));
        for y in 70..160 {
            for x in 0..160 {
                sky.put_pixel(x, y, Rgb([120, 140, 90]));
            }
        }
        assert!(not_a_picture(&sky, whole(&sky)) < TEXT_LIMIT);
    }
}
