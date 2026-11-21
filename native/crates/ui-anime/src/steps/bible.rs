//! Step 3, Bible (design 9.6) [bible]: who and where the film is. W3-S2.
//!
//! One main action builds the bible from the read chapters (the engine's Bible job). Then five
//! tabs, each a calm grid of cards with a status: **Cast** (main sheet, name, signature colour,
//! the reference crops cut from manga panels), **Places** (master plate, the floor-plan editor,
//! camera stations with their background plates, light states), **Props** (prop sheet, grip
//! sheets, states, who holds them), **Style** (the style frame, the style line used word for word
//! in every prompt, things to avoid, the colour keys) and **Voices** (one voice per speaking
//! character: change, audition, lock).
//!
//! Opening a card shows it as a detail view: its pictures as a tidy gallery, the selected one
//! large with Approve / Reject / Lock / Upload your own / History and its latest options, and its
//! editable words. Every picture is made through the shared Generation panel on the right, filled
//! in by the engine's sheet specification (prompt and references with their stated jobs), which
//! the user can change; nothing is drawn unless the user asks. A picture whose inputs changed
//! since it was made is marked Outdated, with Update / Review one by one / Keep.
//!
//! "Approve the bible" (gate 1) is offered only when what the engine requires exists
//! (`bible::gate_missing`); until then the screen lists plainly what is still missing.
//!
//! Every edit is a function on the bible, applied to the screen's copy at once and, in the
//! background, to a fresh read of the files (one write at a time), so a job's new pictures are
//! never overwritten. User edits lock their field (design 5.4) so building again never undoes them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_anime::bible::{gate_missing, main_characters, speaks};
use mv_anime::queue::{self, Job, JobKind, JobState};
use mv_anime::sheets;
use mv_anime::{
    hash, source, Approval, BibleIndex, Card, CharVoice, Character, GenSpec, Outfit, Place, PlaceKind, Prop, RefCrop, Role, Series, Sheet, SheetKind, Status, Store,
    Take, TakeKind, TakeMeta, TakeSource, Target,
};
use mv_ui::panel::tab_bar;
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::{
    btn, confirm_dialog, empty_view, error_view, icon_button, icon_ph, inline_alert, menu_button, ph, ph_icon, popup_surface, progress_bar, size, skeleton_block, skeleton_line, spinner, tag, tip, toast,
    AlertKind, ButtonKind, MenuItem, Ph, Toast, Tokens, Tone, Weight,
};

use super::floorplan::{compass_word, opt_chip, PlaceOp, PlanEditor};
use super::shared::{cached_take, latest_round, TakeCache};
use super::{Step, StepCtx};
use crate::backends;
use crate::gen_panel::{self, busy_of, Busy, GenPanel};
use crate::Turns;
use crate::history::{history_button, History};
use crate::queue_strip::{web_held, Eta};
use crate::style::{COLUMN_HEAD_H, POLL_MS, REF_TILE, TILE_W};
use crate::widgets::{
    ago, card, card_marks, column_body, column_head, count, empty_gallery, error_block, fmt_duration, hover_actions, hue_for, info_line, inspector_row, label_caps, label_row, list_item, marker, now_secs,
    section, selectable, spec, status_chip, status_label, thumb, workspace, LineTone,
};

/// A cast card's picture (3:4, like the main design sheet).
const CAST_H: f32 = TILE_W * 4. / 3.;
/// A place or prop card's picture (16:9).
const TILE_H: f32 = TILE_W * 9. / 16.;
/// A sheet or plate tile in an open item's gallery (4:3, the picture fitted inside).
const SHEET_W: f32 = TILE_W * 0.75;
const SHEET_H: f32 = SHEET_W * 3. / 4.;
/// The selected picture, large.
const STILL_MAX_W: f32 = TILE_W * 3.;
const STILL_H: f32 = TILE_W * 1.5;
/// Reference crops in an open item, and the small ones on a cast card.
const CROP: f32 = REF_TILE * 1.25;
const CROP_MINI: f32 = REF_TILE / 2.;
/// Crops shown on a cast card before "+n".
const CROPS_ON_CARD: usize = 6;
/// List thumbnails in an open item's list column.
const LIST_THUMB: (f32, f32) = (48., 27.);
/// Width of the tab strip.
const TABS_W: f32 = TILE_W * 2.;
/// Missing items named in the approval notice.
const MISSING_SHOWN: usize = 3;
/// One missing item in the approval notice: at most this wide (the whole line is in its tooltip).
const GATE_ITEM_MAX_W: f32 = TILE_W * 1.3;
const GATE_ITEM_H: f32 = 24.;
/// Newest takes read to find a card's latest round of options.
const ROUND_LOOK: usize = 5;
/// Voice speeds offered.
const SPEEDS: [f32; 4] = [0.9, 1.0, 1.1, 1.2];
/// The narrator's row in the cast.
const NARRATOR: &str = "narrator";
/// The initials disc on a character card without a picture.
const INITIALS_DISC: f32 = TILE_W * 0.36;
/// The voice list: its width, its tallest (it scrolls beyond) and a row.
const VOICE_MENU_W: f32 = TILE_W * 1.2;
const VOICE_MENU_MAX_H: f32 = TILE_W * 1.4;
const VOICE_ROW_H: f32 = 30.;

// ---------------------------------------------------------------- pure parts

type Op = Arc<dyn Fn(&mut BibleIndex) + Send + Sync>;
type TextOp = Arc<dyn Fn(&mut BibleIndex, &str) + Send + Sync>;
type WriteLock = Arc<Mutex<()>>;

fn op(f: impl Fn(&mut BibleIndex) + Send + Sync + 'static) -> Op {
    Arc::new(f)
}

fn top(f: impl Fn(&mut BibleIndex, &str) + Send + Sync + 'static) -> TextOp {
    Arc::new(f)
}

fn char_op(id: &str, f: impl Fn(&mut Character) + Send + Sync + 'static) -> Op {
    let id = id.to_string();
    op(move |b| {
        if let Some(c) = b.characters.iter_mut().find(|c| c.id == id) {
            f(c)
        }
    })
}

/// A character's text field typed by the user: sets it and locks it (`ptr`).
fn look_field(id: &str, ptr: &'static str, get: fn(&mut Character) -> &mut String) -> TextOp {
    let id = id.to_string();
    top(move |b, text| {
        if let Some(c) = b.characters.iter_mut().find(|c| c.id == id) {
            *get(c) = text.to_string();
            c.locked.insert(ptr.into());
        }
    })
}

fn place_op(id: &str, f: impl Fn(&mut Place) + Send + Sync + 'static) -> Op {
    let id = id.to_string();
    op(move |b| {
        if let Some(p) = b.places.iter_mut().find(|p| p.id == id) {
            f(p)
        }
    })
}

fn prop_op(id: &str, f: impl Fn(&mut Prop) + Send + Sync + 'static) -> Op {
    let id = id.to_string();
    op(move |b| {
        if let Some(p) = b.props.iter_mut().find(|p| p.id == id) {
            f(p)
        }
    })
}

fn card_op(t: Target, f: impl Fn(&mut Card) + Send + Sync + 'static) -> Op {
    op(move |b| {
        if let Some(c) = card_entry(b, &t) {
            f(c)
        }
    })
}

/// A short stable key of a picture target (ids, never shown).
fn tkey(t: &Target) -> String {
    match t {
        Target::Sheet { owner, sheet } => format!("sheet/{owner}/{sheet}"),
        Target::PlaceSheet { place } => format!("place/{place}"),
        Target::Plate { place, station, state } => format!("plate/{place}/{station}/{state}"),
        Target::StyleFrame => "style".into(),
        other => format!("{other:?}"),
    }
}

/// An element id made of a prefix and a key.
fn eid(prefix: &str, key: &str) -> ElementId {
    ElementId::Name(format!("{prefix}-{key}").into())
}

/// The card a picture target fills, if it exists yet.
fn card_of<'a>(b: &'a BibleIndex, t: &Target) -> Option<&'a Card> {
    match t {
        Target::Sheet { owner, sheet } => {
            let kind = sheets::sheet_kind(sheet)?;
            let list = match b.character(owner) {
                Some(c) => &c.sheets,
                None => &b.prop(owner)?.sheets,
            };
            list.iter().find(|s| s.kind == kind).map(|s| &s.card)
        }
        Target::PlaceSheet { place } => b.place(place).map(|p| &p.sheet),
        Target::Plate { place, station, state } => b.place(place)?.plates.iter().find(|p| &p.station == station && &p.state == state).map(|p| &p.card),
        Target::StyleFrame => Some(&b.style.frame),
        _ => None,
    }
}

/// The card a picture target fills, made when it is new (a sheet or plate entry), like the
/// engine's own job does.
fn card_entry<'a>(b: &'a mut BibleIndex, t: &Target) -> Option<&'a mut Card> {
    match t {
        Target::Sheet { owner, sheet } => {
            let kind = sheets::sheet_kind(sheet)?;
            let list: &mut Vec<Sheet> = match b.characters.iter_mut().find(|c| &c.id == owner) {
                Some(c) => &mut c.sheets,
                None => &mut b.props.iter_mut().find(|p| &p.id == owner)?.sheets,
            };
            let i = match list.iter().position(|s| s.kind == kind) {
                Some(i) => i,
                None => {
                    list.push(Sheet { kind, card: Card::default() });
                    list.len() - 1
                }
            };
            Some(&mut list[i].card)
        }
        Target::PlaceSheet { place } => b.places.iter_mut().find(|p| &p.id == place).map(|p| &mut p.sheet),
        Target::Plate { place, station, state } => {
            let p = b.places.iter_mut().find(|p| &p.id == place)?;
            if !p.stations.iter().any(|s| &s.id == station) {
                return None;
            }
            let i = match p.plates.iter().position(|pl| &pl.station == station && &pl.state == state) {
                Some(i) => i,
                None => {
                    p.plates.push(mv_anime::Plate { station: station.clone(), state: state.clone(), card: Card::default() });
                    p.plates.len() - 1
                }
            };
            Some(&mut p.plates[i].card)
        }
        Target::StyleFrame => Some(&mut b.style.frame),
        _ => None,
    }
}

/// Every picture card of the bible with its target, in screen order.
fn all_cards(b: &BibleIndex) -> Vec<(Target, &Card)> {
    let mut out = Vec::new();
    for c in &b.characters {
        for s in c.sheets.iter().filter(|s| s.kind != SheetKind::Unknown) {
            out.push((Target::Sheet { owner: c.id.clone(), sheet: sheets::sheet_key(&s.kind) }, &s.card));
        }
    }
    for p in &b.places {
        out.push((Target::PlaceSheet { place: p.id.clone() }, &p.sheet));
        for pl in &p.plates {
            out.push((Target::Plate { place: p.id.clone(), station: pl.station.clone(), state: pl.state.clone() }, &pl.card));
        }
    }
    for p in &b.props {
        for s in p.sheets.iter().filter(|s| s.kind != SheetKind::Unknown) {
            out.push((Target::Sheet { owner: p.id.clone(), sheet: sheets::sheet_key(&s.kind) }, &s.card));
        }
    }
    out.push((Target::StyleFrame, &b.style.frame));
    out
}

/// The card has a picture the user approved.
fn approved(card: &Card) -> bool {
    card.status == Status::Approved && card.selected.is_some()
}

fn sheet_ok(sheets: &[Sheet], kind: &SheetKind) -> bool {
    sheets.iter().any(|s| &s.kind == kind && approved(&s.card))
}

/// The sheets of a main character that gate 1 asks for.
const NEEDED: [(SheetKind, &str); 3] = [(SheetKind::Master, "main design"), (SheetKind::Face, "face sheet"), (SheetKind::Turnaround, "turnaround")];

/// What a character still needs for the approval, in words.
fn char_needs(c: &Character, main: bool) -> Vec<&'static str> {
    if !main || c.status == Status::Rejected {
        return Vec::new();
    }
    NEEDED.iter().filter(|(k, _)| !sheet_ok(&c.sheets, k)).map(|(_, w)| *w).collect()
}

/// What a place still needs for the approval, in words.
fn place_needs(p: &Place) -> Vec<&'static str> {
    if p.status == Status::Rejected {
        return Vec::new();
    }
    let mut out = Vec::new();
    if !approved(&p.sheet) {
        out.push("environment master");
    }
    let overheads: Vec<&str> = p.stations.iter().filter(|s| s.overhead).map(|s| s.id.as_str()).collect();
    if overheads.is_empty() {
        out.push("overhead station");
    } else if !p.plates.iter().any(|pl| overheads.contains(&pl.station.as_str()) && approved(&pl.card)) {
        out.push("overhead plate");
    }
    out
}

fn any_review<'a>(cards: impl IntoIterator<Item = &'a Card>) -> bool {
    cards.into_iter().any(|c| c.status == Status::NeedsReview && c.selected.is_some())
}

/// The status a character card shows: left out, approved (by the user, or everything the
/// approval needs is approved), a picture waiting for review, else draft.
fn character_status(c: &Character, main: bool) -> Status {
    match c.status {
        Status::Rejected | Status::Approved => return c.status,
        _ => {}
    }
    if char_needs(c, main).is_empty() && (main || sheet_ok(&c.sheets, &SheetKind::Master)) {
        return Status::Approved;
    }
    if any_review(c.sheets.iter().map(|s| &s.card)) {
        Status::NeedsReview
    } else {
        Status::Draft
    }
}

fn place_status(p: &Place) -> Status {
    match p.status {
        Status::Rejected | Status::Approved => return p.status,
        _ => {}
    }
    if place_needs(p).is_empty() {
        return Status::Approved;
    }
    if any_review(std::iter::once(&p.sheet).chain(p.plates.iter().map(|pl| &pl.card))) {
        Status::NeedsReview
    } else {
        Status::Draft
    }
}

fn prop_status(p: &Prop) -> Status {
    match p.status {
        Status::Rejected | Status::Approved => return p.status,
        _ => {}
    }
    if sheet_ok(&p.sheets, &SheetKind::Master) {
        Status::Approved
    } else if any_review(p.sheets.iter().map(|s| &s.card)) {
        Status::NeedsReview
    } else {
        Status::Draft
    }
}

/// How many of the bible's items in the film are ready, of how many: every character, place and
/// prop not left out, and the style (its line written and its frame approved). The same count on
/// every tab.
fn bible_readiness(b: &BibleIndex, mains: &HashSet<String>) -> (usize, usize) {
    let chars: Vec<&Character> = b.characters.iter().filter(|c| c.status != Status::Rejected).collect();
    let places: Vec<&Place> = b.places.iter().filter(|p| p.status != Status::Rejected).collect();
    let props: Vec<&Prop> = b.props.iter().filter(|p| p.status != Status::Rejected).collect();
    let ready = chars.iter().filter(|c| character_status(c, mains.contains(&c.id)) == Status::Approved).count()
        + places.iter().filter(|p| place_status(p) == Status::Approved).count()
        + props.iter().filter(|p| prop_status(p) == Status::Approved).count()
        + usize::from(!b.style.line.trim().is_empty() && approved(&b.style.frame));
    (ready, chars.len() + places.len() + props.len() + 1)
}

/// The whole entity is locked (the root pointer): building again never changes it.
fn root_locked(locked: &std::collections::BTreeSet<String>) -> bool {
    locked.contains("")
}

/// A name, or `fallback` when the name is empty: plain words on screen (`UNNAMED_*`); the id
/// only where a line of the engine is matched (it names unnamed items by id).
fn name_or<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    if name.trim().is_empty() {
        fallback
    } else {
        name.trim()
    }
}

const UNNAMED_CHARACTER: &str = "Unnamed character";
const UNNAMED_PLACE: &str = "Unnamed place";
const UNNAMED_PROP: &str = "Unnamed prop";

fn role_word(r: Role) -> &'static str {
    match r {
        Role::Hero => "Hero",
        Role::Ally => "Ally",
        Role::Threat => "Threat",
        Role::Neutral | Role::Unknown => "Supporting",
    }
}

/// A sheet kind in words.
fn sheet_label(kind: &SheetKind, b: &BibleIndex, outfits: &[Outfit]) -> String {
    match kind {
        SheetKind::Master => "Main design".into(),
        SheetKind::Face => "Face sheet".into(),
        SheetKind::Turnaround => "Turnaround".into(),
        SheetKind::Expressions => "Expressions".into(),
        SheetKind::Back => "Back view".into(),
        SheetKind::ColourModel => "Colour model".into(),
        SheetKind::Outfit(id) => format!("Outfit · {}", outfits.iter().find(|o| &o.id == id).map(|o| name_or(&o.name, "Unnamed outfit").to_string()).unwrap_or_else(|| "Removed outfit".to_string())),
        SheetKind::Grip(id) => format!("Grip · {}", b.prop(id).map(|p| name_or(&p.name, UNNAMED_PROP).to_string()).unwrap_or_else(|| "Removed prop".to_string())),
        SheetKind::Unknown => "Newer sheet".into(),
    }
}

/// A state id in words ("night" → "Night").
fn state_name(p: &Place, state: &str) -> String {
    let raw = p.states.iter().find(|s| s.id == state).map(|s| if s.name.trim().is_empty() { s.id.clone() } else { s.name.trim().to_string() }).unwrap_or_else(|| state.to_string());
    let mut c = raw.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn station_name(p: &Place, station: &str) -> String {
    p.stations.iter().find(|s| s.id == station).map(|s| if s.name.trim().is_empty() { "Camera station".to_string() } else { s.name.trim().to_string() }).unwrap_or_else(|| "Camera station".to_string())
}

/// What a picture is, in words ("Jinwoo · Face sheet").
fn pic_title(b: &BibleIndex, t: &Target) -> String {
    match t {
        Target::Sheet { owner, sheet } => {
            let kind = sheets::sheet_kind(sheet).unwrap_or(SheetKind::Unknown);
            if let Some(c) = b.character(owner) {
                format!("{} · {}", name_or(&c.name, UNNAMED_CHARACTER), sheet_label(&kind, b, &c.outfits))
            } else if let Some(p) = b.prop(owner) {
                format!("{} · Prop sheet", name_or(&p.name, UNNAMED_PROP))
            } else {
                "Sheet".into()
            }
        }
        Target::PlaceSheet { place } => format!("{} · Environment master", b.place(place).map(|p| name_or(&p.name, UNNAMED_PLACE).to_string()).unwrap_or_else(|| "Removed place".to_string())),
        Target::Plate { place, station, state } => match b.place(place) {
            Some(p) => format!("{} · {} · {}", name_or(&p.name, UNNAMED_PLACE), station_name(p, station), state_name(p, state)),
            None => "Background plate".into(),
        },
        Target::StyleFrame => "Style frame".into(),
        _ => "Picture".into(),
    }
}

/// "#3A6FF2" → 0x3A6FF2 (also "#36F").
fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    let h: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        _ => return None,
    };
    u32::from_str_radix(&h, 16).ok()
}

/// "#3A6FF2" in the form the bible stores.
fn hex_text(v: u32) -> String {
    format!("#{:06X}", v & 0xFF_FFFF)
}

/// A height typed by the user ("1.75", "1,75 m", "175 cm") in metres.
fn parse_height(s: &str) -> Option<f32> {
    let t = s.trim().to_lowercase().replace(',', ".");
    let (num, cm) = if let Some(n) = t.strip_suffix("cm") {
        (n.trim().to_string(), true)
    } else {
        (t.trim_end_matches('m').trim().to_string(), false)
    };
    let v: f32 = num.parse().ok()?;
    let m = if cm || v > 3. { v / 100. } else { v };
    (m.is_finite() && (0.3..=3.).contains(&m)).then_some((m * 100.).round() / 100.)
}

/// What the "not ready to approve" notice says.
fn missing_lines(missing: &[String]) -> (Vec<String>, usize) {
    let shown: Vec<String> = missing.iter().take(MISSING_SHOWN).map(|s| s.trim().trim_end_matches('.').to_string()).collect();
    (shown, missing.len().saturating_sub(MISSING_SHOWN))
}

/// A hash of everything the approval covers.
fn bible_hash(b: &BibleIndex) -> String {
    let v = serde_json::json!({ "characters": b.characters, "places": b.places, "props": b.props, "style": b.style });
    hash::value_hash(&v)
}

/// Voices used by more than one row (voice id → the rows using it), in cast order.
fn shared_voices(rows: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (who, voice) in rows.iter().filter(|(_, v)| !v.trim().is_empty()) {
        match out.iter_mut().find(|(v, _)| v == voice) {
            Some((_, list)) => list.push(who.clone()),
            None => out.push((voice.clone(), vec![who.clone()])),
        }
    }
    out.retain(|(_, list)| list.len() > 1);
    out
}

fn speed_label(s: f32) -> String {
    let s = if s.is_finite() && s > 0. { s } else { 1. };
    if (s - 1.).abs() < 0.01 {
        "Normal".into()
    } else {
        format!("{s:.1}×")
    }
}

/// The picture types a user may upload.
fn picture_ext(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        _ => None,
    }
}

/// The largest picture a user may hand in, on a side and in bytes.
const UPLOAD_MAX_SIDE: u32 = 8192;
const UPLOAD_MAX_BYTES: u64 = 50_000_000;

/// Copies a picture into the project as an Upload take, once it is checked to be a real picture
/// of a sane size. The error is a plain sentence naming the file.
fn import_picture(store: &Store, path: &Path) -> anyhow::Result<Take> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if picture_ext(path).is_none() {
        bail!("{name} is not a PNG, JPG or WebP picture.");
    }
    let ext = mv_ui::thumbs::check_upload(path, UPLOAD_MAX_SIDE, UPLOAD_MAX_BYTES).map_err(|e| anyhow!("{name} could not be used: {e}."))?;
    let bytes = std::fs::read(path).map_err(|e| anyhow!("Could not read {name}: {e}"))?;
    let meta = TakeMeta { kind: TakeKind::Image, target: Target::Upload, source: TakeSource::Upload { name }, ..Default::default() };
    store.add_take(&bytes, ext, meta)
}

/// Imports every usable picture of `paths` (the others are logged and counted).
fn import_pictures(store: &Store, paths: &[PathBuf]) -> (Vec<Take>, Vec<String>) {
    let mut added = Vec::new();
    let mut refused = Vec::new();
    for p in paths {
        match import_picture(store, p) {
            Ok(t) => added.push(t),
            Err(e) => {
                mv_core::logging::global().warn("anime", &format!("bible: upload refused: {e:#}"));
                refused.push(format!("{e}"));
            }
        }
    }
    (added, refused)
}

/// " 1 file was not a usable picture." (nothing when every file was).
fn refused_note(refused: usize) -> String {
    match refused {
        0 => String::new(),
        1 => " 1 file was not a usable picture.".into(),
        n => format!(" {n} files were not usable pictures."),
    }
}

/// A picture's settings were saved in the background: a job for it added after the save began
/// (`since`) that already started may have read the old settings, so it starts over with the
/// saved ones ("Generate" right after a change always uses it).
fn restart_if_started(project: &str, target: &Target, since: u64) {
    if let Busy::Working { id, .. } = busy_of(&queue::snapshot().jobs, project, target) {
        if id > since {
            queue::pause(id);
            queue::resume(id);
        }
    }
}

/// The item a picture belongs to was left out of the film ("Left out"): it is never counted as
/// outdated nor drawn again by "Update all".
fn left_out(b: &BibleIndex, t: &Target) -> bool {
    match t {
        Target::Sheet { owner, .. } => b.character(owner).map(|c| c.status).or_else(|| b.prop(owner).map(|p| p.status)) == Some(Status::Rejected),
        Target::PlaceSheet { place } | Target::Plate { place, .. } => b.place(place).is_some_and(|p| p.status == Status::Rejected),
        _ => false,
    }
}

/// A character's sheets that have a picture waiting for review (what "Approve all drawn sheets"
/// approves).
fn sheets_to_approve(c: &Character) -> Vec<Target> {
    c.sheets
        .iter()
        .filter(|s| s.kind != SheetKind::Unknown && s.card.selected.is_some() && s.card.status == Status::NeedsReview)
        .map(|s| Target::Sheet { owner: c.id.clone(), sheet: sheets::sheet_key(&s.kind) })
        .collect()
}

/// The time line of a confirm dialog.
fn time_note(n: usize, mean: Option<f32>) -> String {
    match mean {
        Some(m) if m.is_finite() && m > 0. => format!("About {} in all.", fmt_duration((m * n as f32).round() as u64)),
        _ => "The time is shown once the first picture is done.".into(),
    }
}

/// Applies `f` to a fresh read of the bible and saves exactly the files it changed, all under the
/// project's one bible lock, so a sheet job recording a picture at the same moment neither loses
/// this change nor has its picture dropped by it. Lock order: screen lock, bible lock, the
/// entity's (or cast's) file lock.
fn commit(store: &Store, lock: &WriteLock, f: &Op) -> anyhow::Result<()> {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    store.with_bible(|| commit_now(store, f))
}

/// [`commit`]'s read and saves, with the locks held.
fn commit_now(store: &Store, f: &Op) -> anyhow::Result<()> {
    let before = store.bible()?;
    let mut after = before.clone();
    f(&mut after);
    for c in &after.characters {
        if before.character(&c.id) != Some(c) {
            store.save_character(c)?;
        }
    }
    for p in &after.places {
        if before.place(&p.id) != Some(p) {
            store.save_place(p)?;
        }
    }
    for p in &after.props {
        if before.prop(&p.id) != Some(p) {
            store.save_prop(p)?;
        }
    }
    if after.style != before.style {
        store.save_style(&after.style)?;
    }
    if after.cast != before.cast {
        // The cast is written under its file's own lock, applied to the cast as it is then (Sound
        // fills and changes it too), so neither screen loses the other's change.
        store.update_cast(|cast| {
            let mut fresh = before.clone();
            fresh.cast = std::mem::take(cast);
            f(&mut fresh);
            *cast = fresh.cast;
        })?;
    }
    Ok(())
}

fn failure<T>(res: anyhow::Result<anyhow::Result<T>>) -> Option<String> {
    match res {
        Ok(Ok(_)) => None,
        Ok(Err(e)) | Err(e) => Some(format!("{e:#}")),
    }
}

// ---------------------------------------------------------------- jobs (pure)

/// Where the latest job of a kind stands.
#[derive(Clone, Debug, PartialEq)]
enum Run {
    Idle,
    Waiting { id: u64, paused: bool },
    Running { id: u64, progress: f32, line: String },
    Failed { id: u64, error: String },
}

impl Run {
    fn active(&self) -> bool {
        matches!(self, Run::Waiting { .. } | Run::Running { .. })
    }
}

fn job_run(jobs: &[Job], project: &str, want: impl Fn(&JobKind) -> bool) -> Run {
    let Some(j) = jobs.iter().filter(|j| j.project == project && want(&j.kind) && !(j.state == JobState::Failed && j.hidden)).max_by_key(|j| j.id) else { return Run::Idle };
    match j.state {
        JobState::Queued => Run::Waiting { id: j.id, paused: false },
        JobState::Paused => Run::Waiting { id: j.id, paused: true },
        JobState::Running => Run::Running { id: j.id, progress: j.progress, line: j.line.clone() },
        JobState::Failed => Run::Failed { id: j.id, error: j.error.clone().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| j.line.clone()) },
        JobState::Done => Run::Idle,
    }
}

/// A picture job of one entity (a character's or prop's sheet, a place's master or plates).
fn draws_for(kind: &JobKind, owner: &str) -> bool {
    match kind {
        JobKind::Sheet { target: Target::Sheet { owner: o, .. }, .. } => o == owner,
        JobKind::Sheet { target: Target::PlaceSheet { place } | Target::Plate { place, .. }, .. } => place == owner,
        JobKind::Crops { character } => character == owner,
        _ => false,
    }
}

// ---------------------------------------------------------------- reading the bible

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Cast,
    Places,
    Props,
    Style,
    Voices,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Cast, Tab::Places, Tab::Props, Tab::Style, Tab::Voices];

    fn label(self) -> &'static str {
        match self {
            Tab::Cast => "Cast",
            Tab::Places => "Places",
            Tab::Props => "Props",
            Tab::Style => "Style",
            Tab::Voices => "Voices",
        }
    }

    fn icon(self) -> Ph {
        match self {
            Tab::Cast => ph::USERS,
            Tab::Places => ph::MOUNTAINS,
            Tab::Props => ph::CUBE,
            Tab::Style => ph::PALETTE,
            Tab::Voices => ph::MICROPHONE,
        }
    }
}

/// The item opened from a tab's grid.
#[derive(Clone, Debug, PartialEq)]
enum Open {
    Character(String),
    Place(String),
    Prop(String),
}

/// Where a picture lives on the screen: its tab and item.
fn home_of(b: &BibleIndex, t: &Target) -> Option<(Tab, Option<Open>)> {
    match t {
        Target::Sheet { owner, .. } if b.character(owner).is_some() => Some((Tab::Cast, Some(Open::Character(owner.clone())))),
        Target::Sheet { owner, .. } if b.prop(owner).is_some() => Some((Tab::Props, Some(Open::Prop(owner.clone())))),
        Target::PlaceSheet { place } | Target::Plate { place, .. } if b.place(place).is_some() => Some((Tab::Places, Some(Open::Place(place.clone())))),
        Target::StyleFrame => Some((Tab::Style, None)),
        _ => None,
    }
}

/// The picture shown first when an item opens.
fn first_pic(open: &Open) -> Target {
    match open {
        Open::Character(id) | Open::Prop(id) => Target::Sheet { owner: id.clone(), sheet: "master".into() },
        Open::Place(id) => Target::PlaceSheet { place: id.clone() },
    }
}

struct Data {
    series: Series,
    bible: BibleIndex,
    /// The episode's chapters and how many have a Read (only looked at while the bible is empty).
    chapters: usize,
    read: usize,
    /// Selected take id → its file.
    files: HashMap<String, PathBuf>,
    /// Audition take id → (voice id, file).
    auditions: HashMap<String, (String, PathBuf)>,
    /// The voices on this PC (the same answer Sound reads).
    installed: backends::InstalledVoices,
    /// What still blocks gate 1.
    gate: Vec<String>,
    /// Hash of what an approval covers now.
    hash: String,
    /// Card covers for items without a drawn picture, found while reading (never on the UI
    /// thread): `c:<id>` a character (a crop the user accepted or uploaded, else a face from
    /// Studio's character data), `p:<id>` a place. None: the initials or neutral tile.
    covers: HashMap<String, PathBuf>,
}

impl Data {
    fn empty(&self) -> bool {
        self.bible.characters.is_empty() && self.bible.places.is_empty() && self.bible.props.is_empty()
    }

    fn mains(&self) -> HashSet<String> {
        main_characters(&self.bible).into_iter().map(|c| c.id.clone()).collect()
    }

    /// Changed since it was approved (or never approved).
    fn approval(&self) -> Option<(&Approval, bool)> {
        self.series.bible_approved.as_ref().map(|a| (a, a.hash == self.hash))
    }
}

enum Load {
    Loading,
    Ready(Box<Data>),
    Failed(String),
}

fn take_path(store: &Store, cache: &TakeCache, id: &str) -> Option<PathBuf> {
    match cached_take(store, cache, id) {
        Some(t) => Some(store.path(&t.file)).filter(|p| p.is_file()),
        None => store.take_file(id),
    }
}

/// Reads the bible and what the screen shows about it. Blocking.
fn load(store: &Store, ep: &str, cache: &TakeCache) -> anyhow::Result<Data> {
    let series = store.series()?;
    let installed = backends::installed_voices();
    // Crops scored by an older version are cut again after the bible is shown (`tidy_crops`).
    let bible = store.bible()?;
    #[cfg(feature = "sound")]
    let bible = fill_voices(store, &series, bible, &installed);
    let (mut chapters, mut read) = (0, 0);
    if bible.characters.is_empty() && bible.places.is_empty() && bible.props.is_empty() {
        let episode = store.episode(ep)?;
        chapters = episode.chapters.len();
        read = episode.chapters.iter().filter(|p| source::read_progress(p, &series.manga_title).0 > 0).count();
    }
    let mut files = HashMap::new();
    for (_, c) in all_cards(&bible) {
        if let Some(id) = c.selected.as_deref() {
            if let Some(p) = take_path(store, cache, id) {
                files.insert(id.to_string(), p);
            }
        }
    }
    let mut auditions = HashMap::new();
    for v in std::iter::once(&bible.cast.narrator).chain(bible.cast.voices.iter()) {
        for id in &v.auditions {
            if let Some(t) = cached_take(store, cache, id) {
                if let TakeSource::Voice { voice, .. } = &t.source {
                    let p = store.path(&t.file);
                    if p.is_file() {
                        auditions.insert(id.clone(), (voice.clone(), p));
                    }
                }
            }
        }
    }
    let gate = gate_missing(&bible);
    let hash = bible_hash(&bible);
    // Covers by the engine's strict rules: an unreviewed panel crop is never a cover.
    let mut covers = HashMap::new();
    let brain = source::brain(&series.manga_id, &series.manga_title);
    for c in &bible.characters {
        if let Some(p) = mv_anime::crops::character_cover(store, c, &brain) {
            covers.insert(format!("c:{}", c.id), p);
        }
    }
    for p in &bible.places {
        if let Some(f) = mv_anime::crops::place_cover_file(store, p) {
            covers.insert(format!("p:{}", p.id), f);
        }
    }
    Ok(Data { series, bible, chapters, read, files, auditions, installed, gate, hash, covers })
}

/// Gives a voice to the narrator and every speaking character who has none, the way Sound does
/// (saved once to the cast, picked for the user and not locked), so Bible › Voices and Sound
/// always show the same cast. Nothing is written when there is nothing to fill.
#[cfg(feature = "sound")]
fn fill_voices(store: &Store, series: &Series, mut bible: BibleIndex, installed: &backends::InstalledVoices) -> BibleIndex {
    if installed.ids.is_empty() {
        return bible;
    }
    let speakers: HashSet<String> = bible.characters.iter().filter(|c| c.status != Status::Rejected && speaks(c)).map(|c| c.id.clone()).collect();
    if !super::sound::needs_casting(&bible.cast, &bible.characters, &speakers) {
        return bible;
    }
    let studio = mv_recap::series::VoiceCast::load(&series.manga_id);
    if super::sound::fill_cast(&bible.cast, &bible.characters, &studio, &installed.ids) == bible.cast {
        return bible;
    }
    match store.update_cast(|c| *c = super::sound::fill_cast(c, &bible.characters, &studio, &installed.ids)) {
        Ok(cast) => bible.cast = cast,
        Err(e) => mv_core::logging::global().warn("anime", &format!("bible: the voices could not be filled in: {e:#}")),
    }
    bible
}

/// The pictures whose inputs changed since they were made (design 5.5). Blocking.
fn scan_outdated(store: &Store) -> HashSet<String> {
    let Ok(bible) = store.bible() else { return HashSet::new() };
    // Items left out of the film are never outdated: nothing redraws them.
    all_cards(&bible).into_iter().filter(|(t, c)| !left_out(&bible, t) && sheets::outdated_in(c, &bible, store)).map(|(t, _)| tkey(&t)).collect()
}

/// The open picture as the engine would make it now, and its latest options.
#[derive(Clone)]
struct Prepared {
    target: Target,
    spec: GenSpec,
    /// The latest round of pictures (take id, file), oldest first.
    round: Vec<(String, Option<PathBuf>)>,
}

/// Builds the open picture's spec and options. Blocking.
fn prepare(store: &Store, cache: &TakeCache, target: &Target) -> anyhow::Result<Prepared> {
    let bible = store.bible()?;
    let card = card_of(&bible, target).cloned().unwrap_or_default();
    let mut spec = sheets::current_spec(store, target)?;
    // The number of pictures is the panel's own setting; the request never carries it.
    if card.gen.variants > 0 {
        spec.variants = card.gen.variants;
    }
    let mut recent: Vec<Take> = card.takes.iter().rev().take(ROUND_LOOK).filter_map(|id| cached_take(store, cache, id)).collect();
    recent.reverse();
    let pairs: Vec<(String, Option<String>)> = recent.iter().map(|t| (t.id.clone(), t.batch.clone())).collect();
    let ids = latest_round(&pairs);
    let round = recent.iter().filter(|t| ids.contains(&t.id)).map(|t| (t.id.clone(), Some(store.path(&t.file)).filter(|p| p.is_file()))).collect();
    Ok(Prepared { target: target.clone(), spec, round })
}

// ---------------------------------------------------------------- the screen

/// The open text box: what it edits and how its words are applied.
struct Editing {
    key: String,
    input: Entity<TextareaState>,
    multi: bool,
    initial: String,
    apply: TextOp,
    check: fn(&str) -> Option<&'static str>,
    _sub: Subscription,
}

fn no_check(_: &str) -> Option<&'static str> {
    None
}

pub struct BibleStep {
    ctx: StepCtx,
    store: Store,
    takes: TakeCache,
    writes: WriteLock,
    turns: Turns,
    data: Load,
    refresh_error: Option<String>,
    loading: bool,
    again: bool,
    /// The project changed while this step was not drawn: read it again when it is next drawn.
    stale: bool,
    /// (store, queue, site) versions last seen.
    seen: (u64, u64, u64),
    playing: Option<PathBuf>,
    jobs: Vec<Job>,
    queue_paused: bool,
    web_held: bool,
    tab: Tab,
    open: Option<Open>,
    /// The picture the panel, viewer and history show.
    pic: Option<Target>,
    prepared: Option<Prepared>,
    preparing: bool,
    prepare_again: bool,
    outdated: HashSet<String>,
    scanning: bool,
    scan_again: bool,
    panel: Option<Entity<GenPanel>>,
    panel_key: Option<String>,
    history: Option<Entity<History>>,
    history_key: Option<String>,
    history_open: bool,
    plan: Option<Entity<PlanEditor>>,
    editing: Option<Editing>,
    /// Narrow window: the Generation panel drawer is open.
    drawer: bool,
    /// "Review one by one" of the outdated pictures.
    reviewing: bool,
    approving: bool,
    /// "Cut the crops again" is running.
    recutting: bool,
    /// Crops scored by an older version are being cut again (once, after the first read).
    tidying: bool,
    tidied: bool,
    body_scroll: ScrollHandle,
    detail_scroll: ScrollHandle,
    /// The voice list open under this cast row (character id or the narrator).
    voice_pick: Option<String>,
    /// The open voice list takes the keyboard, so Escape closes it without picking.
    voice_focus: FocusHandle,
    /// The approval notice lists every missing item.
    gate_open: bool,
    /// The picture last approved or taken back to review, and when (a double click is ignored).
    last_status_click: Option<(String, std::time::Instant)>,
}

pub fn open(ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    backends::check_site_once();
    cx.new(|cx| {
        let store = Store::open(&ctx.project);
        let mut s = BibleStep {
            ctx,
            store,
            takes: Arc::new(Mutex::new(HashMap::new())),
            writes: Arc::new(Mutex::new(())),
            turns: Turns::default(),
            data: Load::Loading,
            refresh_error: None,
            loading: false,
            again: false,
            stale: false,
            seen: (mv_anime::store::version(), queue::version(), backends::site_version()),
            playing: ui_studio::audio::now_playing(),
            jobs: Vec::new(),
            queue_paused: false,
            web_held: false,
            tab: Tab::Cast,
            open: None,
            pic: None,
            prepared: None,
            preparing: false,
            prepare_again: false,
            outdated: HashSet::new(),
            scanning: false,
            scan_again: false,
            panel: None,
            panel_key: None,
            history: None,
            history_key: None,
            history_open: false,
            plan: None,
            editing: None,
            drawer: false,
            reviewing: false,
            approving: false,
            recutting: false,
            tidying: false,
            tidied: false,
            body_scroll: ScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            voice_pick: None,
            voice_focus: cx.focus_handle(),
            gate_open: false,
            last_status_click: None,
        };
        s.refresh_jobs();
        s.reload(cx);
        s.poll(window, cx);
        s
    })
    .into()
}

impl BibleStep {
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(POLL_MS)).await;
            if this.update(cx, |s, cx| s.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let playing = ui_studio::audio::now_playing();
        if playing != self.playing {
            self.playing = playing;
            cx.notify();
        }
        let v = (mv_anime::store::version(), queue::version(), backends::site_version());
        if v == self.seen {
            return;
        }
        let (files, jobs) = (v.0 != self.seen.0, v.1 != self.seen.1);
        self.seen = v;
        if jobs {
            self.refresh_jobs();
        }
        if files {
            // Read again when next drawn: a step that is not on screen is never drawn, so it
            // does not re-read and re-scan the bible on every picture a running job writes.
            self.stale = true;
        }
        cx.notify();
    }

    fn refresh_jobs(&mut self) {
        let q = queue::snapshot();
        self.queue_paused = q.paused || q.interrupted;
        self.web_held = web_held(&q);
        let project = self.ctx.project.clone();
        self.jobs = q.jobs.into_iter().filter(|j| j.project == project).collect();
    }

    /// Reads the bible again in the background (a second call while reading runs once more after).
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.again = true;
            return;
        }
        self.loading = true;
        let (store, ep, takes) = (self.store.clone(), self.ctx.episode.clone(), self.takes.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || load(&store, &ep, &takes)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match res {
                    Ok(Ok(d)) => {
                        this.data = Load::Ready(Box::new(d));
                        this.refresh_error = None;
                        if !std::mem::replace(&mut this.tidied, true) {
                            this.tidy_crops(cx);
                        }
                    }
                    Ok(Err(e)) | Err(e) => {
                        let msg = format!("{e:#}");
                        mv_core::logging::global().warn("anime", &format!("bible: could not read the bible: {msg}"));
                        if matches!(this.data, Load::Ready(_)) {
                            this.refresh_error = Some(msg);
                        } else {
                            this.data = Load::Failed(msg);
                        }
                    }
                }
                this.after_load(cx);
                if std::mem::take(&mut this.again) {
                    this.reload(cx);
                }
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    /// Once the bible is on screen: crops scored by an older version are cut again in the
    /// background (locally, no AI), with one quiet line meanwhile; the bible is read again after.
    fn tidy_crops(&mut self, cx: &mut Context<Self>) {
        self.tidying = true;
        let store = self.store.clone();
        let task = mv_ui::bg::blocking(move || mv_anime::bible::upgrade_crops(&store));
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.tidying = false;
                match res {
                    Ok(Ok(true)) => this.reload(cx),
                    Ok(Ok(false)) => {}
                    Ok(Err(e)) | Err(e) => mv_core::logging::global().warn("anime", &format!("bible: crops not upgraded: {e:#}")),
                }
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    fn after_load(&mut self, cx: &mut Context<Self>) {
        let (gone, pic_gone) = match &self.data {
            Load::Ready(d) => (
                match &self.open {
                    Some(Open::Character(id)) => d.bible.character(id).is_none(),
                    Some(Open::Place(id)) => d.bible.place(id).is_none(),
                    Some(Open::Prop(id)) => d.bible.prop(id).is_none(),
                    None => false,
                },
                self.pic.as_ref().is_some_and(|t| engine_target_gone(&d.bible, t)),
            ),
            _ => (false, false),
        };
        if gone {
            self.close_item(cx);
        } else if pic_gone {
            self.pic = self.open.as_ref().map(first_pic).or(if self.tab == Tab::Style { Some(Target::StyleFrame) } else { None });
            self.prepared = None;
        }
        self.scan(cx);
        self.refresh_pic(cx);
        self.sync_plan(cx);
    }

    /// Looks for outdated pictures in the background.
    fn scan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            self.scan_again = true;
            return;
        }
        self.scanning = true;
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || scan_outdated(&store)).await;
            let _ = this.update(cx, |this, cx| {
                this.scanning = false;
                if let Ok(set) = res {
                    this.outdated = set;
                }
                if std::mem::take(&mut this.scan_again) {
                    this.scan(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Builds the open picture's spec and options in the background, then fills the panel.
    fn refresh_pic(&mut self, cx: &mut Context<Self>) {
        self.sync_panel(cx);
        let Some(target) = self.pic.clone() else {
            self.prepared = None;
            return;
        };
        if self.preparing {
            self.prepare_again = true;
            return;
        }
        self.preparing = true;
        let (store, takes) = (self.store.clone(), self.takes.clone());
        cx.spawn(async move |this, cx| {
            let res = mv_ui::bg::blocking(move || prepare(&store, &takes, &target)).await;
            let _ = this.update(cx, |this, cx| {
                this.preparing = false;
                match res {
                    Ok(Ok(p)) if Some(&p.target) == this.pic.as_ref() => this.prepared = Some(p),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) | Err(e) => mv_core::logging::global().warn("anime", &format!("bible: could not prepare the picture: {e:#}")),
                }
                if std::mem::take(&mut this.prepare_again) {
                    this.refresh_pic(cx);
                } else {
                    this.sync_panel(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts the open picture into the Generation panel and the history (new entities when another
    /// picture opens, so their titles follow it).
    fn sync_panel(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Load::Ready(d)) = (self.pic.clone(), &self.data) else {
            self.panel = None;
            self.panel_key = None;
            self.history = None;
            self.history_key = None;
            return;
        };
        let key = tkey(&target);
        let title = pic_title(&d.bible, &target);
        let card = card_of(&d.bible, &target).cloned().unwrap_or_default();
        let project = self.ctx.project.clone();
        // History.
        if self.history_key.as_deref() == Some(key.as_str()) {
            if let Some(h) = &self.history {
                h.update(cx, |h, cx| {
                    h.set(card.takes.clone(), card.selected.clone());
                    cx.notify();
                });
            }
        } else {
            let (pick, close) = (cx.entity().downgrade(), cx.entity().downgrade());
            let on = target.clone();
            let history = cx.new(|_| {
                History::new(&project, card.takes.clone(), card.selected.clone())
                    .title(title.clone())
                    .on_select(move |take, _w, cx| {
                        let _ = pick.update(cx, |this, cx| this.choose(on.clone(), take, false, cx));
                    })
                    .on_close(move |_w, cx| {
                        let _ = close.update(cx, |this, cx| {
                            this.history_open = false;
                            cx.notify();
                        });
                    })
            });
            self.history = Some(history);
            self.history_key = Some(key.clone());
            self.history_open = false;
        }
        // Panel: made once the engine's spec for this picture is known.
        let Some(prepared) = self.prepared.clone().filter(|p| p.target == target) else {
            if self.panel_key.as_deref() != Some(key.as_str()) {
                self.panel = None;
                self.panel_key = None;
            }
            return;
        };
        if self.panel_key.as_deref() == Some(key.as_str()) {
            if let Some(p) = &self.panel {
                p.update(cx, |p, cx| {
                    p.set(prepared.spec.clone(), None);
                    cx.notify();
                });
            }
            return;
        }
        let weak = cx.entity().downgrade();
        let on = target.clone();
        let fixed_shape = matches!(&target, Target::Sheet { owner, .. } if d.bible.character(owner).is_some());
        let panel = cx.new(|_| {
            let p = GenPanel::new(target.clone(), prepared.spec.clone()).project(&project).title(title.clone()).label(title.clone()).on_change(move |spec, _w, cx| {
                let _ = weak.update(cx, |this, cx| this.save_spec(on.clone(), spec, cx));
            });
            if fixed_shape {
                p.lock_aspect()
            } else {
                p
            }
        });
        self.panel = Some(panel);
        self.panel_key = Some(key);
    }

    /// Hands the open place to the floor-plan editor (made when a place opens).
    fn sync_plan(&mut self, cx: &mut Context<Self>) {
        let (Some(Open::Place(id)), Load::Ready(d)) = (&self.open, &self.data) else {
            self.plan = None;
            return;
        };
        let Some(place) = d.bible.place(id).cloned() else { return };
        let ro = self.place_read_only(&place);
        if let Some(plan) = &self.plan {
            plan.update(cx, |p, cx| p.set_place(&place, ro, cx));
        }
    }

    fn ensure_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Open::Place(id)), Load::Ready(d)) = (&self.open, &self.data) else {
            self.plan = None;
            return;
        };
        if self.plan.is_some() {
            return;
        }
        let Some(place) = d.bible.place(id).cloned() else { return };
        let ro = self.place_read_only(&place);
        let weak = cx.entity().downgrade();
        let pid = place.id.clone();
        let plan = cx.new(|cx| {
            PlanEditor::new(
                &place,
                ro,
                move |what, op: PlaceOp, _w, cx| {
                    let pid = pid.clone();
                    let _ = weak.update(cx, |this, cx| this.change(what, place_op(&pid, move |p| op(p)), cx));
                },
                window,
                cx,
            )
        });
        self.plan = Some(plan);
    }

    fn place_read_only(&self, p: &Place) -> bool {
        root_locked(&p.locked) || self.bible_run().active()
    }

    fn bible_run(&self) -> Run {
        job_run(&self.jobs, &self.ctx.project, |k| matches!(k, JobKind::Bible { .. }))
    }

    fn pic_busy(&self, t: &Target) -> Busy {
        busy_of(&self.jobs, &self.ctx.project, t)
    }

    /// The first job of an item that draws or cuts now (for its card's corner).
    fn item_busy(&self, owner: &str) -> Option<bool> {
        let mut waiting = false;
        for j in self.jobs.iter().filter(|j| draws_for(&j.kind, owner)) {
            match j.state {
                JobState::Running => return Some(true),
                JobState::Queued | JobState::Paused => waiting = true,
                _ => {}
            }
        }
        waiting.then_some(false)
    }

    // ------------------------------------------------------------ navigation

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.commit_text(cx);
        self.tab = tab;
        self.open = None;
        self.plan = None;
        self.reviewing = false;
        self.drawer = false;
        self.pic = if tab == Tab::Style { Some(Target::StyleFrame) } else { None };
        self.prepared = None;
        self.refresh_pic(cx);
        cx.notify();
    }

    fn open_item(&mut self, open: Open, cx: &mut Context<Self>) {
        self.commit_text(cx);
        if self.open.as_ref() != Some(&open) {
            self.plan = None;
            self.pic = Some(first_pic(&open));
            self.open = Some(open);
            self.prepared = None;
            self.history_open = false;
            self.detail_scroll.set_offset(point(px(0.), px(0.)));
            self.refresh_pic(cx);
        }
        cx.notify();
    }

    fn close_item(&mut self, cx: &mut Context<Self>) {
        self.commit_text(cx);
        self.open = None;
        self.plan = None;
        self.reviewing = false;
        self.drawer = false;
        self.pic = if self.tab == Tab::Style { Some(Target::StyleFrame) } else { None };
        self.prepared = None;
        self.refresh_pic(cx);
        cx.notify();
    }

    fn select_pic(&mut self, t: Target, cx: &mut Context<Self>) {
        if self.pic.as_ref() != Some(&t) {
            self.pic = Some(t);
            self.history_open = false;
            self.refresh_pic(cx);
        }
        self.drawer = true;
        cx.notify();
    }

    /// Opens the tab and item a picture belongs to, with that picture selected.
    fn go_to(&mut self, t: &Target, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        let Some((tab, open)) = home_of(&d.bible, t) else { return };
        self.commit_text(cx);
        if self.tab != tab || self.open != open {
            self.tab = tab;
            self.plan = None;
            self.open = open;
            self.detail_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.pic = Some(t.clone());
        self.prepared = None;
        self.history_open = false;
        self.refresh_pic(cx);
        cx.notify();
    }

    // ------------------------------------------------------------ changes

    /// Applies `f` here at once and to the files in the background (one write at a time).
    fn change(&mut self, what: &'static str, f: Op, cx: &mut Context<Self>) {
        if let Load::Ready(d) = &mut self.data {
            f(&mut d.bible);
            d.gate = gate_missing(&d.bible);
        }
        let (store, lock) = (self.store.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| commit(&store, &lock, &f)));
        cx.spawn(async move |this, cx| {
            if let Some(e) = failure(task.await) {
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("{what}: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
        self.sync_plan(cx);
        cx.notify();
    }

    /// The Generation panel's edits: shown at once and saved in the background, in turn. A
    /// Generate pressed right after still uses them: a job for this picture that started before
    /// the save landed starts over (`restart_if_started`).
    fn save_spec(&mut self, target: Target, spec: GenSpec, cx: &mut Context<Self>) {
        let s = spec.clone();
        let f = card_op(target.clone(), move |c| c.gen = s.clone());
        if let Load::Ready(d) = &mut self.data {
            f(&mut d.bible);
        }
        if let Some(p) = self.prepared.as_mut().filter(|p| p.target == target) {
            p.spec = spec;
        }
        let since = self.jobs.iter().map(|j| j.id).max().unwrap_or(0);
        let (store, lock, project) = (self.store.clone(), self.writes.clone(), self.ctx.project.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || {
            turn.run(|| {
                let saved = commit(&store, &lock, &f);
                restart_if_started(&project, &target, since);
                saved
            })
        });
        cx.spawn(async move |this, cx| {
            if let Some(e) = failure(task.await) {
                mv_core::logging::global().warn("anime", &format!("bible: settings not saved: {e}"));
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error("The picture's settings could not be saved. Try the change again."));
                    this.reload(cx);
                });
            }
        })
        .detach();
    }

    fn set_status(&mut self, t: Target, status: Status, cx: &mut Context<Self>) {
        // A second approve / take-back click on the same picture right after the first (a double
        // click) is ignored, so it can never undo the approval it just made.
        if matches!(status, Status::Approved | Status::NeedsReview) && !super::frames::accept_status_click(&mut self.last_status_click, &tkey(&t), std::time::Instant::now()) {
            return;
        }
        let has = match &self.data {
            Load::Ready(d) => card_of(&d.bible, &t).is_some_and(|c| c.selected.is_some()),
            _ => false,
        };
        if status == Status::Approved && !has {
            toast::show(cx, Toast::info("Make or upload a picture first, then approve it."));
            return;
        }
        if status == Status::Approved {
            self.approve_pics(vec![t], cx);
            return;
        }
        let what = match status {
            Status::Approved => "The picture could not be approved",
            Status::Rejected => "The picture could not be rejected",
            _ => "The picture's status could not be changed",
        };
        self.change(
            what,
            card_op(t, move |c| {
                if status != Status::Approved || c.selected.is_some() {
                    c.status = status;
                }
            }),
            cx,
        );
    }

    /// "Approve all drawn sheets" of a character: asks first with the count, then approves every
    /// sheet that has a picture waiting for review (counted again when it is written).
    fn confirm_approve_sheets(&mut self, character: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        let Some(c) = d.bible.character(character) else { return };
        let n = sheets_to_approve(c).len();
        if n == 0 {
            toast::show(cx, Toast::info("No drawn sheet is waiting for review."));
            return;
        }
        let name = name_or(&c.name, UNNAMED_CHARACTER).to_string();
        let weak = cx.entity().downgrade();
        let cid = c.id.clone();
        confirm_dialog(
            format!("Approve {} of {name}?", count(n, "drawn sheet")),
            "Every sheet of this character that has a picture waiting for review is approved, and frames then use it as a reference. You can still change or reject each one later.",
            "Approve all",
            false,
        )
        .icon(ph::CHECKS)
        .on_confirm(move |_w, cx| {
            let cid = cid.clone();
            let _ = weak.update(cx, |this, cx| {
                // Counted again now: the same engine approval as a single Approve, per sheet.
                let targets = match &this.data {
                    Load::Ready(d) => d.bible.character(&cid).map(sheets_to_approve).unwrap_or_default(),
                    _ => Vec::new(),
                };
                if targets.is_empty() {
                    return;
                }
                this.approve_pics(targets, cx);
                toast::show(cx, Toast::success("The drawn sheets are approved."));
            });
        })
        .open(window, cx);
    }

    fn set_locked(&mut self, t: Target, locked: bool, cx: &mut Context<Self>) {
        self.change(if locked { "The picture could not be locked" } else { "The picture could not be unlocked" }, card_op(t, move |c| c.locked = locked), cx);
    }

    /// Approves pictures through the engine (`sheets::approve`), which also remembers the request
    /// each is accepted for, so its "outdated" mark clears. Shown at once, written in turn.
    fn approve_pics(&mut self, targets: Vec<Target>, cx: &mut Context<Self>) {
        if let Load::Ready(d) = &mut self.data {
            for t in &targets {
                if let Some(c) = card_entry(&mut d.bible, t).filter(|c| c.selected.is_some()) {
                    c.status = Status::Approved;
                }
            }
            d.gate = gate_missing(&d.bible);
        }
        for t in &targets {
            self.outdated.remove(&tkey(t));
        }
        let store = self.store.clone();
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || turn.run(|| targets.iter().try_for_each(|t| mv_anime::sheets::approve(&store, t))));
        cx.spawn(async move |this, cx| {
            if let Some(e) = failure(task.await) {
                mv_core::logging::global().warn("anime", &format!("bible: approval not saved: {e}"));
                let _ = this.update(cx, |this, cx| {
                    toast::show(cx, Toast::error(format!("The picture could not be approved: {e}")));
                    this.reload(cx);
                });
            }
        })
        .detach();
        self.sync_plan(cx);
        cx.notify();
    }

    /// Puts one of a card's pictures in use (and approves it when asked).
    fn choose(&mut self, t: Target, take: String, approve: bool, cx: &mut Context<Self>) {
        if approve {
            // In use first, then approved by the engine, in that order.
            self.choose(t.clone(), take, false, cx);
            self.approve_pics(vec![t], cx);
            return;
        }
        self.change(
            "That picture could not be put in use",
            card_op(t, move |c| {
                if !c.takes.contains(&take) {
                    return;
                }
                if c.selected.as_deref() != Some(take.as_str()) {
                    c.selected = Some(take.clone());
                    if c.status == Status::Approved {
                        c.status = Status::NeedsReview;
                    }
                }
                if approve {
                    c.status = Status::Approved;
                } else if c.status == Status::Draft {
                    c.status = Status::NeedsReview;
                }
            }),
            cx,
        );
    }

    fn enqueue_pic(&mut self, t: &Target, variants: u8, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        let Some(kind) = gen_panel::job_for(t, variants) else { return };
        let label = pic_title(&d.bible, t);
        backends::ensure_runner(cx);
        queue::enqueue(kind, &self.ctx.project, label);
        self.refresh_jobs();
        cx.notify();
    }

    fn pick_upload(&mut self, t: Target, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Use this picture".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update(cx, |this, cx| this.upload(t, paths, cx));
        })
        .detach();
    }

    /// The user's own pictures for a card (one picked, or several dropped): all are kept as its
    /// pictures, the first goes in use, to be reviewed. Files that are not usable pictures are
    /// skipped and counted.
    fn upload(&mut self, t: Target, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let (store, lock) = (self.store.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || -> anyhow::Result<(usize, usize)> {
            let (added, refused) = import_pictures(&store, &paths);
            let ids: Vec<String> = added.iter().map(|t| t.id.clone()).collect();
            let Some(first) = ids.first().cloned() else {
                bail!("{}", refused.first().cloned().unwrap_or_else(|| "No picture was given.".into()));
            };
            // The card can be gone meanwhile (a station removed): then nothing is attached.
            let found = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let hit = found.clone();
            let n = ids.len();
            turn.run(|| {
                commit(
                    &store,
                    &lock,
                    &op(move |b| {
                        let Some(c) = card_entry(b, &t) else { return };
                        hit.store(true, std::sync::atomic::Ordering::SeqCst);
                        for id in &ids {
                            if !c.takes.contains(id) {
                                c.takes.push(id.clone());
                            }
                        }
                        c.selected = Some(first.clone());
                        c.status = Status::NeedsReview;
                    }),
                )
            })?;
            if !found.load(std::sync::atomic::Ordering::SeqCst) {
                bail!("This picture is no longer part of the bible, so your picture was not attached.");
            }
            Ok((n, refused.len()))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |_, cx| match res {
                Ok(Ok((n, refused))) => {
                    let base = if n == 1 { "Your picture is in use.".to_string() } else { format!("Added {}; the first is in use.", count(n, "picture")) };
                    toast::show(cx, Toast::success(format!("{base}{} Approve it when it looks right.", refused_note(refused))));
                }
                Ok(Err(e)) | Err(e) => {
                    mv_core::logging::global().warn("anime", &format!("bible: upload failed: {e:#}"));
                    toast::show(cx, Toast::error(format!("Your picture was not used: {e}")));
                }
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ crops

    fn crop_op(open: &Open, f: impl Fn(&mut Vec<RefCrop>) + Send + Sync + 'static) -> Op {
        match open {
            Open::Character(id) => char_op(id, move |c| f(&mut c.crops)),
            Open::Place(id) => place_op(id, move |p| f(&mut p.crops)),
            Open::Prop(id) => prop_op(id, move |p| f(&mut p.crops)),
        }
    }

    fn accept_crop(&mut self, open: &Open, take: String, cx: &mut Context<Self>) {
        self.change(
            "The picture could not be added to the references",
            Self::crop_op(open, move |crops| {
                if let Some(c) = crops.iter_mut().find(|c| c.take == take) {
                    c.approved = true;
                }
            }),
            cx,
        );
    }

    fn drop_crop(&mut self, open: &Open, take: String, cx: &mut Context<Self>) {
        self.change("The picture could not be dropped", Self::crop_op(open, move |crops| crops.retain(|c| c.take != take)), cx);
    }

    fn pick_crop_upload(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Add as references".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update(cx, |this, cx| this.upload_crops(open, paths, cx));
        })
        .detach();
    }

    /// The user's own reference pictures: they count as approved references.
    fn upload_crops(&mut self, open: Open, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let (store, lock) = (self.store.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || -> anyhow::Result<(usize, usize)> {
            // Files that are not usable pictures are skipped; the others are still added.
            let (takes, refused) = import_pictures(&store, &paths);
            if takes.is_empty() {
                bail!("{}", refused.first().cloned().unwrap_or_else(|| "No picture was given.".into()));
            }
            let added: Vec<RefCrop> = takes.iter().map(|t| RefCrop { file: t.file.clone(), take: t.id.clone(), approved: true, score: 1., ..Default::default() }).collect();
            let n = added.len();
            turn.run(|| {
                commit(
                    &store,
                    &lock,
                    &Self::crop_op(&open, move |crops| {
                        for a in &added {
                            if !crops.iter().any(|c| c.take == a.take) {
                                crops.insert(0, a.clone());
                            }
                        }
                    }),
                )
            })?;
            Ok((n, refused.len()))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |_, cx| match res {
                Ok(Ok((n, refused))) => toast::show(cx, Toast::success(format!("Added {} to the references.{}", count(n, "picture"), refused_note(refused)))),
                Ok(Err(e)) | Err(e) => {
                    mv_core::logging::global().warn("anime", &format!("bible: reference upload failed: {e:#}"));
                    toast::show(cx, Toast::error(format!("Your pictures were not added: {e}")));
                }
            });
        })
        .detach();
    }

    fn find_crops(&mut self, character: &str, name: &str, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        queue::enqueue(JobKind::Crops { character: character.to_string() }, &self.ctx.project, format!("Reference pictures · {name}"));
        self.refresh_jobs();
        cx.notify();
    }

    // ------------------------------------------------------------ whole items

    fn toggle_left_out(&mut self, open: &Open, out: bool, cx: &mut Context<Self>) {
        let status = if out { Status::Rejected } else { Status::Draft };
        let what = if out { "It could not be left out" } else { "It could not be brought back" };
        let f = match open {
            Open::Character(id) => char_op(id, move |c| c.status = status),
            Open::Place(id) => place_op(id, move |p| p.status = status),
            Open::Prop(id) => prop_op(id, move |p| p.status = status),
        };
        self.change(what, f, cx);
    }

    fn toggle_item_lock(&mut self, open: &Open, lock: bool, cx: &mut Context<Self>) {
        let set = move |l: &mut std::collections::BTreeSet<String>| {
            if lock {
                l.insert(String::new());
            } else {
                l.remove("");
            }
        };
        let f = match open {
            Open::Character(id) => char_op(id, move |c| set(&mut c.locked)),
            Open::Place(id) => place_op(id, move |p| set(&mut p.locked)),
            Open::Prop(id) => prop_op(id, move |p| set(&mut p.locked)),
        };
        self.change(if lock { "It could not be locked" } else { "It could not be unlocked" }, f, cx);
    }

    // ------------------------------------------------------------ text

    #[allow(clippy::too_many_arguments)]
    fn edit_text(&mut self, key: String, value: String, placeholder: &'static str, multi: bool, apply: TextOp, check: fn(&str) -> Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_text(cx);
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, if multi { 10 } else { 3 }).submit_on_enter(!multi).placeholder(placeholder));
        input.update(cx, |s, cx| {
            s.set_value(value.clone(), window, cx);
            s.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, move |this: &mut BibleStep, _, ev: &InputEvent, _, cx| {
            let done = match ev {
                InputEvent::Blur => true,
                InputEvent::PressEnter { shift, .. } => !multi && !*shift,
                _ => false,
            };
            if done {
                this.finish_text(true, cx);
            }
        });
        self.editing = Some(Editing { key, input, multi, initial: value, apply, check, _sub: sub });
        cx.notify();
    }

    /// Saves the open text box, if any (the user moved on to something else: a value the check
    /// refuses is dropped with a note).
    fn commit_text(&mut self, cx: &mut Context<Self>) {
        self.finish_text(false, cx);
    }

    /// Saves the open text box. With `keep` (the box itself was left or Enter pressed), a value
    /// the check refuses keeps the box open with the typed words to correct (after Enter the
    /// cursor stays in it; a click elsewhere is not undone).
    fn finish_text(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(e) = self.editing.take() else { return };
        let raw = e.input.read(cx).value().to_string();
        let text = if e.multi { raw.trim().to_string() } else { raw.split_whitespace().collect::<Vec<_>>().join(" ") };
        cx.notify();
        if text == e.initial.trim() {
            return;
        }
        if let Some(why) = (e.check)(&text) {
            toast::show(cx, Toast::warning(why));
            if keep {
                self.editing = Some(e);
            }
            return;
        }
        let apply = e.apply.clone();
        self.change("Your change could not be saved", op(move |b| apply(b, &text)), cx);
    }

    // ------------------------------------------------------------ voices

    fn cast_change(&mut self, who: &str, what: &'static str, f: impl Fn(&mut CharVoice) + Send + Sync + 'static, cx: &mut Context<Self>) {
        let who = who.to_string();
        self.change(
            what,
            op(move |b| {
                if who == NARRATOR {
                    if b.cast.narrator.character.is_empty() {
                        b.cast.narrator.character = NARRATOR.into();
                    }
                    f(&mut b.cast.narrator);
                    return;
                }
                match b.cast.voices.iter_mut().find(|v| v.character == who) {
                    Some(v) => f(v),
                    None => {
                        let mut v = CharVoice { character: who.clone(), speed: 1., ..Default::default() };
                        f(&mut v);
                        b.cast.voices.push(v);
                    }
                }
            }),
            cx,
        );
    }

    fn audition(&mut self, who: &str, name: &str, voice: &str, cx: &mut Context<Self>) {
        if voice.trim().is_empty() {
            toast::show(cx, Toast::info("Choose a voice first, then audition it."));
            return;
        }
        backends::ensure_runner(cx);
        queue::enqueue(JobKind::Audition { character: who.to_string(), voice: voice.to_string() }, &self.ctx.project, format!("Audition · {name}"));
        self.refresh_jobs();
        cx.notify();
    }

    #[cfg(feature = "sound")]
    fn cast_everyone(&mut self, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        if !d.installed.any() {
            toast::show(cx, Toast::info(backends::INSTALL_VOICES_HINT));
            return;
        }
        let manga = d.series.manga_id.clone();
        let installed = d.installed.ids.clone();
        let (store, lock) = (self.store.clone(), self.writes.clone());
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || {
            let studio = mv_recap::series::VoiceCast::load(&manga);
            turn.run(|| {
                let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let characters = store.bible()?.characters;
                store.update_cast(|cast| *cast = mv_anime::voices::recast(cast, &characters, &studio, &installed)).map(|_| ())
            })
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |_, cx| match failure(res) {
                Some(e) => toast::show(cx, Toast::error(format!("The voices could not be chosen: {e}"))),
                None => toast::show(cx, Toast::success("Voices chosen. Locked voices stayed as they were.")),
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ the gate and the build

    fn build(&mut self, cx: &mut Context<Self>) {
        backends::ensure_runner(cx);
        queue::enqueue(JobKind::Bible { episode: self.ctx.episode.clone() }, &self.ctx.project, "Build the bible".into());
        self.refresh_jobs();
        cx.notify();
    }

    /// "Cut the crops again": cuts and scores every reference picture again from the read pages,
    /// on this PC with no AI (what the user approved, uploaded or dropped stays as it is).
    fn recut(&mut self, cx: &mut Context<Self>) {
        if self.recutting {
            return;
        }
        self.recutting = true;
        cx.notify();
        let store = self.store.clone();
        let task = mv_ui::bg::blocking(move || mv_anime::bible::recut_crops(&store));
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.recutting = false;
                match res {
                    Ok(Ok(n)) => toast::show(cx, Toast::success(format!("The crops were cut again: {}. The ones you accepted, uploaded or dropped stayed as they were.", count(n, "picture")))),
                    Ok(Err(e)) | Err(e) => {
                        mv_core::logging::global().warn("anime", &format!("bible: crops not cut again: {e:#}"));
                        toast::show(cx, Toast::error(format!("The crops could not be cut again: {e}")));
                    }
                }
                this.reload(cx);
                cx.notify();
                crate::redraw(cx);
            });
        })
        .detach();
    }

    fn confirm_build_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        confirm_dialog(
            "Build the bible again?",
            "The AI reads the chapters again and adds what is new: characters, places and props it did not find before, and fields that are still empty. Everything you changed, approved or locked stays as it is.",
            "Build again",
            false,
        )
        .icon(ph::SPARKLE)
        .on_confirm(move |_w, cx| {
            let _ = weak.update(cx, |this, cx| this.build(cx));
        })
        .open(window, cx);
    }

    fn approve_bible(&mut self, approve: bool, cx: &mut Context<Self>) {
        let Load::Ready(d) = &self.data else { return };
        if approve && !d.gate.is_empty() {
            toast::show(cx, Toast::info("Some things are still missing; they are listed at the top of the Bible."));
            return;
        }
        let store = self.store.clone();
        self.approving = true;
        // After every change made before it is written, the approval covers the bible as it is
        // on disk then (its hash), and only if nothing is missing in that same read.
        let turn = self.turns.take();
        let task = mv_ui::bg::blocking(move || {
            turn.run(|| {
                // The bible cannot change between the read and the approval (bible lock, then
                // the project file's lock).
                store.with_bible(|| {
                    let bible = store.bible()?;
                    if approve && !gate_missing(&bible).is_empty() {
                        bail!("Some things are still missing; they are listed at the top of the Bible.");
                    }
                    let hash = bible_hash(&bible);
                    store.update_series(|s| s.bible_approved = approve.then(|| Approval { at: mv_anime::store::now_s(), hash })).map(|_| ())
                })
            })
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.approving = false;
                match failure(res) {
                    Some(e) => toast::show(cx, Toast::error(format!("The approval could not be saved: {e}"))),
                    None if approve => toast::show(cx, Toast::success("The bible is approved. Shots and Frames now draw from it.")),
                    None => toast::show(cx, Toast::info("The approval is withdrawn. Approve the bible again when it is ready.")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ outdated pictures

    fn outdated_targets(&self) -> Vec<Target> {
        match &self.data {
            Load::Ready(d) => all_cards(&d.bible).into_iter().map(|(t, _)| t).filter(|t| self.outdated.contains(&tkey(t)) && !left_out(&d.bible, t)).collect(),
            _ => Vec::new(),
        }
    }

    fn update_pic(&mut self, t: Target, cx: &mut Context<Self>) {
        let variants = match &self.data {
            Load::Ready(d) => card_of(&d.bible, &t).map(|c| c.gen.variants).unwrap_or(1),
            _ => 1,
        };
        self.enqueue_pic(&t, variants.max(1), cx);
        self.review_next(&t, cx);
    }

    fn keep_pic(&mut self, t: Target, cx: &mut Context<Self>) {
        self.set_locked(t.clone(), true, cx);
        self.outdated.remove(&tkey(&t));
        self.review_next(&t, cx);
    }

    fn review_next(&mut self, done: &Target, cx: &mut Context<Self>) {
        if !self.reviewing {
            return;
        }
        let queued: HashSet<String> = self.outdated_targets().into_iter().filter(|t| matches!(self.pic_busy(t), Busy::Waiting { .. } | Busy::Working { .. })).map(|t| tkey(&t)).collect();
        let next = self.outdated_targets().into_iter().find(|t| t != done && !queued.contains(&tkey(t)));
        match next {
            Some(t) => self.go_to(&t, cx),
            None => {
                self.reviewing = false;
                toast::show(cx, Toast::success("That was the last outdated picture."));
                cx.notify();
            }
        }
    }

    fn start_review(&mut self, cx: &mut Context<Self>) {
        if let Some(first) = self.outdated_targets().into_iter().next() {
            self.reviewing = true;
            self.go_to(&first, cx);
        }
    }

    fn keep_all(&mut self, cx: &mut Context<Self>) {
        let all = self.outdated_targets();
        let n = all.len();
        let keys: Vec<String> = all.iter().map(tkey).collect();
        self.change(
            "The pictures could not be kept",
            op(move |b| {
                for t in &all {
                    if let Some(c) = card_entry(b, t) {
                        c.locked = true;
                    }
                }
            }),
            cx,
        );
        for k in keys {
            self.outdated.remove(&k);
        }
        if n > 0 {
            toast::show(cx, Toast::success(format!("Kept {} as they are. Unlock a picture to update it later.", count(n, "picture"))));
        }
    }

    fn confirm_update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let all: Vec<Target> = self.outdated_targets().into_iter().filter(|t| !matches!(self.pic_busy(t), Busy::Waiting { .. } | Busy::Working { .. })).collect();
        if all.is_empty() {
            toast::show(cx, Toast::info("The outdated pictures are already in the queue."));
            return;
        }
        let body = format!(
            "Each is drawn again with what changed, on Magnific and only while it shows them as credit-free. The pictures in use stay until you choose a new one. {}",
            time_note(all.len(), Eta::current().mean("sheet"))
        );
        let weak = cx.entity().downgrade();
        confirm_dialog(format!("Update {}?", count(all.len(), "outdated picture")), body, "Update all", false)
            .icon(ph::ARROWS_CLOCKWISE)
            .on_confirm(move |_w, cx| {
                let all = all.clone();
                let _ = weak.update(cx, |this, cx| {
                    for t in &all {
                        // Each picture with its own number of options, as a single Update does.
                        let variants = match &this.data {
                            Load::Ready(d) => card_of(&d.bible, t).map(|c| c.gen.variants).unwrap_or(1),
                            _ => 1,
                        };
                        this.enqueue_pic(t, variants.max(1), cx);
                    }
                });
            })
            .open(window, cx);
    }
}

/// The picture's target is no longer in the bible (a removed station, character, ...).
fn engine_target_gone(b: &BibleIndex, t: &Target) -> bool {
    match t {
        Target::Sheet { owner, sheet } => sheets::sheet_kind(sheet).is_none() || (b.character(owner).is_none() && b.prop(owner).is_none()),
        Target::PlaceSheet { place } => b.place(place).is_none(),
        Target::Plate { place, station, state } => !b.place(place).is_some_and(|p| p.stations.iter().any(|s| &s.id == station) && p.states.iter().any(|s| &s.id == state)),
        _ => false,
    }
}

// ---------------------------------------------------------------- view helpers

fn centred(el: impl IntoElement) -> Div {
    div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(el)
}

/// The 40 px bar at the top of a column.
fn bar() -> Div {
    div().flex().flex_none().items_center().gap(px(space::SM)).h(px(COLUMN_HEAD_H)).px(px(space::MD)).border_b_1().border_color(Tokens::border())
}

/// A small chip over a picture's top-left corner.
fn corner_chip() -> Div {
    div()
        .absolute()
        .top(px(6.))
        .left(px(6.))
        .flex()
        .items_center()
        .gap(px(space::XS))
        .h(px(20.))
        .px(px(6.))
        .rounded(px(radius::SM))
        .bg(Tokens::popover())
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::XS))
        .text_color(Tokens::text_1())
        .whitespace_nowrap()
}

/// What a picture shows while its job waits, draws or failed.
fn busy_marks(key: &str, busy: &Busy, held: bool) -> Vec<AnyElement> {
    match busy {
        Busy::Working { progress, line, .. } => {
            let pct = (progress.clamp(0., 1.) * 100.).round() as u32;
            let hint = if line.trim().is_empty() { "Drawing on Magnific".to_string() } else { line.clone() };
            vec![
                div().absolute().inset_0().bg(Tokens::canvas().alpha(0.35)).into_any_element(),
                corner_chip().id(eid("bi-working", key)).tooltip(tip(hint)).child(spinner(eid("bi-spin", key), Tokens::accent(), icon_px::XS)).child(format!("Drawing {pct}%")).into_any_element(),
                div().absolute().left_0().right_0().bottom_0().child(progress_bar(eid("bi-progress", key), *progress, !held)).into_any_element(),
            ]
        }
        Busy::Waiting { ahead, .. } => {
            let text = if held {
                "Waiting for Magnific".to_string()
            } else if *ahead > 0 {
                format!("Waiting · {ahead} ahead")
            } else {
                "Next in line".to_string()
            };
            vec![corner_chip().child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::XS)).child(text).into_any_element()]
        }
        Busy::Failed { error, .. } => vec![div()
            .id(eid("bi-failed", key))
            .absolute()
            .top(px(6.))
            .left(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.))
            .rounded_full()
            .bg(Tokens::danger_soft())
            .border_1()
            .border_color(Tokens::danger_border())
            .tooltip(tip(format!("Could not be drawn: {error}")))
            .child(ph_icon(ph::WARNING_CIRCLE, Weight::Fill, Tokens::danger(), icon_px::SM))
            .into_any_element()],
        _ => Vec::new(),
    }
}

/// A picture filling `w` x `h` (cover) or fitted inside it on the well (contain).
fn picture(path: Option<PathBuf>, w: Option<f32>, h: f32, contain: bool) -> Div {
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .map(|d| match w {
            Some(w) => d.w(px(w)),
            None => d.w_full(),
        })
        .h(px(h))
        .overflow_hidden()
        .bg(if contain { Tokens::well() } else { Tokens::surface_2() })
        // Not looked at on disk here (render runs every frame); the loads keep only existing files.
        .map(|d| match path {
            Some(p) => d.child(img(mv_ui::thumbs::cover_source(p)).size_full().object_fit(if contain { ObjectFit::Contain } else { ObjectFit::Cover })),
            None => d,
        })
}

/// Width over height of an aspect like "3:4" (16:9 when it cannot be read).
fn aspect_ratio(aspect: &str) -> f32 {
    let mut it = aspect.split(':').map(|v| v.trim().parse::<f32>().ok());
    match (it.next().flatten(), it.next().flatten()) {
        (Some(w), Some(h)) if w > 0. && h > 0. && (w / h).is_finite() => w / h,
        _ => 16. / 9.,
    }
}

/// The large view of a picture at its own aspect: as tall as [`STILL_H`], unless that would be
/// wider than [`STILL_MAX_W`] (a 21:9 turnaround), then as wide as that.
fn still_box(aspect: &str) -> (f32, f32) {
    let r = aspect_ratio(aspect);
    if STILL_H * r <= STILL_MAX_W {
        (STILL_H * r, STILL_H)
    } else {
        (STILL_MAX_W, STILL_MAX_W / r)
    }
}

/// A list thumbnail that shows the whole picture (a 3:4 design is never cut to 16:9).
fn whole_thumb(path: Option<PathBuf>, w: f32, h: f32) -> Div {
    picture(path, Some(w), h, true).rounded(px(radius::MD))
}

/// The empty middle of a picture ("No design yet").
fn no_picture(text: &'static str) -> Div {
    div().absolute().inset_0().flex().flex_col().items_center().justify_center().gap(px(space::XS)).child(icon_ph(ph::IMAGE, Tokens::text_3(), icon_px::MD)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(text))
}


/// A card with no picture: the character's initials in its signature colour (theme hue when
/// unset), tidy rather than a random panel.
fn initials_tile(name: &str, hex: &str, i: usize) -> Div {
    let ring = parse_hex(hex).map(rgb).unwrap_or_else(|| Tokens::hue(hue_for(i)));
    div().absolute().inset_0().flex().flex_col().items_center().justify_center().gap(px(space::SM)).bg(Tokens::surface_2()).child(
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(INITIALS_DISC))
            .rounded_full()
            .bg(Tokens::surface_3())
            .border_2()
            .border_color(ring)
            .child(div().text_size(px(size::XL)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(crate::widgets::initials(name))),
    )
    .child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("No design yet"))
}

/// A neutral tile with an icon and a word (a prop without a sheet).
fn icon_tile(icon: Ph, text: &'static str) -> Div {
    div().absolute().inset_0().flex().flex_col().items_center().justify_center().gap(px(space::SM)).bg(Tokens::surface_2()).child(icon_ph(icon, Tokens::text_3(), icon_px::LG)).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(text))
}

/// A character's signature colour as a small round swatch (theme hue when unset).
fn colour_dot(hex: &str, i: usize, px_size: f32) -> Div {
    let fill = parse_hex(hex).map(rgb).unwrap_or_else(|| Tokens::hue(hue_for(i)));
    div().flex_none().size(px(px_size)).rounded_full().bg(fill).border_1().border_color(Tokens::border_strong())
}

/// A small mark of an item's status for a list row (icon, words in the tooltip).
fn status_mark(id: ElementId, status: Status, locked: bool, outdated: bool, busy: Option<bool>) -> Stateful<Div> {
    let base = div().id(id).flex().flex_none().items_center().gap(px(space::XXS));
    let (icon, colour, text): (Option<Ph>, Rgba, String) = match busy {
        Some(true) => (None, Tokens::accent(), "Working on it now".into()),
        Some(false) => (Some(ph::HOURGLASS), Tokens::text_2(), "Waiting in the queue".into()),
        None if outdated => (Some(ph::ARROWS_CLOCKWISE), Tokens::warning(), "Outdated: something a picture uses changed".into()),
        None => match status {
            Status::Approved => (Some(ph::CHECK), Tokens::success(), "Ready".into()),
            Status::Rejected => (Some(ph::PROHIBIT), Tokens::text_3(), "Left out of the film".into()),
            Status::NeedsReview => (Some(ph::EYE), Tokens::info(), "Pictures to review".into()),
            s => (Some(ph::CIRCLE_DASHED), Tokens::text_3(), status_label(s).to_string()),
        },
    };
    let text = if locked { format!("{text} · locked") } else { text };
    base.tooltip(tip(text)).when(locked, |d| d.child(icon_ph(ph::LOCK_SIMPLE, Tokens::text_3(), icon_px::XS))).map(|d| match icon {
        Some(i) => d.child(icon_ph(i, colour, icon_px::SM)),
        None => d.child(spinner("bi-mark-spin", colour, icon_px::SM)),
    })
}

/// "Needs: face sheet, turnaround" or "Ready for the approval".
fn needs_line(needs: &[&str], status: Status) -> Div {
    let (icon, colour, text) = if status == Status::Rejected {
        (ph::PROHIBIT, Tokens::text_3(), "Left out of the film".to_string())
    } else if needs.is_empty() {
        (ph::CHECK, Tokens::success(), "Nothing missing for the approval".to_string())
    } else {
        (ph::CIRCLE_DASHED, Tokens::text_3(), format!("Needs {}", needs.join(", ")))
    };
    div().flex().items_center().gap(px(space::XS)).child(icon_ph(icon, colour, icon_px::XS)).child(div().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(text))
}

/// Clickable text that becomes a text box (placeholder in `text_3` when empty).
fn editable(id: ElementId, text: &str, placeholder: &str, multi: bool) -> Stateful<Div> {
    let empty = text.trim().is_empty();
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .px(px(space::XS))
        .py(px(2.))
        .rounded(px(radius::SM))
        .cursor_text()
        .hover(|s| s.bg(Tokens::hover()))
        .text_size(px(size::SM))
        .line_height(px(18.))
        .text_color(if empty { Tokens::text_3() } else { Tokens::text_1() })
        .when(multi, |d| d.whitespace_normal())
        .when(!multi, |d| d.truncate())
        .child(if empty { placeholder.to_string() } else { text.trim().to_string() })
        .tooltip(tip("Click to change"))
}

/// A text box in the screen's frame (focused: strong border).
fn editor(input: &Entity<TextareaState>, window: &Window, cx: &App) -> Div {
    let focused = input.read(cx).focus_handle(cx).contains_focused(window, cx);
    div()
        .flex_1()
        .min_w_0()
        .px(px(space::SM))
        .py(px(space::XS))
        .rounded(px(radius::MD))
        .bg(Tokens::surface())
        .border_1()
        .border_color(if focused { Tokens::text_1() } else { Tokens::border_strong() })
        .child(Textarea::new(input).appearance(false).bordered(false))
}

/// A labelled block with a counter on the right of its label ("SHEETS      3 of 6 approved").
fn titled(title: &str, right: impl Into<SharedString>) -> Div {
    div().flex().flex_col().gap(px(space::SM)).child(label_row(title, right))
}

/// A removable word chip ("scar on left cheek  ×"); add the remove button as a child.
fn word_chip(text: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(space::XS))
        .h(px(26.))
        .pl(px(9.))
        .pr(px(space::XXS))
        .rounded(px(radius::SM))
        .bg(Tokens::surface_2())
        .border_1()
        .border_color(Tokens::border())
        .text_size(px(size::SM))
        .text_color(Tokens::text_1())
        .child(text.to_string())
}

// ---------------------------------------------------------------- view

impl Render for BibleStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
        self.ensure_plan(window, cx);
        let body = match &self.data {
            Load::Loading => self.skeleton(window).into_any_element(),
            Load::Failed(e) => centred(error_view(format!("The bible could not be read: {e}")).retry(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.data = Load::Loading;
                this.reload(cx);
                cx.notify();
            })))
            .into_any_element(),
            Load::Ready(d) if d.empty() => self.empty(d, cx).into_any_element(),
            Load::Ready(d) => self.ready(d, window, cx).into_any_element(),
        };
        div().size_full().child(body)
    }
}

impl BibleStep {
    fn skeleton(&self, window: &Window) -> Div {
        let grid = div()
            .flex()
            .flex_col()
            .size_full()
            .child(bar().child(skeleton_line(px(TABS_W))))
            .child(div().flex().flex_wrap().gap(px(space::MD)).p(px(space::MD)).children((0..6).map(|_| skeleton_block(px(TILE_W), px(CAST_H)))));
        workspace(window, None, grid, None)
    }

    /// No bible yet: one action, or what has to happen first, or the build in progress.
    fn empty(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let run = self.bible_run();
        if run.active() {
            let body = div()
                .flex()
                .flex_col()
                .gap(px(space::MD))
                .w_full()
                .max_w(px(TILE_W * 2.2))
                .child(div().flex().items_center().gap(px(space::SM)).child(spinner("bi-build-spin", Tokens::accent(), icon_px::SM)).child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Building the bible")))
                .child(div().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_2()).whitespace_normal().child("The AI reads the chapters and drafts the cast, the places with their floor plans and camera stations, the props and the style. It takes a few minutes; you can work in other steps meanwhile."))
                .children(self.progress(&run));
            return centred(card().w_full().max_w(px(TILE_W * 2.2)).child(body));
        }
        let go = self.ctx.go.clone();
        let view = if d.chapters == 0 {
            empty_view(Step::Bible.icon(), "No chapters yet", "The bible is drafted from the chapters this episode tells. Add them in the Manga step first.")
                .action(btn("bi-go-manga", "Go to Manga", ButtonKind::Primary).icon(Step::Manga.icon()).on_click(move |_, w, cx| go(Step::Manga, w, cx)))
        } else if d.read == 0 {
            empty_view(Step::Bible.icon(), "Read the chapters first", "The bible is drafted from Studio's Read of each chapter: who appears, where they are and what they hold. Read the chapters in the Manga step, then come back.")
                .action(btn("bi-go-manga", "Go to Manga", ButtonKind::Primary).icon(Step::Manga.icon()).on_click(move |_, w, cx| go(Step::Manga, w, cx)))
        } else {
            empty_view(
                Step::Bible.icon(),
                "No bible yet",
                "The AI drafts every recurring character, place and prop from the read chapters, with floor plans, camera stations and the style. You check and approve each one; nothing is drawn until you ask.",
            )
            .action(btn("bi-build", "Build the bible", ButtonKind::Primary).icon(ph::SPARKLE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.build(cx))))
        };
        let failed = match &run {
            Run::Failed { id, error } => Some(self.failure(*id, error.clone())),
            _ => None,
        };
        div().size_full().flex().flex_col().items_center().justify_center().gap(px(space::LG)).p(px(space::XXL)).child(view).children(failed.map(|f| div().w_full().max_w(px(TILE_W * 2.2)).child(f)))
    }

    fn failure(&self, id: u64, error: String) -> Div {
        error_block(format!("The bible could not be built: {error}")).child(
            div()
                .flex()
                .flex_none()
                .gap(px(space::XS))
                .child(btn(("bi-retry", id as usize), "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(move |_, _, cx| {
                    backends::ensure_runner(cx);
                    queue::retry(id);
                }))
                .child(icon_button(("bi-dismiss", id as usize), ph::X, "Dismiss").small().on_click(move |_, _, _| queue::cancel(id))),
        )
    }

    /// Progress of the build, with Cancel (and Try now while the AI waits for its limit).
    fn progress(&self, run: &Run) -> Option<Div> {
        match run {
            Run::Running { id, progress, line } => {
                let id = *id;
                let waiting = line.contains("usage limit") || line.contains("Waiting about");
                let line = if line.trim().is_empty() { "Building…".to_string() } else { line.clone() };
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .child(progress_bar("bi-progress", *progress, !waiting))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::SM))
                                .child(div().flex_1().min_w_0().text_size(px(size::XS)).text_color(if waiting { Tokens::warning() } else { Tokens::text_2() }).whitespace_normal().child(line))
                                .when(waiting, |d| d.child(btn("bi-now", "Try now", ButtonKind::Secondary).small().on_click(|_, _, _| mv_anime::llm::retry_now())))
                                .child(btn("bi-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                        ),
                )
            }
            Run::Waiting { id, paused } => {
                let (id, paused) = (*id, *paused);
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(icon_ph(if paused { ph::PAUSE_CIRCLE } else { ph::HOURGLASS }, Tokens::text_2(), icon_px::SM))
                        // The one place to resume is the queue strip at the bottom of the tab.
                        .child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_2()).child(if paused { "Waiting: the queue is paused. Resume it in the queue strip below." } else { "Waiting in the queue" }))
                        .child(btn("bi-cancel", "Cancel", ButtonKind::Ghost).small().on_click(move |_, _, _| queue::cancel(id))),
                )
            }
            _ => None,
        }
    }

    fn ready(&self, d: &Data, window: &Window, cx: &mut Context<Self>) -> Div {
        let narrow = crate::widgets::is_narrow(window);
        match (self.tab, &self.open) {
            (Tab::Style, _) => workspace(window, None, self.style_view(d, window, cx), self.panel_el(narrow)),
            (Tab::Voices, _) => workspace(window, None, self.voices_view(d, cx), None),
            (_, None) => workspace(window, None, self.gallery(d, cx), None),
            (_, Some(open)) => {
                let open = open.clone();
                workspace(window, Some(self.list(d, &open, cx).into_any_element()), self.detail(d, &open, window, cx), self.panel_el(narrow))
            }
        }
    }

    /// The Generation panel of the open picture (a drawer below the narrow width).
    fn panel_el(&self, narrow: bool) -> Option<AnyElement> {
        if narrow && !self.drawer {
            return None;
        }
        self.pic.as_ref()?;
        Some(match &self.panel {
            Some(p) => p.clone().into_any_element(),
            None => div().flex().flex_col().gap(px(space::MD)).p(px(space::MD)).child(skeleton_block(relative(1.), px(REF_TILE))).child(skeleton_line(px(TILE_W / 2.))).child(skeleton_block(relative(1.), px(REF_TILE))).child(skeleton_block(relative(1.), px(TILE_H))).into_any_element(),
        })
    }

    // ------------------------------------------------------------ the top bar and notices

    fn tabs(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let mains = d.mains();
        let review_cast = d.bible.characters.iter().any(|c| character_status(c, mains.contains(&c.id)) == Status::NeedsReview);
        let review_places = d.bible.places.iter().any(|p| place_status(p) == Status::NeedsReview);
        let review_props = d.bible.props.iter().any(|p| prop_status(p) == Status::NeedsReview);
        let review_style = d.bible.style.frame.status == Status::NeedsReview && d.bible.style.frame.selected.is_some();
        let tabs: Vec<(Tab, &'static str, Ph, bool)> = Tab::ALL
            .into_iter()
            .map(|t| {
                let live = match t {
                    Tab::Cast => review_cast,
                    Tab::Places => review_places,
                    Tab::Props => review_props,
                    Tab::Style => review_style,
                    Tab::Voices => false,
                };
                (t, t.label(), t.icon(), live)
            })
            .collect();
        let weak = cx.entity().downgrade();
        div().w(px(TABS_W)).child(tab_bar("bi-tabs", &tabs, self.tab, move |t, _w, cx| {
            let t = *t;
            let _ = weak.update(cx, |this, cx| this.set_tab(t, cx));
        }))
    }

    /// The tab strip with the counts, the main action and the overflow menu.
    fn top_bar(&self, d: &Data, primary: bool, cx: &mut Context<Self>) -> Div {
        // One count on every tab: the bible's items in the film (characters, places, props and
        // the style) and how many are ready.
        let (ready, total) = bible_readiness(&d.bible, &d.mains());
        let kind = if primary { ButtonKind::Primary } else { ButtonKind::Secondary };
        let main = match d.approval() {
            Some((_, true)) if d.gate.is_empty() => {
                let go = self.ctx.go.clone();
                btn("bi-main", "Continue to Shots", kind).small().icon(Step::Shots.icon()).tooltip("The bible is approved; plan the shots next").on_click(move |_, w, cx| go(Step::Shots, w, cx))
            }
            approval => {
                let label = if approval.is_some() { "Approve again" } else { "Approve the bible" };
                let b = btn("bi-main", label, kind).small().icon(ph::SEAL_CHECK).loading(self.approving);
                if d.gate.is_empty() {
                    b.tooltip("Freeze who and where the film is: Shots and Frames draw from it").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.approve_bible(true, cx)))
                } else {
                    b.disabled(format!("Still missing: {}", count(d.gate.len(), "thing")))
                }
            }
        };
        let (a, b) = (cx.entity().downgrade(), cx.entity().downgrade());
        let mut items = vec![MenuItem::new("Build the bible again", move |w, cx| {
            let _ = a.update(cx, |this, cx| this.confirm_build_again(w, cx));
        })
        .icon(ph::SPARKLE)
        .disabled(self.bible_run().active())];
        let r = cx.entity().downgrade();
        items.push(
            MenuItem::new(if self.recutting { "Cutting the crops again…" } else { "Cut the crops again" }, move |_, cx| {
                let _ = r.update(cx, |this, cx| this.recut(cx));
            })
            .icon(ph::CROP)
            .disabled(self.recutting || self.bible_run().active()),
        );
        if d.series.bible_approved.is_some() {
            items.push(MenuItem::separator());
            items.push(
                MenuItem::new("Withdraw the approval", move |_, cx| {
                    let _ = b.update(cx, |this, cx| this.approve_bible(false, cx));
                })
                .icon(ph::X),
            );
        }
        let outdated = self.outdated.len();
        bar()
            .child(self.tabs(d, cx))
            .child(div().flex_1())
            .when(total > 0, |b| b.child(spec(format!("{ready} of {total} ready"))))
            .when(outdated > 0, |b| b.child(self.outdated_menu(outdated, cx)))
            .child(main)
            .child(menu_button("bi-more", ph::DOTS_THREE, "More actions", items))
    }

    fn outdated_menu(&self, n: usize, cx: &mut Context<Self>) -> AnyElement {
        let (a, b, c) = (cx.entity().downgrade(), cx.entity().downgrade(), cx.entity().downgrade());
        let items = vec![
            MenuItem::new("Update all", move |w, cx| {
                let _ = a.update(cx, |this, cx| this.confirm_update_all(w, cx));
            })
            .icon(ph::SPARKLE),
            MenuItem::new("Review one by one", move |_, cx| {
                let _ = b.update(cx, |this, cx| this.start_review(cx));
            })
            .icon(ph::EYE),
            MenuItem::separator(),
            MenuItem::new("Keep all as they are", move |_, cx| {
                let _ = c.update(cx, |this, cx| this.keep_all(cx));
            })
            .icon(ph::LOCK_SIMPLE),
        ];
        menu_button("bi-outdated", ph::ARROWS_CLOCKWISE, "Pictures whose references or settings changed: update, review or keep", items).label(format!("{n} outdated")).small().into_any_element()
    }

    /// What stops or needs the user: the build, the approval, failures, the queue, Magnific.
    fn notices(&self, d: &Data, cx: &mut Context<Self>) -> Option<Div> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(e) = &self.refresh_error {
            out.push(
                error_block(format!("The latest changes could not be read: {e}"))
                    .child(btn("bi-refresh", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))))
                    .into_any_element(),
            );
        }
        // A still line (nothing that turns), gone when the crops are done.
        if self.tidying || self.recutting {
            let text = if self.recutting { "Cutting the crops again from the read pages…" } else { "Tidying the cast pictures…" };
            out.push(info_line(ph::CROP, LineTone::Neutral, text).into_any_element());
        }
        let run = self.bible_run();
        match &run {
            Run::Running { .. } | Run::Waiting { .. } => {
                out.push(card().child(div().flex().items_center().gap(px(space::SM)).child(spinner("bi-again-spin", Tokens::accent(), icon_px::SM)).child(label_caps("Building the bible again"))).children(self.progress(&run)).into_any_element());
            }
            Run::Failed { id, error } => out.push(self.failure(*id, error.clone()).into_any_element()),
            Run::Idle => {}
        }
        out.push(self.approval_notice(d, cx).into_any_element());
        // Being signed out of Magnific is shown in the picture panel only, not on every tab.
        let failed = self.jobs.iter().filter(|j| j.state == JobState::Failed && !j.hidden && matches!(j.kind, JobKind::Sheet { .. })).count();
        if failed > 0 {
            out.push(
                inline_alert(AlertKind::Warning, format!("{} could not be drawn", count(failed, "picture")), "Point at the red mark on a picture to see why.")
                    .action("Try them again", cx.listener(|this, _: &ClickEvent, _, cx| {
                        backends::ensure_runner(cx);
                        for j in this.jobs.iter().filter(|j| j.state == JobState::Failed && !j.hidden && matches!(j.kind, JobKind::Sheet { .. })) {
                            queue::retry(j.id);
                        }
                    }))
                    .into_any_element(),
            );
        }
        let waiting = self.jobs.iter().filter(|j| matches!(j.state, JobState::Queued) && matches!(j.kind, JobKind::Sheet { .. })).count();
        if self.queue_paused && waiting > 0 {
            // Why they wait; the one Resume is the queue strip's.
            out.push(inline_alert(AlertKind::Info, "The queue is paused", format!("{} waiting. They are drawn once the queue is resumed.", count(waiting, "picture"))).into_any_element());
        }
        (!out.is_empty()).then(|| div().flex().flex_col().gap(px(space::SM)).children(out))
    }

    /// Where the approval stands, and plainly what is still missing.
    fn approval_notice(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let now = now_secs();
        match d.approval() {
            Some((a, same)) if d.gate.is_empty() => {
                if same {
                    info_line(ph::SEAL_CHECK, LineTone::Success, format!("Approved {}. Shots and Frames draw from this bible; a change here marks the pictures that used it as outdated.", ago(a.at, now)))
                } else {
                    info_line(ph::INFO, LineTone::Info, format!("Approved {}, and changed since. Approve again when you are happy with the changes.", ago(a.at, now)))
                }
            }
            approval => {
                if d.gate.is_empty() {
                    return info_line(ph::CHECK, LineTone::Success, "Everything the bible needs is approved. Approve the bible to move on to Shots.");
                }
                // Compact: the count and the first few items on one line, the rest on request.
                // Every item opens what it names. A quiet fill: still to do, never an error.
                let (shown, more) = missing_lines(&d.gate);
                let lines: Vec<String> = if self.gate_open { d.gate.iter().map(|s| s.trim().trim_end_matches('.').to_string()).collect() } else { shown };
                let title = if approval.is_some() { "Approved earlier, now missing" } else { "To approve the bible" };
                let mut row = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::XS))
                    .child(icon_ph(ph::LIST_CHECKS, Tokens::warning(), icon_px::SM))
                    .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("{title} · {}:", count(d.gate.len(), "thing"))));
                for (i, line) in lines.iter().enumerate() {
                    let target = self.missing_target(d, line);
                    row = row.child(
                        div()
                            .id(("bi-missing", i))
                            .flex()
                            .items_center()
                            .gap(px(space::XS))
                            .max_w(px(GATE_ITEM_MAX_W))
                            .h(px(GATE_ITEM_H))
                            .px(px(space::SM))
                            .rounded(px(radius::MD))
                            .bg(Tokens::surface_3())
                            .child(icon_ph(ph::CIRCLE_DASHED, Tokens::text_3(), icon_px::XS))
                            .child(div().min_w_0().truncate().text_size(px(size::XS)).text_color(Tokens::text_1()).child(line.clone()))
                            .map(|chip| match target {
                                Some(t) => chip.cursor_pointer().hover(|s| s.bg(Tokens::hover())).tooltip(tip(format!("{line}. Open it"))).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go_to(&t, cx))),
                                None => chip.tooltip(tip(line.clone())),
                            }),
                    );
                }
                if more > 0 {
                    let label = if self.gate_open { "Show less".to_string() } else { format!("and {more} more") };
                    row = row.child(btn("bi-missing-more", label, ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.gate_open = !this.gate_open;
                        cx.notify();
                    })));
                }
                div().px(px(space::MD)).py(px(space::SM)).rounded(px(radius::LG)).bg(Tokens::surface_2()).border_1().border_color(Tokens::border()).child(row)
            }
        }
    }

    /// The picture a gate line is about ("Jinwoo: approve the face sheet." → his face sheet).
    fn missing_target(&self, d: &Data, line: &str) -> Option<Target> {
        if line.trim_start().starts_with("Write the style line") {
            return Some(Target::StyleFrame);
        }
        let (who, what) = line.split_once(':')?;
        let who = who.trim();
        if let Some(c) = d.bible.characters.iter().find(|c| name_or(&c.name, &c.id) == who) {
            let sheet = NEEDED.iter().find(|(k, w)| what.contains(w) && !sheet_ok(&c.sheets, k)).map(|(k, _)| sheets::sheet_key(k)).unwrap_or_else(|| "master".into());
            return Some(Target::Sheet { owner: c.id.clone(), sheet });
        }
        let p = d.bible.places.iter().find(|p| name_or(&p.name, &p.id) == who)?;
        if what.contains("overhead plate") {
            let st = p.stations.iter().find(|s| s.overhead)?;
            let state = p.states.first().map(|s| s.id.clone()).unwrap_or_else(|| "day".into());
            return Some(Target::Plate { place: p.id.clone(), station: st.id.clone(), state });
        }
        Some(Target::PlaceSheet { place: p.id.clone() })
    }

    // ------------------------------------------------------------ the grid of a tab

    fn gallery(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let mut body = column_body("bi-gallery").gap(px(space::LG)).track_scroll(&self.body_scroll);
        if let Some(n) = self.notices(d, cx) {
            body = body.child(n);
        }
        let mut grid = div().flex().flex_wrap().gap(px(space::MD));
        let mains = d.mains();
        let mut n = 0;
        match self.tab {
            Tab::Cast => {
                for (i, c) in d.bible.characters.iter().enumerate() {
                    grid = grid.child(self.cast_card(d, i, c, mains.contains(&c.id), cx));
                    n += 1;
                }
            }
            Tab::Places => {
                for (i, p) in d.bible.places.iter().enumerate() {
                    grid = grid.child(self.place_card(d, i, p, cx));
                    n += 1;
                }
            }
            Tab::Props => {
                for (i, p) in d.bible.props.iter().enumerate() {
                    grid = grid.child(self.prop_card(d, i, p, cx));
                    n += 1;
                }
            }
            _ => {}
        }
        if n == 0 {
            let (icon, title, text) = match self.tab {
                Tab::Places => (ph::MOUNTAINS, "No places", "The bible found no recurring place in the read chapters. Build it again after reading more chapters."),
                Tab::Props => (ph::CUBE, "No props", "No object is held or shown close in the read chapters, so there is nothing to design here."),
                _ => (ph::USERS, "No characters", "The bible found no recurring character in the read chapters. Build it again after reading more chapters."),
            };
            body = body.child(empty_gallery(icon, title, text));
        } else {
            body = body.child(grid);
        }
        div().flex().flex_col().size_full().child(self.top_bar(d, true, cx)).child(body)
    }

    fn card_shell(&self, id: ElementId, group: SharedString, open: Open, cx: &mut Context<Self>) -> Stateful<Div> {
        selectable(id, false).relative().group(group).w(px(TILE_W)).overflow_hidden().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_item(open.clone(), cx)))
    }

    fn corner_busy(&self, owner: &str) -> Option<AnyElement> {
        let busy = self.item_busy(owner)?;
        Some(if busy {
            corner_chip().child(spinner(eid("bi-card-spin", owner), Tokens::accent(), icon_px::XS)).child("Working").into_any_element()
        } else {
            corner_chip().child(icon_ph(ph::HOURGLASS, Tokens::text_2(), icon_px::XS)).child("Waiting").into_any_element()
        })
    }

    fn outdated_any(&self, prefix: &str) -> bool {
        self.outdated.iter().any(|k| k.starts_with(prefix))
    }

    fn cast_card(&self, d: &Data, i: usize, c: &Character, main: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let status = character_status(c, main);
        let needs = char_needs(c, main);
        let master = c.sheets.iter().find(|s| s.kind == SheetKind::Master).and_then(|s| s.card.selected.as_deref()).and_then(|id| d.files.get(id)).cloned();
        let usable: Vec<&RefCrop> = c.crops.iter().filter(|r| mv_anime::crops::usable(r)).collect();
        // Without a design: the best face picture from the manga, else the initials tile.
        let stand_in = if master.is_none() { d.covers.get(&format!("c:{}", c.id)).cloned() } else { None };
        let from_manga = master.is_none() && stand_in.is_some();
        let has_pic = master.is_some() || stand_in.is_some();
        let pic = picture(master.or(stand_in), None, CAST_H, false)
            .when(from_manga, |p| p.child(corner_chip().child(icon_ph(ph::BOOK_OPEN_TEXT, Tokens::text_2(), icon_px::XS)).child("From the manga")))
            .when(!has_pic, |p| p.child(initials_tile(name_or(&c.name, UNNAMED_CHARACTER), &c.look.signature_colour, i)))
            .children(self.corner_busy(&c.id));
        let mut crops = div().flex().items_center().gap(px(space::XXS));
        let all: Vec<&RefCrop> = usable.iter().copied().chain(c.crops.iter().filter(|r| !mv_anime::crops::usable(r))).collect();
        for r in all.iter().take(CROPS_ON_CARD) {
            let ok = mv_anime::crops::usable(r);
            crops = crops.child(div().rounded(px(radius::XS)).overflow_hidden().border_1().border_color(if ok { Tokens::border_strong() } else { Tokens::border() }).when(!ok, |d| d.opacity(0.55)).child(thumb(Some(self.store.path(&r.file)), CROP_MINI, CROP_MINI)));
        }
        if all.len() > CROPS_ON_CARD {
            crops = crops.child(div().pl(px(space::XXS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("+{}", all.len() - CROPS_ON_CARD)));
        }
        let suggestions = all.len() - usable.len();
        let caption = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(colour_dot(&c.look.signature_colour, i, 10.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name_or(&c.name, UNNAMED_CHARACTER).to_string()))
                    .child(spec(role_word(c.role))),
            )
            .child(card_marks(status, root_locked(&c.locked), self.outdated_any(&format!("sheet/{}/", c.id))))
            .child(needs_line(&needs, status))
            .when(!all.is_empty(), |cap| {
                cap.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .child(crops)
                        .child(div().flex_1())
                        .when(suggestions > 0, |d| d.child(div().text_size(px(size::XS)).text_color(Tokens::info()).child(format!("{suggestions} to check")))),
                )
            });
        self.card_shell(("bi-cast", i).into(), SharedString::from(format!("bi-cast-{i}")), Open::Character(c.id.clone()), cx)
            .tooltip(tip("Open the character"))
            .child(pic)
            .child(caption)
            .when(c.status == Status::Rejected, |d| d.opacity(0.6))
    }

    fn place_card(&self, d: &Data, i: usize, p: &Place, cx: &mut Context<Self>) -> Stateful<Div> {
        let status = place_status(p);
        let needs = place_needs(p);
        let overhead = p.stations.iter().find(|s| s.overhead).map(|s| s.id.clone());
        let master = p.sheet.selected.as_deref().and_then(|id| d.files.get(id)).cloned().or_else(|| {
            p.plates.iter().filter(|pl| Some(&pl.station) == overhead.as_ref()).find_map(|pl| pl.card.selected.as_deref()).and_then(|id| d.files.get(id)).cloned()
        });
        // Only a panel that shows the place itself stands in; with none, a neutral tile.
        let stand_in = if master.is_none() { d.covers.get(&format!("p:{}", p.id)).cloned() } else { None };
        let from_manga = master.is_none() && stand_in.is_some();
        let pic = picture(master.or(stand_in), None, TILE_H, false)
            .when(from_manga, |pc| pc.child(corner_chip().child(icon_ph(ph::BOOK_OPEN_TEXT, Tokens::text_2(), icon_px::XS)).child("From the manga")))
            .when(!from_manga && p.sheet.selected.is_none() && p.plates.iter().all(|pl| pl.card.selected.is_none()), |pc| pc.child(no_picture("No master plate yet")))
            .children(self.corner_busy(&p.id));
        let plates = p.plates.iter().filter(|pl| approved(&pl.card)).count();
        let caption = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name_or(&p.name, UNNAMED_PLACE).to_string()))
                    .child(spec(if p.kind == PlaceKind::Exterior { "Outside" } else { "Inside" })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::XS))
                    .child(spec(count(p.stations.len(), "station")))
                    .child(spec(format!("{plates} of {} plates approved", p.stations.len() * p.states.len().max(1)))),
            )
            .child(card_marks(status, root_locked(&p.locked), self.outdated.contains(&format!("place/{}", p.id)) || self.outdated_any(&format!("plate/{}/", p.id))))
            .child(needs_line(&needs, status));
        self.card_shell(("bi-place", i).into(), SharedString::from(format!("bi-place-{i}")), Open::Place(p.id.clone()), cx)
            .tooltip(tip("Open the place and its floor plan"))
            .child(pic)
            .child(caption)
            .when(p.status == Status::Rejected, |d| d.opacity(0.6))
    }

    fn prop_card(&self, d: &Data, i: usize, p: &Prop, cx: &mut Context<Self>) -> Stateful<Div> {
        let status = prop_status(p);
        let master = p.sheets.iter().find(|s| s.kind == SheetKind::Master).and_then(|s| s.card.selected.as_deref()).and_then(|id| d.files.get(id)).cloned();
        // A prop's manga pictures are whole panels (captions, bubbles, other things): only one the
        // user approved stands in; otherwise a neutral tile.
        let stand_in = if master.is_none() { p.crops.iter().find(|r| r.approved).map(|r| self.store.path(&r.file)) } else { None };
        let from_manga = master.is_none() && stand_in.is_some();
        let pic = picture(master.or(stand_in), None, TILE_H, false)
            .when(from_manga, |pc| pc.child(corner_chip().child(icon_ph(ph::BOOK_OPEN_TEXT, Tokens::text_2(), icon_px::XS)).child("From the manga")))
            .when(!from_manga && p.sheets.iter().all(|s| s.card.selected.is_none()), |pc| pc.child(icon_tile(ph::CUBE, "No prop sheet yet")))
            .children(self.corner_busy(&p.id));
        let owner = p.owner.as_deref().and_then(|o| d.bible.character(o)).map(|c| name_or(&c.name, UNNAMED_CHARACTER).to_string());
        let held = match (&owner, p.held) {
            (Some(o), true) => format!("Held by {o}"),
            (Some(o), false) => format!("{o}'s"),
            (None, true) => "Held in hand".to_string(),
            (None, false) => "Not held".to_string(),
        };
        let caption = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .child(div().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name_or(&p.name, UNNAMED_PROP).to_string()))
            .child(div().flex().flex_wrap().gap(px(space::XS)).child(spec(held)).when(!p.states.is_empty(), |d| d.child(spec(count(p.states.len(), "state")))))
            .child(card_marks(status, root_locked(&p.locked), self.outdated_any(&format!("sheet/{}/", p.id))));
        self.card_shell(("bi-prop", i).into(), SharedString::from(format!("bi-prop-{i}")), Open::Prop(p.id.clone()), cx)
            .tooltip(tip("Open the prop"))
            .child(pic)
            .child(caption)
            .when(p.status == Status::Rejected, |d| d.opacity(0.6))
    }

    // ------------------------------------------------------------ an open item: the list

    fn list(&self, d: &Data, open: &Open, cx: &mut Context<Self>) -> Div {
        let mains = d.mains();
        let mut body = column_body("bi-list").gap(px(space::XXS));
        let mut n = 0;
        let row = |this: &Self, i: usize, key: &str, item: Open, name: String, sub: String, pic: Option<PathBuf>, status: Status, locked: bool, outdated: bool, cx: &mut Context<Self>| {
            let on = &item == open;
            let busy = this.item_busy(key);
            // The shared fixed-height row: many entries never squash it.
            list_item(eid("bi-row", key), on)
                .gap(px(space::SM))
                .px(px(space::XS))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_item(item.clone(), cx)))
                .child(whole_thumb(pic, LIST_THUMB.0, LIST_THUMB.1))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(space::XXS))
                        .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name))
                        .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(sub)),
                )
                .child(status_mark(eid("bi-row-mark", &format!("{i}")), status, locked, outdated, busy))
        };
        let title = match open {
            Open::Character(_) => {
                for (i, c) in d.bible.characters.iter().enumerate() {
                    let pic = c.sheets.iter().find(|s| s.kind == SheetKind::Master).and_then(|s| s.card.selected.as_deref()).and_then(|id| d.files.get(id)).cloned().or_else(|| d.covers.get(&format!("c:{}", c.id)).cloned());
                    body = body.child(row(self, i, &c.id, Open::Character(c.id.clone()), name_or(&c.name, UNNAMED_CHARACTER).to_string(), role_word(c.role).to_string(), pic, character_status(c, mains.contains(&c.id)), root_locked(&c.locked), self.outdated_any(&format!("sheet/{}/", c.id)), cx));
                    n += 1;
                }
                "Cast"
            }
            Open::Place(_) => {
                for (i, p) in d.bible.places.iter().enumerate() {
                    let pic = p.sheet.selected.as_deref().and_then(|id| d.files.get(id)).cloned().or_else(|| d.covers.get(&format!("p:{}", p.id)).cloned());
                    let out = self.outdated.contains(&format!("place/{}", p.id)) || self.outdated_any(&format!("plate/{}/", p.id));
                    body = body.child(row(self, i, &p.id, Open::Place(p.id.clone()), name_or(&p.name, UNNAMED_PLACE).to_string(), count(p.stations.len(), "station"), pic, place_status(p), root_locked(&p.locked), out, cx));
                    n += 1;
                }
                "Places"
            }
            Open::Prop(_) => {
                for (i, p) in d.bible.props.iter().enumerate() {
                    let pic = p.sheets.iter().find(|s| s.kind == SheetKind::Master).and_then(|s| s.card.selected.as_deref()).and_then(|id| d.files.get(id)).cloned().or_else(|| p.crops.iter().find(|r| r.approved).map(|r| self.store.path(&r.file)));
                    let owner = p.owner.as_deref().and_then(|o| d.bible.character(o)).map(|c| format!("Held by {}", name_or(&c.name, UNNAMED_CHARACTER))).unwrap_or_else(|| "Prop".into());
                    body = body.child(row(self, i, &p.id, Open::Prop(p.id.clone()), name_or(&p.name, UNNAMED_PROP).to_string(), owner, pic, prop_status(p), root_locked(&p.locked), self.outdated_any(&format!("sheet/{}/", p.id)), cx));
                    n += 1;
                }
                "Props"
            }
        };
        div().flex().flex_col().size_full().child(column_head(title).child(spec(n.to_string()))).child(body)
    }

    // ------------------------------------------------------------ an open item: the detail

    fn detail(&self, d: &Data, open: &Open, window: &Window, cx: &mut Context<Self>) -> Div {
        let (name, sub, status, locked, rejected) = match open {
            Open::Character(id) => match d.bible.character(id) {
                Some(c) => (name_or(&c.name, UNNAMED_CHARACTER).to_string(), role_word(c.role).to_string(), character_status(c, d.mains().contains(&c.id)), root_locked(&c.locked), c.status == Status::Rejected),
                None => return div(),
            },
            Open::Place(id) => match d.bible.place(id) {
                Some(p) => (name_or(&p.name, UNNAMED_PLACE).to_string(), if p.kind == PlaceKind::Exterior { "Outside".to_string() } else { "Inside".to_string() }, place_status(p), root_locked(&p.locked), p.status == Status::Rejected),
                None => return div(),
            },
            Open::Prop(id) => match d.bible.prop(id) {
                Some(p) => (name_or(&p.name, UNNAMED_PROP).to_string(), "Prop".to_string(), prop_status(p), root_locked(&p.locked), p.status == Status::Rejected),
                None => return div(),
            },
        };
        let back_label = match open {
            Open::Character(_) => "All cast",
            Open::Place(_) => "All places",
            Open::Prop(_) => "All props",
        };
        let (o1, o2) = (open.clone(), open.clone());
        let lock = btn("bi-item-lock", if locked { "Locked" } else { "Lock" }, ButtonKind::Ghost)
            .small()
            .icon(if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN })
            .selected(locked)
            .tooltip(if locked { "Locked: building the bible again never changes it. Press to unlock" } else { "Keep it as it is: building the bible again never changes it" })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_item_lock(&o1, !locked, cx)));
        let leave = btn("bi-item-out", if rejected { "Bring back" } else { "Leave out" }, ButtonKind::Ghost)
            .small()
            .icon(if rejected { ph::ARROW_COUNTER_CLOCKWISE } else { ph::PROHIBIT })
            .tooltip(if rejected { "Use it in the film again" } else { "Not part of the film: the approval no longer asks for it" })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_left_out(&o2, !rejected, cx)));
        // A character's drawn sheets waiting for review, approved in one go (asks first).
        let approve_all = match open {
            Open::Character(id) => d.bible.character(id).map(|c| {
                let n = sheets_to_approve(c).len();
                let cid = c.id.clone();
                let b = btn("bi-approve-sheets", "Approve all drawn sheets", ButtonKind::Ghost).small().icon(ph::CHECKS);
                if n == 0 {
                    b.disabled("No drawn sheet is waiting for review")
                } else {
                    b.tooltip(format!("Approve the {} waiting for review, so frames use them as references", count(n, "drawn sheet")))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.confirm_approve_sheets(&cid, window, cx)))
                }
            }),
            _ => None,
        };
        let narrow = crate::widgets::is_narrow(window);
        let head = bar()
            .child(btn("bi-back", back_label, ButtonKind::Ghost).small().icon(ph::ARROW_LEFT).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_item(cx))))
            .child(div().min_w_0().truncate().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name))
            .child(spec(sub))
            .child(status_chip(status))
            .when(self.reviewing, |h| {
                h.child(tag(format!("Reviewing outdated · {} left", self.outdated.len()), Tone::Warning)).child(btn("bi-review-stop", "Stop", ButtonKind::Ghost).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.reviewing = false;
                    cx.notify();
                })))
            })
            .child(div().flex_1())
            .when(narrow, |h| {
                let on = self.drawer;
                h.child(btn("bi-drawer", if on { "Hide panel" } else { "Generate" }, ButtonKind::Ghost).small().icon(ph::SPARKLE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.drawer = !on;
                    cx.notify();
                })))
            })
            .children(approve_all)
            .child(lock)
            .child(leave);
        let mut body = column_body("bi-detail").gap(px(space::LG)).track_scroll(&self.detail_scroll);
        if self.refresh_error.is_some() || self.bible_run().active() {
            if let Some(n) = self.notices_short(cx) {
                body = body.child(n);
            }
        }
        body = match open {
            Open::Character(id) => match d.bible.character(id) {
                Some(c) => self.character_body(d, c, body, window, cx),
                None => body,
            },
            Open::Place(id) => match d.bible.place(id) {
                Some(p) => self.place_body(d, p, body, window, cx),
                None => body,
            },
            Open::Prop(id) => match d.bible.prop(id) {
                Some(p) => self.prop_body(d, p, body, window, cx),
                None => body,
            },
        };
        div().flex().flex_col().size_full().child(head).child(body)
    }

    /// The notices an open item needs (a failed refresh, a build in progress).
    fn notices_short(&self, cx: &mut Context<Self>) -> Option<Div> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(e) = &self.refresh_error {
            out.push(
                error_block(format!("The latest changes could not be read: {e}"))
                    .child(btn("bi-refresh", "Try again", ButtonKind::Secondary).small().icon(ph::ARROW_CLOCKWISE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reload(cx))))
                    .into_any_element(),
            );
        }
        if self.bible_run().active() {
            out.push(info_line(ph::HOURGLASS, LineTone::Info, "The bible is being built again; the floor plan can be changed once it is done.").into_any_element());
        }
        (!out.is_empty()).then(|| div().flex().flex_col().gap(px(space::SM)).children(out))
    }

    fn character_body(&self, d: &Data, c: &Character, body: Stateful<Div>, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let main = d.mains().contains(&c.id);
        let mut kinds = sheets::character_pack(c, &d.bible);
        for s in &c.sheets {
            if s.kind != SheetKind::Unknown && !kinds.contains(&s.kind) {
                kinds.push(s.kind.clone());
            }
        }
        let tiles: Vec<(Target, String, bool)> = kinds
            .iter()
            .map(|k| {
                let needed = main && NEEDED.iter().any(|(n, _)| n == k);
                (Target::Sheet { owner: c.id.clone(), sheet: sheets::sheet_key(k) }, sheet_label(k, &d.bible, &c.outfits), needed)
            })
            .collect();
        let done = tiles.iter().filter(|(t, ..)| card_of(&d.bible, t).is_some_and(approved)).count();
        let body = body.child(titled("Sheets", format!("{done} of {} approved", tiles.len())).child(self.pic_grid(d, &tiles, cx)));
        let body = match &self.pic {
            Some(t) if tiles.iter().any(|(x, ..)| x == t) => body.child(self.viewer(d, t, window, cx)),
            _ => body,
        };
        body.child(self.crops_section(&Open::Character(c.id.clone()), &c.crops, Some(&c.name), cx)).child(self.look_section(d, c, window, cx)).child(self.outfits_section(c, window, cx))
    }

    fn place_body(&self, d: &Data, p: &Place, body: Stateful<Div>, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        // Master and plates.
        let mut tiles: Vec<(Target, String, bool)> = vec![(Target::PlaceSheet { place: p.id.clone() }, "Environment master".into(), true)];
        let states: Vec<String> = if p.states.is_empty() { vec!["day".into()] } else { p.states.iter().map(|s| s.id.clone()).collect() };
        let mut stations = p.stations.clone();
        stations.sort_by_key(|s| !s.overhead);
        let mut rows = div().flex().flex_col().gap(px(space::MD));
        for st in &stations {
            let plate_tiles: Vec<(Target, String, bool)> = states.iter().map(|state| (Target::Plate { place: p.id.clone(), station: st.id.clone(), state: state.clone() }, state_name(p, state), st.overhead)).collect();
            let what = if st.overhead { "Straight down".to_string() } else { format!("Looks {}", compass_word(st.yaw_deg)) };
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(icon_ph(ph::VIDEO_CAMERA, Tokens::text_2(), icon_px::SM))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(station_name(p, &st.id)))
                            .when(st.overhead, |d| d.child(tag("Overhead", Tone::Info)))
                            .child(spec(format!("{} mm", st.lens_mm)))
                            .child(spec(what)),
                    )
                    .child(self.pic_grid(d, &plate_tiles, cx)),
            );
            // The picked plate opens right under its own station, with its Approve / Reject /
            // Lock row in reach, not at the end of every station.
            if let Some(t) = self.pic.as_ref().filter(|t| plate_tiles.iter().any(|(x, ..)| x == *t)) {
                rows = rows.child(self.viewer(d, t, window, cx));
            }
            tiles.extend(plate_tiles);
        }
        let done = tiles.iter().filter(|(t, ..)| card_of(&d.bible, t).is_some_and(approved)).count();
        let master_tile = vec![tiles[0].clone()];
        // The floor plan is the centre of a place: first, where it is seen without scrolling.
        let mut body = body;
        if let Some(plan) = &self.plan {
            body = body.child(card().child(plan.clone()));
        }
        let mut body = body.child(titled("Pictures", format!("{done} of {} approved", tiles.len())).child(self.pic_grid(d, &master_tile, cx)));
        if let Some(t) = self.pic.as_ref().filter(|t| matches!(t, Target::PlaceSheet { .. })) {
            body = body.child(self.viewer(d, t, window, cx));
        }
        body = body.child(section("Camera stations and background plates").child(info_line(ph::INFO, LineTone::Neutral, "One background per station and light state, reused by every shot from that station, so the place never changes between shots.")).child(rows));
        body.child(self.place_about(p, window, cx)).child(self.crops_section(&Open::Place(p.id.clone()), &p.crops, None, cx))
    }

    fn prop_body(&self, d: &Data, p: &Prop, body: Stateful<Div>, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut tiles: Vec<(Target, String, bool)> = vec![(Target::Sheet { owner: p.id.clone(), sheet: "master".into() }, "Prop sheet".into(), false)];
        if let Some(owner) = p.owner.as_deref().and_then(|o| d.bible.character(o)).filter(|_| p.held) {
            tiles.push((Target::Sheet { owner: owner.id.clone(), sheet: sheets::sheet_key(&SheetKind::Grip(p.id.clone())) }, format!("Grip · {}", name_or(&owner.name, UNNAMED_CHARACTER)), false));
        }
        let body = body.child(section("Sheets").child(self.pic_grid(d, &tiles, cx)));
        let body = match &self.pic {
            Some(t) if tiles.iter().any(|(x, ..)| x == t) => body.child(self.viewer(d, t, window, cx)),
            _ => body,
        };
        body.child(self.prop_about(d, p, window, cx)).child(self.crops_section(&Open::Prop(p.id.clone()), &p.crops, None, cx))
    }

    /// A tidy grid of picture tiles; the selected one is outlined.
    fn pic_grid(&self, d: &Data, tiles: &[(Target, String, bool)], cx: &mut Context<Self>) -> Div {
        let mut grid = div().flex().flex_wrap().gap(px(space::MD));
        for (t, label, needed) in tiles {
            grid = grid.child(self.pic_tile(d, t, label, *needed, cx));
        }
        grid
    }

    fn pic_tile(&self, d: &Data, t: &Target, label: &str, needed: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = tkey(t);
        let card = card_of(&d.bible, t);
        let path = card.and_then(|c| c.selected.as_deref()).and_then(|id| d.files.get(id)).cloned();
        let has = card.is_some_and(|c| c.selected.is_some());
        let busy = self.pic_busy(t);
        let selected = self.pic.as_ref() == Some(t);
        let status = card.map(|c| c.status).unwrap_or_default();
        let locked = card.is_some_and(|c| c.locked);
        let outdated = self.outdated.contains(&key);
        let is_ok = card.is_some_and(approved);
        let pic = picture(path, Some(SHEET_W), SHEET_H, true).when(!has, |p| p.child(no_picture("Nothing drawn yet"))).children(busy_marks(&key, &busy, self.web_held));
        let caption = div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::XS))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(label.to_string()))
                    .when(needed && !is_ok, |d| d.child(tag("Needed", Tone::Warning))),
            )
            .child(card_marks(if has { status } else { Status::Draft }, locked, outdated));
        let (pick, drop) = (t.clone(), t.clone());
        selectable(eid("bi-pic", &key), selected)
            .relative()
            .w(px(SHEET_W))
            .overflow_hidden()
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
            .on_drop(cx.listener(move |this, p: &ExternalPaths, _, cx| this.upload(drop.clone(), p.paths().to_vec(), cx)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.select_pic(pick.clone(), cx)))
            .child(pic)
            .child(caption)
    }

    /// The selected picture large, with its actions, its outdated state, its latest options and
    /// its history.
    fn viewer(&self, d: &Data, t: &Target, _window: &Window, cx: &mut Context<Self>) -> Div {
        let key = tkey(t);
        let card_now = card_of(&d.bible, t).cloned().unwrap_or_default();
        let busy = self.pic_busy(t);
        let prepared = self.prepared.as_ref().filter(|p| &p.target == t);
        let still = card_now.selected.as_deref().and_then(|id| d.files.get(id).cloned().or_else(|| prepared.and_then(|p| p.round.iter().find(|(r, _)| r == id)).and_then(|(_, p)| p.clone())));
        let has = card_now.selected.is_some();
        let outdated = self.outdated.contains(&key);
        let approved_now = approved(&card_now);
        let rejected = card_now.status == Status::Rejected;
        let locked = card_now.locked;
        let (a, b, c, u) = (t.clone(), t.clone(), t.clone(), t.clone());
        let approve = btn("bi-v-approve", if approved_now { "Approved" } else { "Approve" }, ButtonKind::Secondary).small().icon(ph::CHECK).selected(approved_now);
        // Approve only ever approves and shows "Approved" in its place (a double click leaves it
        // approved); taking an approval back is its own small button on the other side of the
        // row, never under the pointer.
        let approve = if !has {
            approve.disabled("Make or upload a picture first")
        } else if approved_now {
            approve.tooltip("The film uses this picture")
        } else {
            approve.tooltip("Use this picture in the film").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(a.clone(), Status::Approved, cx)))
        };
        let back = approved_now.then(|| {
            let t = t.clone();
            btn("bi-v-unapprove", "Back to review", ButtonKind::Ghost)
                .small()
                .icon(ph::ARROW_COUNTER_CLOCKWISE)
                .tooltip("Take the approval back: the picture waits for your review again")
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(t.clone(), Status::NeedsReview, cx)))
        });
        let reject = btn("bi-v-reject", if rejected { "Rejected" } else { "Reject" }, ButtonKind::Ghost).small().icon(ph::X).selected(rejected);
        let reject = if !has {
            reject.disabled("There is no picture to reject yet")
        } else {
            reject.tooltip(if rejected { "Press again to review it later" } else { "Not usable: draw it again or upload your own" }).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_status(b.clone(), if rejected { Status::NeedsReview } else { Status::Rejected }, cx)))
        };
        let lock = btn("bi-v-lock", if locked { "Locked" } else { "Lock" }, ButtonKind::Ghost)
            .small()
            .icon(if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN })
            .selected(locked)
            .tooltip(if locked { "Locked: never marked outdated and never changed automatically. Press to unlock" } else { "Keep this picture as it is" })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_locked(c.clone(), !locked, cx)));
        let upload = btn("bi-v-upload", "Upload your own", ButtonKind::Ghost)
            .small()
            .icon(ph::UPLOAD_SIMPLE)
            .tooltip("Use your own art for this picture (or drop a file on it)")
            .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.pick_upload(u.clone(), w, cx)));
        let is_master = matches!(t, Target::Sheet { owner, sheet } if sheet == "master" && d.bible.character(owner).is_some());
        let options = is_master.then(|| {
            let t = t.clone();
            let queued = matches!(busy, Busy::Waiting { .. } | Busy::Working { .. });
            let b = btn("bi-v-options", "Design options", ButtonKind::Secondary).small().icon(ph::SPARKLE);
            if queued {
                b.disabled("Already being drawn")
            } else {
                b.tooltip(format!("Draw {} different designs from the manga to choose from", sheets::MASTER_OPTIONS)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.enqueue_pic(&t, sheets::MASTER_OPTIONS, cx)))
            }
        });
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::SM))
            .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(pic_title(&d.bible, t)))
            .child(card_marks(card_now.status, locked, outdated))
            .children(back)
            .child(div().flex_1())
            .children(options)
            .child(approve)
            .child(reject)
            .child(lock)
            .child(upload)
            .child(history_button("bi-v-history", self.history_open).small().on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.history_open = !this.history_open;
                cx.notify();
            })));
        let drop = t.clone();
        // Every sheet at its own shape (a 3:4 design, a 21:9 turnaround), never cut to 16:9.
        let (still_w, still_h) = still_box(&card_now.gen.aspect);
        let frame = div()
            .id(eid("bi-still", &key))
            .relative()
            .w(px(still_w))
            .max_w(relative(1.))
            .h(px(still_h))
            .rounded(px(radius::LG))
            .overflow_hidden()
            .bg(Tokens::well())
            .border_1()
            .border_color(Tokens::border())
            .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
            .on_drop(cx.listener(move |this, p: &ExternalPaths, _, cx| this.upload(drop.clone(), p.paths().to_vec(), cx)))
            .map(|f| match still {
                Some(p) => f.child(img(mv_ui::thumbs::display_source(p)).size_full().object_fit(ObjectFit::Contain)),
                None => f
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(space::SM))
                    .p(px(space::LG))
                    .child(ph_icon(ph::IMAGE, Weight::Duotone, Tokens::text_3(), icon_px::HERO))
                    .child(div().text_size(px(size::MD)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child("Nothing drawn yet"))
                    .child(div().max_w(px(TILE_W * 1.5)).text_center().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Press Generate in the panel to draw it from the engine's prompt and references, or drop your own art here.")),
            })
            .children(busy_marks(&format!("v-{key}"), &busy, self.web_held));
        let mut out = card().child(head);
        // Sheets lead the frames only once approved.
        if matches!(t, Target::Sheet { .. }) && has && card_now.status == Status::NeedsReview {
            out = out.child(info_line(ph::INFO, LineTone::Neutral, "Waiting for review: frames do not use this sheet as a reference until you approve it."));
        }
        out = out.child(div().flex().justify_center().child(frame));
        if outdated {
            out = out.child(self.outdated_block(t, &busy, cx));
        }
        if let Some(round) = prepared.and_then(|p| self.round(t, p, &card_now, cx)) {
            out = out.child(round);
        }
        if self.history_open && self.history_key.as_deref() == Some(key.as_str()) {
            if let Some(h) = &self.history {
                out = out.child(h.clone());
            }
        }
        if card_now.gen.refs_user {
            let t = t.clone();
            out = out.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XS))
                    .child(info_line(ph::INFO, LineTone::Neutral, "You chose this picture's references by hand, so the engine no longer picks them."))
                    .child(div().child(btn("bi-ai-refs", "Let the engine pick them again", ButtonKind::Ghost).small().icon(ph::ARROW_COUNTER_CLOCKWISE).tooltip("The references you pinned stay").on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| this.change("The references could not be reset", card_op(t.clone(), |c| c.gen.refs_user = false), cx),
                    )))),
            );
        }
        out
    }

    fn outdated_block(&self, t: &Target, busy: &Busy, cx: &mut Context<Self>) -> Div {
        let (u, k) = (t.clone(), t.clone());
        let queued = matches!(busy, Busy::Waiting { .. } | Busy::Working { .. });
        let update = btn("bi-od-update", "Update", ButtonKind::Secondary).small().icon(ph::SPARKLE);
        let update = if queued { update.disabled("Already being drawn again") } else { update.tooltip("Draw it again with what changed; this picture stays in Earlier versions").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.update_pic(u.clone(), cx))) };
        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .p(px(space::MD))
            .rounded(px(radius::LG))
            .bg(Tokens::warning_soft())
            .border_1()
            .border_color(Tokens::warning_border())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::SM))
                    .child(div().pt(px(1.)).child(icon_ph(ph::ARROWS_CLOCKWISE, Tokens::warning(), icon_px::SM)))
                    .child(div().flex_1().min_w_0().text_size(px(size::SM)).line_height(px(18.)).text_color(Tokens::text_1()).whitespace_normal().child(if queued {
                        "Outdated: it is being drawn again with what changed. Choose the new picture when it arrives."
                    } else {
                        "Outdated: its prompt, a reference or the style changed after this picture was made."
                    })),
            )
            .child(div().flex().gap(px(space::SM)).child(update).child(btn("bi-od-keep", "Keep", ButtonKind::Ghost).small().icon(ph::LOCK_SIMPLE).tooltip("Keep this picture and lock it, so it is no longer marked outdated").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.keep_pic(k.clone(), cx)))))
    }

    /// The latest round of options, side by side, to choose from.
    fn round(&self, t: &Target, p: &Prepared, c: &Card, cx: &mut Context<Self>) -> Option<Div> {
        let selected = c.selected.as_deref();
        let worth = p.round.len() > 1 || p.round.first().is_some_and(|(id, _)| Some(id.as_str()) != selected);
        if !worth {
            return None;
        }
        let mut tiles = div().flex().flex_wrap().gap(px(space::MD));
        for (k, (take, path)) in p.round.iter().enumerate() {
            let in_use = selected == Some(take.as_str());
            let group = SharedString::from(format!("bi-var-{k}"));
            let (pick, ok, t1, t2) = (take.clone(), take.clone(), t.clone(), t.clone());
            tiles = tiles.child(
                selectable(("bi-var", k), in_use)
                    .relative()
                    .group(group.clone())
                    .w(px(SHEET_W))
                    .overflow_hidden()
                    .tooltip(tip(if in_use { "In use" } else { "Use this one" }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(t1.clone(), pick.clone(), false, cx)))
                    .child(picture(path.clone(), Some(SHEET_W), SHEET_H, true))
                    .child(div().flex().items_center().h(px(36.)).px(px(space::SM)).map(|d| if in_use { d.child(tag("In use", Tone::Accent)) } else { d.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child(format!("Option {}", k + 1))) }))
                    .child(hover_actions(group).p(px(space::XXS)).rounded(px(radius::MD)).bg(Tokens::popover()).child(icon_button(("bi-var-ok", k), ph::CHECK, if in_use { "Approve this one" } else { "Use this one and approve it" }).small().on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.choose(t2.clone(), ok.clone(), true, cx);
                        },
                    )))),
            );
        }
        Some(div().flex().flex_col().gap(px(space::SM)).child(label_row("Latest options", "Click one to use it")).child(tiles))
    }

    /// Reference pictures cut from the manga: the ones in use, then suggestions to accept or drop.
    fn crops_section(&self, open: &Open, crops: &[RefCrop], who: Option<&str>, cx: &mut Context<Self>) -> Div {

        let used: Vec<&RefCrop> = crops.iter().filter(|c| mv_anime::crops::usable(c)).collect();
        let suggested: Vec<&RefCrop> = crops.iter().filter(|c| !mv_anime::crops::usable(c)).collect();
        let tile = |r: &RefCrop, ok: bool, k: usize, cx: &mut Context<Self>| -> Stateful<Div> {
            let group = SharedString::from(format!("bi-crop-{}-{k}", if ok { "u" } else { "s" }));
            let (o1, o2, t1, t2) = (open.clone(), open.clone(), r.take.clone(), r.take.clone());
            let mut actions = hover_actions(group.clone()).p(px(space::XXS)).rounded(px(radius::MD)).bg(Tokens::popover());
            if !ok {
                actions = actions.child(icon_button(eid("bi-crop-ok", &r.take), ph::CHECK, "Use as a reference").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.accept_crop(&o1, t1.clone(), cx))));
            }
            actions = actions.child(icon_button(eid("bi-crop-x", &r.take), ph::X, if ok { "Stop using it" } else { "Drop this suggestion" }).small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.drop_crop(&o2, t2.clone(), cx))));
            let page = match &r.source {
                s if !s.chapter_id.is_empty() => format!("Page {}{}", s.page + 1, s.panel.map(|p| format!(", panel {}", p + 1)).unwrap_or_default()),
                _ => "Your picture".to_string(),
            };
            div()
                .id(eid("bi-crop", &r.take))
                .relative()
                .group(group)
                .flex_none()
                .rounded(px(radius::MD))
                .overflow_hidden()
                .border_1()
                .border_color(if ok { Tokens::border_strong() } else { Tokens::border() })
                .when(!ok, |d| d.border_dashed())
                .tooltip(tip(page))
                .child(div().when(!ok, |d| d.opacity(0.7)).child(thumb(Some(self.store.path(&r.file)), CROP, CROP)))
                .child(actions)
        };
        let mut used_row = div().flex().flex_wrap().gap(px(space::SM));
        for (k, r) in used.iter().enumerate() {
            used_row = used_row.child(tile(r, true, k, cx));
        }
        let up = open.clone();
        used_row = used_row.child(
            div()
                .id(eid("bi-crop-add", &format!("{open:?}")))
                .flex()
                .flex_col()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(px(space::XXS))
                .size(px(CROP))
                .rounded(px(radius::MD))
                .border_1()
                .border_dashed()
                .border_color(Tokens::border_strong())
                .cursor_pointer()
                .hover(|s| s.bg(Tokens::hover()))
                .tooltip(tip("Add your own reference pictures (or drop them here)"))
                .drag_over::<ExternalPaths>(|s, _, _, _| s.border_color(Tokens::accent()).bg(Tokens::accent_soft()))
                .on_drop(cx.listener({
                    let open = open.clone();
                    move |this, p: &ExternalPaths, _, cx| this.upload_crops(open.clone(), p.paths().to_vec(), cx)
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.pick_crop_upload(up.clone(), w, cx)))
                .child(icon_ph(ph::UPLOAD_SIMPLE, Tokens::text_2(), icon_px::SM))
                .child(div().text_size(px(size::XS)).text_color(Tokens::text_2()).child("Upload")),
        );
        let mut s = titled("From the manga", format!("{} in use · {} to check", used.len(), suggested.len()));
        if let (Open::Character(id), Some(name)) = (open, who) {
            let run = job_run(&self.jobs, &self.ctx.project, |k| matches!(k, JobKind::Crops { character } if character == id));
            let (id, name) = (id.clone(), name_or(name, UNNAMED_CHARACTER).to_string());
            let find = btn(eid("bi-find-crops", &id), "Find in the manga again", ButtonKind::Ghost).small().icon(ph::CROP).loading(run.active());
            let find = if run.active() { find } else { find.tooltip("Cut new reference pictures from every read chapter; the ones you accepted stay").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.find_crops(&id, &name, cx))) };
            s = s.child(div().flex().items_center().gap(px(space::SM)).child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("A picture is used as a reference only when two signals agree or you accept it.")).child(find));
            if let Run::Failed { error, .. } = run {
                s = s.child(error_block(format!("The reference pictures could not be cut: {error}")));
            }
        }
        s = s.child(used_row);
        if !suggested.is_empty() {
            let mut sug = div().flex().flex_wrap().gap(px(space::SM));
            for (k, r) in suggested.iter().enumerate() {
                sug = sug.child(tile(r, false, k, cx));
            }
            s = s.child(label_caps("Suggestions")).child(sug);
        }
        s
    }

    /// One editable text, as a text box while it is open.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, key: &str, value: &str, placeholder: &'static str, multi: bool, ro: bool, apply: TextOp, check: fn(&str) -> Option<&'static str>, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(e) = self.editing.as_ref().filter(|e| e.key == key) {
            return editor(&e.input, window, cx).into_any_element();
        }
        if ro {
            let empty = value.trim().is_empty();
            return div()
                .flex_1()
                .min_w_0()
                .px(px(space::XS))
                .py(px(2.))
                .text_size(px(size::SM))
                .line_height(px(18.))
                .text_color(if empty { Tokens::text_3() } else { Tokens::text_1() })
                .whitespace_normal()
                .child(if empty { "—".to_string() } else { value.trim().to_string() })
                .into_any_element();
        }
        let (k, v) = (key.to_string(), value.to_string());
        editable(eid("bi-text", key), value, placeholder, multi).on_click(cx.listener(move |this, _: &ClickEvent, w, cx| this.edit_text(k.clone(), v.clone(), placeholder, multi, apply.clone(), check, w, cx))).into_any_element()
    }

    fn look_section(&self, d: &Data, c: &Character, window: &Window, cx: &mut Context<Self>) -> Div {
        let ro = root_locked(&c.locked);
        let id = c.id.clone();
        let i =d.bible.characters.iter().position(|x| x.id == c.id).unwrap_or(0);
        let mut roles = div().flex().flex_wrap().gap(px(space::XS));
        for (k, r) in [Role::Hero, Role::Ally, Role::Threat, Role::Neutral].into_iter().enumerate() {
            let on = c.role == r;
            let id = c.id.clone();
            roles = roles.child(opt_chip(("bi-role", k), role_word(r), on).when(!ro && !on, |chip| {
                chip.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.change(
                        "The role could not be changed",
                        char_op(&id, move |c| {
                            c.role = r;
                            c.locked.insert("/role".into());
                        }),
                        cx,
                    )
                }))
            }));
        }
        let colour = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(colour_dot(&c.look.signature_colour, i, 16.))
            .child(self.text(
                &format!("{}-colour", c.id),
                &c.look.signature_colour,
                "#RRGGBB",
                false,
                ro,
                {
                    let id = id.clone();
                    top(move |b, text| {
                        if let (Some(c), Some(v)) = (b.characters.iter_mut().find(|c| c.id == id), parse_hex(text)) {
                            c.look.signature_colour = hex_text(v);
                            c.locked.insert("/look/signature_colour".into());
                        }
                    })
                },
                |t| parse_hex(t).is_none().then_some("Type a colour like #3A6FF2."),
                window,
                cx,
            ));
        let height_text = if c.look.height_m > 0. { format!("{:.2} m", c.look.height_m) } else { String::new() };
        let height = self.text(
            &format!("{}-height", c.id),
            &height_text,
            "1.75 m",
            false,
            ro,
            {
                let id = id.clone();
                top(move |b, text| {
                    if let (Some(c), Some(v)) = (b.characters.iter_mut().find(|c| c.id == id), parse_height(text)) {
                        c.look.height_m = v;
                        c.locked.insert("/look/height_m".into());
                    }
                })
            },
            |t| parse_height(t).is_none().then_some("Type a height like 1.75 m or 175 cm."),
            window,
            cx,
        );
        section("Look")
            .child(inspector_row("Role", roles))
            .child(inspector_row("About", self.text(&format!("{}-desc", c.id), &c.description, "Who they are, in a few lines", true, ro, look_field(&id, "/description", |c| &mut c.description), no_check, window, cx)))
            .child(inspector_row("Hair", self.text(&format!("{}-hair", c.id), &c.look.hair, "Colour, length, style", false, ro, look_field(&id, "/look/hair", |c| &mut c.look.hair), no_check, window, cx)))
            .child(inspector_row("Eyes", self.text(&format!("{}-eyes", c.id), &c.look.eyes, "Colour and shape", false, ro, look_field(&id, "/look/eyes", |c| &mut c.look.eyes), no_check, window, cx)))
            .child(inspector_row("Build", self.text(&format!("{}-build", c.id), &c.look.build, "Body type", false, ro, look_field(&id, "/look/build", |c| &mut c.look.build), no_check, window, cx)))
            .child(inspector_row("Outfit", self.text(&format!("{}-outfit", c.id), &c.look.default_outfit, "What they usually wear", true, ro, look_field(&id, "/look/default_outfit", |c| &mut c.look.default_outfit), no_check, window, cx)))
            .child(inspector_row("Height", height))
            .child(inspector_row("Colour", colour))
            .child(self.keep_row(c, window, cx))
    }

    /// The must-keep features: chips with a remove button, and a box to add one.
    fn keep_row(&self, c: &Character, window: &Window, cx: &mut Context<Self>) -> Div {
        let ro = root_locked(&c.locked);
        let mut chips = div().flex().flex_wrap().gap(px(space::XS));
        for (k, word) in c.look.keep.iter().enumerate() {
            let (id, w) = (c.id.clone(), word.clone());
            chips = chips.child(word_chip(word).when(!ro, |chip| {
                chip.child(icon_button(("bi-keep-x", k), ph::X, "Remove").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let w = w.clone();
                    this.change(
                        "The feature could not be removed",
                        char_op(&id, move |c| {
                            c.look.keep.retain(|x| *x != w);
                            c.locked.insert("/look/keep".into());
                        }),
                        cx,
                    )
                })))
            }));
        }
        let id = c.id.clone();
        let add = (!ro).then(|| {
            self.text(
                &format!("{}-keep-new", c.id),
                "",
                "Add a feature every picture must keep",
                false,
                false,
                top(move |b, text| {
                    if text.is_empty() {
                        return;
                    }
                    if let Some(c) = b.characters.iter_mut().find(|c| c.id == id) {
                        if !c.look.keep.iter().any(|k| k.eq_ignore_ascii_case(text)) {
                            c.look.keep.push(text.to_string());
                        }
                        c.locked.insert("/look/keep".into());
                    }
                }),
                no_check,
                window,
                cx,
            )
        });
        let mut col = div().flex().flex_col().gap(px(space::XS));
        if c.look.keep.is_empty() {
            col = col.child(div().px(px(space::XS)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("Nothing yet: a scar, a hairpin, a colour of the eyes."));
        } else {
            col = col.child(chips);
        }
        inspector_row("Keep", col.children(add))
    }

    fn outfits_section(&self, c: &Character, window: &Window, cx: &mut Context<Self>) -> Div {
        let ro = root_locked(&c.locked);
        let mut rows = div().flex().flex_col().gap(px(space::SM));
        for (k, o) in c.outfits.iter().enumerate() {
            let (cid, oid) = (c.id.clone(), o.id.clone());
            let name_op = top(move |b, text| {
                if text.is_empty() {
                    return;
                }
                if let Some(c) = b.characters.iter_mut().find(|c| c.id == cid) {
                    if let Some(o) = c.outfits.iter_mut().find(|o| o.id == oid) {
                        o.name = text.to_string();
                    }
                    c.locked.insert("/outfits".into());
                }
            });
            let (cid, oid) = (c.id.clone(), o.id.clone());
            let desc_op = top(move |b, text| {
                if let Some(c) = b.characters.iter_mut().find(|c| c.id == cid) {
                    if let Some(o) = c.outfits.iter_mut().find(|o| o.id == oid) {
                        o.description = text.to_string();
                    }
                    c.locked.insert("/outfits".into());
                }
            });
            let (cid, oid) = (c.id.clone(), o.id.clone());
            // Its outfit sheet goes with it; one with a picture asks first.
            let drawn = c.sheets.iter().any(|s| s.kind == SheetKind::Outfit(o.id.clone()) && s.card.selected.is_some());
            let label = name_or(&o.name, "this outfit").to_string();
            let remove = (!ro).then(|| {
                icon_button(("bi-outfit-x", k), ph::TRASH, "Remove this outfit").small().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    let (cid, oid) = (cid.clone(), oid.clone());
                    let go = move |this: &mut BibleStep, cx: &mut Context<BibleStep>| {
                        let oid = oid.clone();
                        this.change(
                            "The outfit could not be removed",
                            char_op(&cid, move |c| {
                                c.outfits.retain(|o| o.id != oid);
                                c.sheets.retain(|s| s.kind != SheetKind::Outfit(oid.clone()));
                                c.locked.insert("/outfits".into());
                            }),
                            cx,
                        )
                    };
                    if !drawn {
                        go(this, cx);
                        return;
                    }
                    let weak = cx.entity().downgrade();
                    let go = Rc::new(go);
                    confirm_dialog(format!("Remove {label}?"), "Its outfit sheet goes with it. Its pictures stay in the project but are no longer shown.", "Remove", true)
                        .icon(ph::TRASH)
                        .on_confirm(move |_w, cx| {
                            let go = go.clone();
                            let _ = weak.update(cx, |this, cx| go(this, cx));
                        })
                        .open(window, cx);
                }))
            });
            let chapters = if o.chapters.is_empty() { None } else { Some(spec(format!("Chapters {}", o.chapters.join(", ")))) };
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::XXS))
                    .p(px(space::SM))
                    .rounded(px(radius::MD))
                    .bg(Tokens::surface_2())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(self.text(&format!("{}-of-{}-name", c.id, o.id), &o.name, "Outfit name", false, ro, name_op, no_check, window, cx)))
                            .children(chapters)
                            .children(remove),
                    )
                    .child(self.text(&format!("{}-of-{}-desc", c.id, o.id), &o.description, "What it looks like", true, ro, desc_op, no_check, window, cx)),
            );
        }
        let cid = c.id.clone();
        let add = btn("bi-outfit-add", "Add outfit", ButtonKind::Ghost).small().icon(ph::PLUS);
        let add = if ro {
            add.disabled("This character is locked")
        } else {
            add.tooltip("Another outfit or form; it gets its own sheet").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.change(
                    "The outfit could not be added",
                    char_op(&cid, |c| {
                        let taken: HashSet<String> = c.outfits.iter().map(|o| o.id.clone()).collect();
                        let id = (1..).map(|n| format!("of_{n}")).find(|id| !taken.contains(id)).unwrap_or_else(|| "of_new".into());
                        c.outfits.push(Outfit { id, name: format!("Outfit {}", c.outfits.len() + 1), ..Default::default() });
                        c.locked.insert("/outfits".into());
                    }),
                    cx,
                )
            }))
        };
        let head = div().flex().items_center().gap(px(space::SM)).child(label_caps("Outfits")).child(div().flex_1()).child(add);
        let mut s = div().flex().flex_col().gap(px(space::SM)).child(head);
        if c.outfits.is_empty() {
            s = s.child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("One outfit: the one in the look above. Add more when the story changes clothes."));
        } else {
            s = s.child(rows).child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).child("The first outfit is drawn on the main sheets; every other one gets its own outfit sheet."));
        }
        s
    }

    fn place_about(&self, p: &Place, window: &Window, cx: &mut Context<Self>) -> Div {
        let ro = root_locked(&p.locked);
        let id = p.id.clone();
        let mut kinds = div().flex().gap(px(space::XS));
        for (k, (kind, word)) in [(PlaceKind::Interior, "Inside"), (PlaceKind::Exterior, "Outside")].into_iter().enumerate() {
            let on = p.kind == kind;
            let id = id.clone();
            kinds = kinds.child(opt_chip(("bi-pkind", k), word, on).when(!ro && !on, |chip| {
                chip.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.change(
                        "The place could not be changed",
                        place_op(&id, move |p| {
                            p.kind = kind;
                            p.locked.insert("/kind".into());
                        }),
                        cx,
                    )
                }))
            }));
        }
        let name_op = {
            let id = id.clone();
            top(move |b, text| {
                if text.is_empty() {
                    return;
                }
                if let Some(p) = b.places.iter_mut().find(|p| p.id == id) {
                    p.name = text.to_string();
                    p.locked.insert("/name".into());
                }
            })
        };
        let desc_op = {
            let id = id.clone();
            top(move |b, text| {
                if let Some(p) = b.places.iter_mut().find(|p| p.id == id) {
                    p.description = text.to_string();
                    p.locked.insert("/description".into());
                }
            })
        };
        let mut fixed = div().flex().flex_wrap().gap(px(space::XS));
        for l in &p.plan.landmarks {
            fixed = fixed.child(spec(format!("{} · {}", name_or(&l.name, "Unnamed landmark"), super::floorplan::kind_label(l.kind))));
        }
        let forces = p.extra.get(mv_anime::bible::EXTRA_FORCES);
        let wind = forces.and_then(|f| f.get("wind")).and_then(|w| w.as_str()).filter(|w| !w.trim().is_empty()).map(|w| w.trim().to_string());
        let moving: Vec<String> = forces.and_then(|f| f.get("moving")).and_then(|m| m.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string()).collect()).unwrap_or_default();
        let mut s = section("About this place")
            .child(inspector_row("Name", self.text(&format!("{}-name", p.id), &p.name, "Place name", false, ro, name_op, no_check, window, cx)))
            .child(inspector_row("Kind", kinds))
            .child(inspector_row("About", self.text(&format!("{}-desc", p.id), &p.description, "What the place looks like, what never changes", true, ro, desc_op, no_check, window, cx)))
            .child(inspector_row("Fixed", if p.plan.landmarks.is_empty() { div().pt(px(5.)).text_size(px(size::XS)).text_color(Tokens::text_3()).child("No landmarks yet: add them on the floor plan.") } else { fixed }));
        if wind.is_some() || !moving.is_empty() {
            let mut always = Vec::new();
            if let Some(w) = wind {
                always.push(format!("wind: {w}"));
            }
            always.extend(moving);
            s = s.child(inspector_row("Always", div().pt(px(5.)).text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_normal().child(always.join(" · "))));
        }
        s
    }

    fn prop_about(&self, d: &Data, p: &Prop, window: &Window, cx: &mut Context<Self>) -> Div {
        let ro = root_locked(&p.locked);
        let id = p.id.clone();
        let name_op = {
            let id = id.clone();
            top(move |b, text| {
                if text.is_empty() {
                    return;
                }
                if let Some(p) = b.props.iter_mut().find(|p| p.id == id) {
                    p.name = text.to_string();
                    p.locked.insert("/name".into());
                }
            })
        };
        let desc_op = {
            let id = id.clone();
            top(move |b, text| {
                if let Some(p) = b.props.iter_mut().find(|p| p.id == id) {
                    p.description = text.to_string();
                    p.locked.insert("/description".into());
                }
            })
        };
        // Who holds it.
        let owner_name = p.owner.as_deref().and_then(|o| d.bible.character(o)).map(|c| name_or(&c.name, UNNAMED_CHARACTER).to_string());
        let mut items: Vec<MenuItem> = Vec::new();
        let weak = cx.entity().downgrade();
        {
            let (weak, id) = (weak.clone(), id.clone());
            items.push(
                MenuItem::new("Nobody", move |_, cx| {
                    let id = id.clone();
                    let _ = weak.update(cx, |this, cx| {
                        this.change(
                            "The owner could not be changed",
                            prop_op(&id, |p| {
                                p.owner = None;
                                p.locked.insert("/owner".into());
                            }),
                            cx,
                        )
                    });
                })
                .checked(p.owner.is_none()),
            );
        }
        for c in d.bible.characters.iter().filter(|c| c.status != Status::Rejected) {
            let (weak, id, cid) = (weak.clone(), id.clone(), c.id.clone());
            items.push(
                MenuItem::new(name_or(&c.name, UNNAMED_CHARACTER).to_string(), move |_, cx| {
                    let (id, cid) = (id.clone(), cid.clone());
                    let _ = weak.update(cx, |this, cx| {
                        this.change(
                            "The owner could not be changed",
                            prop_op(&id, move |p| {
                                p.owner = Some(cid.clone());
                                p.locked.insert("/owner".into());
                            }),
                            cx,
                        )
                    });
                })
                .checked(p.owner.as_deref() == Some(c.id.as_str())),
            );
        }
        let owner: AnyElement = if ro {
            div().pt(px(5.)).text_size(px(size::SM)).text_color(Tokens::text_1()).child(owner_name.unwrap_or_else(|| "Nobody".into())).into_any_element()
        } else {
            menu_button("bi-prop-owner", ph::USER, "Who it belongs to", items).label(owner_name.unwrap_or_else(|| "Nobody".into())).small().into_any_element()
        };
        let mut held = div().flex().gap(px(space::XS));
        for (k, (v, word)) in [(true, "In hand"), (false, "Not held")].into_iter().enumerate() {
            let on = p.held == v;
            let id = id.clone();
            held = held.child(opt_chip(("bi-held", k), word, on).when(!ro && !on, |chip| {
                chip.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.change(
                        "The prop could not be changed",
                        prop_op(&id, move |p| {
                            p.held = v;
                            p.locked.insert("/held".into());
                        }),
                        cx,
                    )
                }))
            }));
        }
        let mut states = div().flex().flex_wrap().gap(px(space::XS));
        for (k, st) in p.states.iter().enumerate() {
            let (id, s2) = (id.clone(), st.clone());
            states = states.child(word_chip(st).when(!ro, |chip| {
                chip.child(icon_button(("bi-pstate-x", k), ph::X, "Remove").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let s2 = s2.clone();
                    this.change(
                        "The state could not be removed",
                        prop_op(&id, move |p| {
                            p.states.retain(|x| *x != s2);
                            p.locked.insert("/states".into());
                        }),
                        cx,
                    )
                })))
            }));
        }
        let add_state = (!ro).then(|| {
            let id = id.clone();
            self.text(
                &format!("{}-state-new", p.id),
                "",
                "Add a state (sheathed, drawn, broken)",
                false,
                false,
                top(move |b, text| {
                    if text.is_empty() {
                        return;
                    }
                    if let Some(p) = b.props.iter_mut().find(|p| p.id == id) {
                        if !p.states.iter().any(|s| s.eq_ignore_ascii_case(text)) {
                            p.states.push(text.to_string());
                        }
                        p.locked.insert("/states".into());
                    }
                }),
                no_check,
                window,
                cx,
            )
        });
        section("About this prop")
            .child(inspector_row("Name", self.text(&format!("{}-name", p.id), &p.name, "Prop name", false, ro, name_op, no_check, window, cx)))
            .child(inspector_row("About", self.text(&format!("{}-desc", p.id), &p.description, "What it looks like", true, ro, desc_op, no_check, window, cx)))
            .child(inspector_row("Owner", owner))
            .child(inspector_row("Held", held))
            .child(inspector_row("States", div().flex().flex_col().gap(px(space::XS)).when(!p.states.is_empty(), |d| d.child(states)).children(add_state)))
    }

    // ------------------------------------------------------------ style

    fn style_view(&self, d: &Data, window: &Window, cx: &mut Context<Self>) -> Div {
        let st = &d.bible.style;
        let ro = root_locked(&st.locked);
        let mut body = column_body("bi-style").gap(px(space::LG)).track_scroll(&self.body_scroll);
        if let Some(n) = self.notices(d, cx) {
            body = body.child(n);
        }
        if crate::widgets::is_narrow(window) {
            let on = self.drawer;
            body = body.child(div().flex().justify_end().child(btn("bi-style-drawer", if on { "Hide panel" } else { "Generate" }, ButtonKind::Ghost).small().icon(ph::SPARKLE).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.drawer = !on;
                cx.notify();
            }))));
        }
        body = body.child(self.viewer(d, &Target::StyleFrame, window, cx));
        let line = self.text(
            "style-line",
            &st.line,
            "How every picture should look: lines, shading, backgrounds, light",
            true,
            ro,
            top(|b, text| {
                if !text.is_empty() {
                    b.style.line = text.to_string();
                    b.style.locked.insert("/line".into());
                }
            }),
            |t| t.trim().is_empty().then_some("The style line cannot be empty: every picture prompt starts with it."),
            window,
            cx,
        );
        body = body.child(section("Style line").child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Used word for word at the start of every picture prompt, so every sheet, plate and frame is drawn the same way.")).child(card().child(line)));
        let mut neg = div().flex().flex_wrap().gap(px(space::XS));
        for (k, w) in st.negative.iter().enumerate() {
            let w2 = w.clone();
            neg = neg.child(word_chip(w).when(!ro, |chip| {
                chip.child(icon_button(("bi-neg-x", k), ph::X, "Remove").small().on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let w = w2.clone();
                    this.change(
                        "It could not be removed",
                        op(move |b| {
                            b.style.negative.retain(|x| *x != w);
                            b.style.locked.insert("/negative".into());
                        }),
                        cx,
                    )
                })))
            }));
        }
        let add = (!ro).then(|| {
            self.text(
                "style-neg-new",
                "",
                "Add something no picture should have",
                false,
                false,
                top(|b, text| {
                    if !text.is_empty() && !b.style.negative.iter().any(|x| x.eq_ignore_ascii_case(text)) {
                        b.style.negative.push(text.to_string());
                        b.style.locked.insert("/negative".into());
                    }
                }),
                no_check,
                window,
                cx,
            )
        });
        body = body.child(section("Never in a picture").child(neg).children(add));
        body = body.child(self.palettes(d, window, cx));
        div().flex().flex_col().size_full().child(self.top_bar(d, false, cx)).child(body)
    }

    fn palettes(&self, d: &Data, window: &Window, cx: &mut Context<Self>) -> Div {
        let st = &d.bible.style;
        let ro = root_locked(&st.locked);
        let mut s = section("Colour keys").child(div().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Seven colours with roles per mood, used by the scenes' colour themes. Click a colour's code to change it."));
        if st.palettes.is_empty() {
            return s.child(div().text_size(px(size::SM)).text_color(Tokens::text_3()).child("No colour keys yet. Building the bible suggests them from the manga's colours."));
        }
        for (pi, pal) in st.palettes.iter().enumerate() {
            let mut sw = div().flex().flex_wrap().gap(px(space::MD));
            for (k, swatch) in pal.swatches.iter().enumerate() {
                let (pid, idx) = (pal.id.clone(), k);
                let hex = self.text(
                    &format!("pal-{}-{k}", pal.id),
                    &swatch.hex,
                    "#RRGGBB",
                    false,
                    ro,
                    top(move |b, text| {
                        if let (Some(p), Some(v)) = (b.style.palettes.iter_mut().find(|p| p.id == pid), parse_hex(text)) {
                            if let Some(s) = p.swatches.get_mut(idx) {
                                s.hex = hex_text(v);
                            }
                            b.style.locked.insert("/palettes".into());
                        }
                    }),
                    |t| parse_hex(t).is_none().then_some("Type a colour like #3A6FF2."),
                    window,
                    cx,
                );
                let fill = parse_hex(&swatch.hex).map(rgb).unwrap_or_else(Tokens::surface_3);
                sw = sw.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .w(px(CROP))
                        .child(div().size(px(CROP)).rounded(px(radius::MD)).bg(fill).border_1().border_color(Tokens::border_strong()))
                        .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_2()).child(swatch.role.clone()))
                        .child(div().text_size(px(size::XS)).child(hex)),
                );
            }
            let name = if pal.name.trim().is_empty() { format!("Colour key {}", pi + 1) } else { pal.name.trim().to_string() };
            s = s.child(
                card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::SM))
                            .child(icon_ph(ph::SWATCHES, Tokens::text_2(), icon_px::SM))
                            .child(div().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name))
                            .child(div().flex_1())
                            .when(pal.brightness > 0., |d| d.child(spec(format!("Brightness {:.0}%", pal.brightness * 100.))))
                            .when(pal.saturation > 0., |d| d.child(spec(format!("Saturation {:.0}%", pal.saturation * 100.)))),
                    )
                    .child(sw),
            );
        }
        s
    }

    // ------------------------------------------------------------ voices

    fn voices_view(&self, d: &Data, cx: &mut Context<Self>) -> Div {
        let b = &d.bible;
        // Rows: the narrator, every speaking character, and anyone else already cast.
        let mut rows: Vec<(String, String, Option<usize>, String)> = vec![(NARRATOR.to_string(), "Narrator".to_string(), None, "Tells the story between the lines".to_string())];
        for (i, c) in b.characters.iter().enumerate() {
            if c.status == Status::Rejected {
                continue;
            }
            if speaks(c) || b.cast.voices.iter().any(|v| v.character == c.id) {
                rows.push((c.id.clone(), name_or(&c.name, UNNAMED_CHARACTER).to_string(), Some(i), role_word(c.role).to_string()));
            }
        }
        let cast_of = |who: &str| -> CharVoice {
            if who == NARRATOR {
                b.cast.narrator.clone()
            } else {
                b.cast.voices.iter().find(|v| v.character == who).cloned().unwrap_or_default()
            }
        };
        let pairs: Vec<(String, String)> = rows.iter().map(|(id, name, ..)| (name.clone(), cast_of(id).voice_id)).collect();
        let shared = shared_voices(&pairs);
        // Only voices really on this PC, in the project's language, grouped by engine.
        let voices = d.installed.options(&d.series.settings.language);

        let mut list = div().flex().flex_col();
        for (k, (who, name, hue, sub)) in rows.iter().enumerate() {
            let v = cast_of(who);
            let busy = job_run(&self.jobs, &self.ctx.project, |j| matches!(j, JobKind::Audition { character, .. } if character == who));
            let in_shared = shared.iter().any(|(voice, _)| *voice == v.voice_id);
            list = list.child(self.voice_row(d, k, who, name, *hue, sub, &v, &voices, in_shared, &busy, cx));
        }
        let mut body = column_body("bi-voices").gap(px(space::LG)).track_scroll(&self.body_scroll);
        if let Some(n) = self.notices(d, cx) {
            body = body.child(n);
        }
        if voices.is_empty() {
            body = body.child(
                inline_alert(AlertKind::Info, "No voices installed", backends::INSTALL_VOICES_HINT)
                    .action("Open Settings › Audio", |_, w, cx| backends::open_audio_settings(w, cx)),
            );
        }
        for (voice, who) in &shared {
            let label = mv_studio::tts::find_voice(voice).map(|v| v.label.to_string()).unwrap_or_else(|| voice.clone());
            body = body.child(info_line(ph::WARNING, LineTone::Warning, format!("{} share one voice ({label}). Give each a voice of their own, so viewers can tell them apart.", join_names(who))));
        }
        let pick_all = self.cast_button(cx);
        body = body.child(
            section("One voice per speaking character")
                .child(div().flex().items_center().gap(px(space::SM)).child(div().flex_1().text_size(px(size::XS)).text_color(Tokens::text_3()).whitespace_normal().child("Audition a voice to hear it say the character's first line. Lock a voice to keep it when the cast is chosen again.")).children(pick_all))
                .child(card().p(px(space::XS)).gap(px(0.)).child(list)),
        );
        div().flex().flex_col().size_full().child(self.top_bar(d, true, cx)).child(body)
    }

    #[cfg(feature = "sound")]
    fn cast_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            btn("bi-cast-all", "Choose voices for me", ButtonKind::Secondary)
                .small()
                .icon(ph::SPARKLE)
                .tooltip("Gives every character without a locked voice a distinct installed voice that fits them")
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.cast_everyone(cx)))
                .into_any_element(),
        )
    }

    #[cfg(not(feature = "sound"))]
    fn cast_button(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn voice_row(&self, d: &Data, k: usize, who: &str, name: &str, hue: Option<usize>, sub: &str, v: &CharVoice, voices: &[(&'static str, Vec<&'static str>)], shared: bool, run: &Run, cx: &mut Context<Self>) -> Div {
        let current = voice_label(&v.voice_id, &d.installed);
        let open = self.voice_pick.as_deref() == Some(who);
        let who_t = who.to_string();
        let trigger = crate::widgets::field(eid("bi-voice", who), Some(ph::MICROPHONE), current, open).min_w(px(TILE_W)).tooltip(tip("Choose the voice")).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.voice_pick = if this.voice_pick.as_deref() == Some(who_t.as_str()) { None } else { Some(who_t.clone()) };
            if this.voice_pick.is_some() {
                window.focus(&this.voice_focus, cx);
            }
            cx.notify();
        }));
        let menu = div().relative().flex().w(px(TILE_W)).child(trigger).when(open, |m| m.child(self.voice_menu(who, &v.voice_id, voices, cx)));
        let mut speeds = div().flex().gap(px(space::XXS));
        let sp = if v.speed > 0. { v.speed } else { 1. };
        for (i, s) in SPEEDS.into_iter().enumerate() {
            let on = (sp - s).abs() < 0.01;
            let who2 = who.to_string();
            speeds = speeds.child(opt_chip(eid("bi-speed", &format!("{who}-{i}")), speed_label(s), on).when(!on, |chip| {
                chip.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cast_change(&who2, "The speed could not be changed", move |v| v.speed = s, cx)))
            }));
        }
        let take = v.auditions.iter().rev().find_map(|id| d.auditions.get(id).filter(|(voice, _)| *voice == v.voice_id).map(|(_, p)| p.clone()));
        let play = take.map(|p| {
            let on = self.playing.as_deref() == Some(p.as_path());
            icon_button(eid("bi-play", who), if on { ph::STOP } else { ph::PLAY }, if on { "Stop" } else { "Play the audition" }).small().selected(on).on_click(move |_, _, _| ui_studio::audio::toggle(&p))
        });
        let audition = btn(eid("bi-audition", who), "Audition", ButtonKind::Ghost).small().icon(ph::SPEAKER_HIGH).loading(run.active());
        let audition = if !cfg!(feature = "sound") {
            audition.disabled("Auditions come with the Sound step, which is not part of this build")
        } else if v.voice_id.trim().is_empty() {
            audition.disabled("Choose a voice first")
        } else if run.active() {
            audition
        } else {
            let (who2, name2, voice) = (who.to_string(), name.to_string(), v.voice_id.clone());
            audition.tooltip("Hear this voice say the character's first line").on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.audition(&who2, &name2, &voice, cx)))
        };
        let locked = v.locked;
        let who3 = who.to_string();
        let lock = icon_button(eid("bi-vlock", who), if locked { ph::LOCK_SIMPLE } else { ph::LOCK_SIMPLE_OPEN }, if locked { "Locked: kept when voices are chosen again. Press to unlock" } else { "Keep this voice when voices are chosen again" })
            .small()
            .selected(locked)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cast_change(&who3, "The voice could not be locked", move |v| v.locked = !locked, cx)));
        let mark = match hue {
            Some(i) => marker(i, name, 28.).into_any_element(),
            None => div().flex().items_center().justify_center().size(px(28.)).rounded_full().bg(Tokens::surface_3()).child(icon_ph(ph::BOOK_OPEN_TEXT, Tokens::text_2(), icon_px::SM)).into_any_element(),
        };
        let failed = match run {
            Run::Failed { error, .. } => Some(div().pl(px(space::XXXL)).child(info_line(ph::WARNING_CIRCLE, LineTone::Danger, format!("The audition could not be made: {error}")))),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .rounded(px(radius::MD))
            .when(k > 0, |d| d.border_t_1().border_color(Tokens::border()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .child(mark)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(TILE_W * 0.6))
                            .min_w_0()
                            .child(div().truncate().text_size(px(size::SM)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(name.to_string()))
                            .child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(sub.to_string())),
                    )
                    .child(menu)
                    // Chosen by the app (as Sound shows it), not by the user: not locked.
                    .when(!v.voice_id.trim().is_empty() && !v.locked, |d| d.child(div().id(eid("bi-vpicked", who)).tooltip(tip("Picked for you. Choose another, or lock it to keep it")).child(tag("Picked for you", Tone::Neutral))))
                    .when(shared, |d| d.child(tag("Shared", Tone::Warning)))
                    .child(speeds)
                    .child(div().flex_1())
                    .child(audition)
                    .children(play)
                    .child(lock),
            )
            .children(failed)
    }
}

impl BibleStep {
    /// The voice list under a cast row: only voices installed on this PC, grouped by engine, in
    /// a box that fits the window and scrolls; with none installed, where to get them.
    fn voice_menu(&self, who: &str, current: &str, voices: &[(&'static str, Vec<&'static str>)], cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("bi-voice-list").flex().flex_col().gap(px(2.)).max_h(px(VOICE_MENU_MAX_H)).overflow_y_scroll();
        if voices.is_empty() {
            list = list
                .child(div().p(px(space::SM)).text_size(px(size::SM)).text_color(Tokens::text_2()).whitespace_normal().child(backends::INSTALL_VOICES_HINT))
                .child(div().px(px(space::SM)).pb(px(space::SM)).child(btn("bi-voice-settings", "Open Settings › Audio", ButtonKind::Secondary).small().icon(ph::GEAR).on_click(|_, w, cx| backends::open_audio_settings(w, cx))));
        }
        for (g, (engine, ids)) in voices.iter().enumerate() {
            list = list.child(div().px(px(space::SM)).pt(px(if g == 0 { space::XS } else { space::SM })).pb(px(space::XXS)).child(label_caps(*engine)));
            for (k, id) in ids.iter().enumerate() {
                let on = *id == current;
                let (who2, pick) = (who.to_string(), id.to_string());
                let label = voice_hint(id);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("bi-voice-{g}-{k}")))
                        .flex()
                        .items_center()
                        .gap(px(space::SM))
                        .h(px(VOICE_ROW_H))
                        .px(px(space::SM))
                        .rounded(px(radius::MD))
                        .cursor_pointer()
                        .when(on, |d| d.bg(Tokens::active()))
                        .hover(|s| s.bg(Tokens::hover()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.voice_pick = None;
                            let id = pick.clone();
                            // As in Sound: a clone clip belongs to the voice it was made for, and a
                            // voice the user picked is kept by "Choose voices for me".
                            this.cast_change(
                                &who2,
                                "The voice could not be changed",
                                move |v| {
                                    if v.voice_id != id {
                                        v.reference = None;
                                    }
                                    v.voice_id = id.clone();
                                    v.locked = true;
                                },
                                cx,
                            );
                        }))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(size::SM)).text_color(Tokens::text_1()).child(label))
                        .when(on, |d| d.child(icon_ph(ph::CHECK, Tokens::accent(), icon_px::SM))),
                );
            }
        }
        // A click outside or Escape closes the list without picking anything.
        let pop = popup_surface()
            .w(px(VOICE_MENU_W))
            .p(px(space::XS))
            .track_focus(&self.voice_focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.voice_pick = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.voice_pick = None;
                cx.notify();
            }));
        div()
            .absolute()
            .top_full()
            .left_0()
            .child(deferred(anchored().anchor(Anchor::TopLeft).offset(point(px(0.), px(space::XS))).snap_to_window_with_margin(px(space::SM)).child(mv_ui::appear("bi-voice-pop", pop.child(list)))).with_priority(3))
    }
}

/// A voice in the menu as Sound's menu shows it: its name and what it sounds like ("Heart (US) ·
/// warm female").
fn voice_hint(id: &str) -> String {
    match mv_studio::tts::find_voice(id) {
        Some(v) => {
            let name = v.label.split_once(" — ").map(|(_, n)| n).unwrap_or(v.label).trim();
            let desc = v.desc.split_once(" · ").map(|(_, d)| d).unwrap_or(v.desc).trim();
            if desc.is_empty() {
                name.to_string()
            } else {
                format!("{name} · {desc}")
            }
        }
        None => "Voice not in this version".into(),
    }
}

/// What a cast row's voice field says: the voice, a note when it is not on this PC, or
/// "Choose a voice" (Sound says the same).
fn voice_label(id: &str, installed: &backends::InstalledVoices) -> String {
    if id.trim().is_empty() {
        return "Choose a voice".into();
    }
    let label = mv_studio::tts::find_voice(id).map(|i| i.label.to_string()).unwrap_or_else(|| id.to_string());
    if installed.has(id) {
        label
    } else {
        format!("{label} (not installed)")
    }
}

/// "Jinwoo and Song", "Jinwoo, Song and Ahn".
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{
        all_cards, card_entry, card_of, char_needs, character_status, engine_target_gone, home_of, join_names, latest_round, missing_lines, parse_height, parse_hex, place_needs,
        place_status, prop_status, shared_voices, speed_label, tkey, Open, Tab, MISSING_SHOWN,
    };
    use mv_anime::{BibleIndex, Card, Character, Place, PlaceState, Plate, Prop, Role, Sheet, SheetKind, Station, Status, Target};

    fn ok_card() -> Card {
        Card { status: Status::Approved, selected: Some("t".into()), takes: vec!["t".into()], ..Default::default() }
    }

    fn hero() -> Character {
        Character { id: "kai".into(), name: "Kai".into(), role: Role::Hero, ..Default::default() }
    }

    fn hall() -> Place {
        Place {
            id: "pl_hall".into(),
            name: "Hall".into(),
            stations: vec![Station { id: "st_1".into(), ..Default::default() }, Station { id: "st_overhead".into(), overhead: true, ..Default::default() }],
            states: vec![PlaceState { id: "day".into(), name: "day".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    fn bible() -> BibleIndex {
        BibleIndex { characters: vec![hero()], places: vec![hall()], props: vec![Prop { id: "pr_sword".into(), name: "Sword".into(), ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn cards_are_found_and_made_like_the_engine_does() {
        let mut b = bible();
        let face = Target::Sheet { owner: "kai".into(), sheet: "face".into() };
        assert!(card_of(&b, &face).is_none());
        if let Some(c) = card_entry(&mut b, &face) {
            c.status = Status::NeedsReview;
        }
        assert_eq!(b.characters[0].sheets.len(), 1);
        assert_eq!(card_of(&b, &face).map(|c| c.status), Some(Status::NeedsReview));
        // A prop's sheet, a plate (only of a station that exists), the style frame.
        assert!(card_entry(&mut b, &Target::Sheet { owner: "pr_sword".into(), sheet: "master".into() }).is_some());
        assert_eq!(b.props[0].sheets.len(), 1);
        assert!(card_entry(&mut b, &Target::Plate { place: "pl_hall".into(), station: "st_1".into(), state: "day".into() }).is_some());
        assert!(card_entry(&mut b, &Target::Plate { place: "pl_hall".into(), station: "gone".into(), state: "day".into() }).is_none());
        assert_eq!(b.places[0].plates.len(), 1);
        assert!(card_entry(&mut b, &Target::StyleFrame).is_some());
        assert!(card_entry(&mut b, &Target::Sheet { owner: "nobody".into(), sheet: "face".into() }).is_none());
        assert!(card_entry(&mut b, &Target::Sheet { owner: "kai".into(), sheet: "mouth".into() }).is_none(), "unknown kinds are left alone");
        // Every card, with its target.
        let keys: Vec<String> = all_cards(&b).iter().map(|(t, _)| tkey(t)).collect();
        assert_eq!(keys, ["sheet/kai/face", "place/pl_hall", "plate/pl_hall/st_1/day", "sheet/pr_sword/master", "style"]);
    }

    #[test]
    fn statuses_say_what_still_needs_the_user() {
        let mut c = hero();
        assert_eq!(char_needs(&c, true), ["main design", "face sheet", "turnaround"]);
        assert_eq!(character_status(&c, true), Status::Draft);
        c.sheets.push(Sheet { kind: SheetKind::Master, card: Card { status: Status::NeedsReview, selected: Some("t".into()), ..Default::default() } });
        assert_eq!(character_status(&c, true), Status::NeedsReview);
        c.sheets[0].card = ok_card();
        c.sheets.push(Sheet { kind: SheetKind::Face, card: ok_card() });
        c.sheets.push(Sheet { kind: SheetKind::Turnaround, card: ok_card() });
        assert!(char_needs(&c, true).is_empty());
        assert_eq!(character_status(&c, true), Status::Approved);
        // A supporting character needs nothing for the approval; their main design makes them ready.
        let mut side = Character { id: "s".into(), ..Default::default() };
        assert!(char_needs(&side, false).is_empty());
        assert_eq!(character_status(&side, false), Status::Draft);
        side.sheets.push(Sheet { kind: SheetKind::Master, card: ok_card() });
        assert_eq!(character_status(&side, false), Status::Approved);
        side.status = Status::Rejected;
        assert_eq!(character_status(&side, false), Status::Rejected);

        let mut p = hall();
        assert_eq!(place_needs(&p), ["environment master", "overhead plate"]);
        p.sheet = ok_card();
        p.plates.push(Plate { station: "st_1".into(), state: "day".into(), card: ok_card() });
        assert_eq!(place_needs(&p), ["overhead plate"], "a plate from another station does not count");
        p.plates.push(Plate { station: "st_overhead".into(), state: "day".into(), card: ok_card() });
        assert!(place_needs(&p).is_empty());
        assert_eq!(place_status(&p), Status::Approved);
        p.stations.retain(|s| !s.overhead);
        assert_eq!(place_needs(&p), ["overhead station"]);

        let mut prop = Prop::default();
        assert_eq!(prop_status(&prop), Status::Draft);
        prop.sheets.push(Sheet { kind: SheetKind::Master, card: ok_card() });
        assert_eq!(prop_status(&prop), Status::Approved);
    }

    #[test]
    fn pictures_know_their_tab() {
        let b = bible();
        assert_eq!(home_of(&b, &Target::Sheet { owner: "kai".into(), sheet: "face".into() }), Some((Tab::Cast, Some(Open::Character("kai".into())))));
        assert_eq!(home_of(&b, &Target::Sheet { owner: "pr_sword".into(), sheet: "master".into() }), Some((Tab::Props, Some(Open::Prop("pr_sword".into())))));
        assert_eq!(home_of(&b, &Target::Plate { place: "pl_hall".into(), station: "st_1".into(), state: "day".into() }), Some((Tab::Places, Some(Open::Place("pl_hall".into())))));
        assert_eq!(home_of(&b, &Target::StyleFrame), Some((Tab::Style, None)));
        assert_eq!(home_of(&b, &Target::PlaceSheet { place: "gone".into() }), None);
        assert!(engine_target_gone(&b, &Target::Plate { place: "pl_hall".into(), station: "st_9".into(), state: "day".into() }));
        assert!(engine_target_gone(&b, &Target::Plate { place: "pl_hall".into(), station: "st_1".into(), state: "night".into() }));
        assert!(!engine_target_gone(&b, &Target::Plate { place: "pl_hall".into(), station: "st_1".into(), state: "day".into() }));
    }

    #[test]
    fn typed_values_are_read_kindly() {
        assert_eq!(parse_hex("#3A6FF2"), Some(0x3A6FF2));
        assert_eq!(parse_hex(" 36f "), Some(0x3366FF));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("blue"), None);
        assert_eq!(parse_height("1.75"), Some(1.75));
        assert_eq!(parse_height("1,8 m"), Some(1.8));
        assert_eq!(parse_height("175 cm"), Some(1.75));
        assert_eq!(parse_height("180"), Some(1.8));
        assert_eq!(parse_height("tall"), None);
        assert_eq!(parse_height("12"), None);
        assert_eq!(speed_label(1.), "Normal");
        assert_eq!(speed_label(0.), "Normal");
        assert_eq!(speed_label(1.1), "1.1×");
    }

    #[test]
    fn one_ready_count_on_every_tab_and_left_out_items_do_not_count() {
        let mut b = bible();
        let (ready, total) = super::bible_readiness(&b, &std::collections::HashSet::new());
        assert_eq!(total, b.characters.len() + b.places.len() + b.props.len() + 1);
        assert!(ready <= total);
        if let Some(c) = b.characters.first_mut() {
            c.status = Status::Rejected;
            let t = Target::Sheet { owner: c.id.clone(), sheet: "master".into() };
            assert!(super::left_out(&b, &t), "a left-out character's sheets are never redrawn");
        }
        assert_eq!(super::bible_readiness(&b, &std::collections::HashSet::new()).1, total - usize::from(!b.characters.is_empty()));
    }

    #[test]
    fn notices_and_lists_read_plainly() {
        let missing: Vec<String> = (0..MISSING_SHOWN + 2).map(|i| format!("Item {i}: approve it.")).collect();
        let (shown, more) = missing_lines(&missing);
        assert_eq!(shown.len(), MISSING_SHOWN);
        assert_eq!(shown[0], "Item 0: approve it");
        assert_eq!(more, 2);
        assert_eq!(join_names(&["A".into(), "B".into(), "C".into()]), "A, B and C");
        let shared = shared_voices(&[("Kai".into(), "am_adam".into()), ("Mia".into(), "af_bella".into()), ("Ren".into(), "am_adam".into()), ("Nar".into(), "".into()), ("Q".into(), "".into())]);
        assert_eq!(shared, vec![("am_adam".to_string(), vec!["Kai".to_string(), "Ren".to_string()])]);
    }

    #[test]
    fn the_latest_round_is_the_newest_batch() {
        let takes = vec![("a".to_string(), Some("1".to_string())), ("b".to_string(), Some("2".to_string())), ("c".to_string(), Some("2".to_string()))];
        assert_eq!(latest_round(&takes), ["b", "c"]);
        let upload = vec![("a".to_string(), Some("1".to_string())), ("u".to_string(), None)];
        assert_eq!(latest_round(&upload), ["u"]);
        assert!(latest_round(&[]).is_empty());
    }
}

#[cfg(test)]
mod cover_tests {
    // Explicit imports: with test-support, `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    // Which crop may be a cover is the engine's rule (`mv_anime::crops::character_cover`), tested
    // there.
    use super::voice_label;

    #[test]
    fn a_voice_not_on_this_pc_says_so() {
        let none = crate::backends::InstalledVoices::default();
        assert_eq!(voice_label("", &none), "Choose a voice");
        assert!(voice_label("af_heart", &none).ends_with("(not installed)"));
    }
}
