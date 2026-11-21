//! The reference-slot algorithm (design 6.6, plan 10): which pictures go into the picture
//! model's 14 reference slots for one shot, in upload order ("Image N" in the prompt).
//!
//! Nano Banana 2.1 keeps likeness for at most 4 characters and fidelity for at most 10 other
//! pictures, and extra pictures get averaged together, so the order is a priority list that
//! stops at a soft cap of 8 ("fewer, cleaner"):
//!
//! 1. the background: a matched-room shot gets its station's approved plate (else the place's
//!    environment master); a simple-backdrop shot gets the place master for colours only; a
//!    no-background shot gets nothing. Pinned user references come right after it;
//! 2. one identity picture per visible character, left to right on screen, at most 4: the face
//!    sheet for close shots, the back view when seen from behind, else the outfit sheet or the
//!    turnaround; an approved manga crop when no approved sheet exists (character and prop
//!    sheets are used only once approved, see [`left_out`]). A crowded shot (5+ visible) gives
//!    identity pictures to the 2 characters nearest the centre only (a second pass adds the rest);
//! 3. the style frame; 4. expression sheets for strong emotions; 5. grip or prop sheets for
//!    everything held; 6. colour models at night, dusk or in a dark key; 7. a second view of the
//!    place for wide shots; 8. the source manga panel as a layout guide.
//!
//! Every reference states one job (the tile caption and the upload label). [`swap`] applies the
//! retry rules after a failed check. Hashes are content hashes: a take file (`takes/aa/<sha256>.ext`)
//! is named by its content hash; any other file is hashed (cached by size and time).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::compile::{emotion_adjective, fill};
use crate::geometry;
use crate::model::{
    BackgroundKind, BibleIndex, Card, Character, CropFraming, Derived, Issue, Key, Mark, Place, PlaceState, RefCrop, RefRole, RefSlot, Scene, Severity, SheetKind, Shot,
    ShotSize, SiteSlot, Station, Status, TakeKind, TakeMeta, TakeSource, Target, TimeOfDay, ViewAngle, Visibility,
};
use crate::store::Store;

/// Most references the site accepts.
pub const MAX_REFS: usize = 14;
/// Most identity images in one request.
pub const MAX_IDENTITY: usize = 4;
/// Default cap ("fewer, cleaner").
pub const SOFT_CAP: usize = 8;
/// Visible characters from which a shot is crowded.
pub const CROWDED_FROM: usize = 5;
/// Characters that keep an identity picture in a crowded shot.
pub const CROWDED_KEEP: usize = 2;
/// The issue code of a crowded shot.
pub const CROWDED_CODE: &str = "crowded";

/// The single job stated for each kind of reference: the tile caption in the Generation panel
/// and the label sent with the upload. Plain data, easy to tune. `{name}`, `{prop}` and
/// `{emotion}` are filled in. (The driver sends a reference whose label says "plate" larger.)
pub mod jobs {
    /// The plate of a matched background.
    pub const PLATE: &str = "background plate: owns the room, its layout and the camera";
    /// The place master standing in for a plate.
    pub const PLACE_MASTER: &str = "place sheet: owns the room, its layout and its materials";
    /// The place master behind a simple backdrop.
    pub const BACKDROP: &str = "place sheet: colours and materials only";
    /// A second view of the place.
    pub const PLACE_VIEW: &str = "another view of the place: keeps the room the same, not the camera";
    /// The style frame.
    pub const STYLE: &str = "line, shading and colour style only";
    /// The source manga panel.
    pub const PANEL: &str = "manga panel: pose and composition only";
    /// A face sheet or close crop.
    pub const FACE: &str = "{name}'s face and hair only";
    /// A turnaround, outfit sheet or full-body crop.
    pub const BODY: &str = "{name}'s body and outfit";
    /// A back view.
    pub const BACK: &str = "{name} seen from behind";
    /// An expression sheet.
    pub const EXPRESSION: &str = "{name}'s {emotion} expression only";
    /// A grip sheet.
    pub const GRIP: &str = "how {name} holds the {prop}";
    /// A prop sheet or crop.
    pub const PROP: &str = "the {prop}: shape, colours and size";
    /// A colour model sheet.
    pub const COLOUR_MODEL: &str = "{name}'s colours in this light";
}

/// Which view of a character a shot needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// MCU, CU, ECU: the face.
    Close,
    /// Seen from behind (back or over-the-shoulder visibility).
    Back,
    /// Everything else: the body and the outfit.
    Wide,
}

/// What failed a check on a picture; [`swap`] answers with a changed reference list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The character (id) does not look like themself.
    Identity(String),
    /// The prop (id) is wrong or missing.
    Prop(String),
    /// The place does not match.
    Place,
    /// The style drifted.
    Style,
}

/// The view a mark needs in this shot.
pub fn view_of(shot: &Shot, mark: &Mark) -> View {
    if matches!(mark.visible, Visibility::Back | Visibility::Ots) {
        View::Back
    } else if matches!(shot.camera.size, ShotSize::Mcu | ShotSize::Cu | ShotSize::Ecu) {
        View::Close
    } else {
        View::Wide
    }
}

/// Visible characters (one mark each, in mark order).
fn visible(shot: &Shot) -> Vec<&Mark> {
    let mut seen = BTreeSet::new();
    shot.marks.iter().filter(|m| m.visible != Visibility::Offscreen && !m.character.trim().is_empty() && seen.insert(m.character.as_str())).collect()
}

/// Screen x of a character, if it is in the frame.
fn sx(derived: &Derived, id: &str) -> Option<f32> {
    derived.screen_x.get(id).copied().filter(|x| x.is_finite())
}

/// Sorts marks left to right on screen; marks outside the frame keep their order at the end.
pub(crate) fn screen_order<'a>(mut marks: Vec<&'a Mark>, derived: &Derived) -> Vec<&'a Mark> {
    marks.sort_by(|a, b| {
        let (xa, xb) = (sx(derived, &a.character).unwrap_or(f32::MAX), sx(derived, &b.character).unwrap_or(f32::MAX));
        xa.total_cmp(&xb)
    });
    marks
}

/// Visible characters in screen order, left to right.
pub fn on_screen<'a>(shot: &'a Shot, derived: &Derived) -> Vec<&'a Mark> {
    screen_order(visible(shot), derived)
}

/// 5 or more characters are visible: one picture cannot keep all their faces.
pub fn is_crowded(shot: &Shot) -> bool {
    visible(shot).len() >= CROWDED_FROM
}

/// The characters that get an identity picture, in screen order: everyone visible (at most 4),
/// or in a crowded shot the [`CROWDED_KEEP`] nearest the centre of the frame.
pub fn identity_cast<'a>(shot: &'a Shot, derived: &Derived) -> Vec<&'a Mark> {
    let mut cast = visible(shot);
    if cast.len() >= CROWDED_FROM {
        cast.sort_by(|a, b| {
            let da = sx(derived, &a.character).map(|x| (x - 0.5).abs()).unwrap_or(f32::MAX);
            let db = sx(derived, &b.character).map(|x| (x - 0.5).abs()).unwrap_or(f32::MAX);
            da.total_cmp(&db)
        });
        cast.truncate(CROWDED_KEEP);
    }
    let mut cast = screen_order(cast, derived);
    cast.truncate(MAX_IDENTITY);
    cast
}

/// The Info issue of a crowded shot ("crowded: second pass"), or `None`. Recorded with the
/// episode's checks by `checks::check_episode`.
pub fn crowded_issue(shot: &Shot, scene: &Scene, bible: &BibleIndex) -> Option<Issue> {
    let n = visible(shot).len();
    if n < CROWDED_FROM {
        return None;
    }
    let place = scene.place.as_deref().and_then(|id| bible.place(id));
    let derived = geometry::derive(shot, place);
    let kept: Vec<String> = identity_cast(shot, &derived).iter().map(|m| bible.character(&m.character).map(|c| display(c)).unwrap_or_else(|| m.character.clone())).collect();
    Some(Issue {
        code: CROWDED_CODE.into(),
        severity: Severity::Info,
        scene: scene.id.clone(),
        shot: Some(shot.id.clone()),
        message: format!(
            "Crowded: second pass. {n} characters are visible, and one picture keeps faces for at most {MAX_IDENTITY}, so only {} get reference pictures in this pass.",
            join_and(&kept)
        ),
        fix: Some("Add the other characters in a second pass, or split the shot.".into()),
    })
}

fn display(c: &Character) -> String {
    if c.name.trim().is_empty() {
        c.id.clone()
    } else {
        c.name.trim().to_string()
    }
}

pub(crate) fn join_and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

// ---------------------------------------------------------------- hashes and files

type HashCache = HashMap<PathBuf, (u64, SystemTime, String)>;

fn hash_cache() -> &'static Mutex<HashCache> {
    static CACHE: OnceLock<Mutex<HashCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// sha256 of a file's content, cached by (size, modified time); `None` when it cannot be read.
pub fn content_hash(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let stamp = meta.modified().ok();
    if let Some(t) = stamp {
        let cache = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((len, at, h)) = cache.get(path) {
            if *len == meta.len() && *at == t {
                return Some(h.clone());
            }
        }
    }
    let h = crate::hash::file_hash(path).ok()?;
    if let Some(t) = stamp {
        let mut cache = hash_cache().lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(path.to_path_buf(), (meta.len(), t, h.clone()));
    }
    Some(h)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A project-relative file that stays inside the project.
fn safe_rel(rel: &str) -> bool {
    let rel = rel.replace('\\', "/");
    !rel.trim().is_empty() && !rel.starts_with('/') && !rel.split('/').any(|p| p == ".." || p.contains(':'))
}

/// The content hash of a project-relative reference file; `None` when it is missing. A take
/// file is named by its content hash, so it is not read again.
pub fn ref_hash(store: &Store, rel: &str) -> Option<String> {
    if !safe_rel(rel) {
        return None;
    }
    let path = store.path(rel);
    if !path.is_file() {
        return None;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let in_takes = rel.replace('\\', "/").starts_with("takes/");
    if in_takes && is_hex64(&stem) {
        return Some(stem);
    }
    content_hash(&path)
}

/// The project-relative file a card shows now (its selected take), unless it was rejected or
/// the file is gone. Character and prop sheets are references only when approved
/// ([`sheet_file`]).
pub fn card_file(store: &Store, card: &Card) -> Option<String> {
    if card.status == Status::Rejected {
        return None;
    }
    let id = card.selected.as_deref()?;
    let abs = store.take_file(id)?;
    store.rel(&abs)
}

/// The take id a project-relative file is named after (`takes/aa/<id>.png` -> `<id>`).
pub(crate) fn file_stem(rel: &str) -> &str {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.split('.').next().unwrap_or(name)
}

/// User references with their hashes read again (a changed file changes its hash), capped at
/// [`MAX_REFS`] and [`MAX_IDENTITY`] identity pictures. A missing file keeps its old hash (the
/// job then says it is missing).
pub fn refresh(store: &Store, refs: &[RefSlot]) -> Vec<RefSlot> {
    let mut out: Vec<RefSlot> = Vec::new();
    let mut identity = 0;
    for r in refs {
        if out.len() >= MAX_REFS || out.iter().any(|o| o.file == r.file) {
            continue;
        }
        if matches!(r.role, RefRole::Identity(_)) {
            if identity >= MAX_IDENTITY {
                continue;
            }
            identity += 1;
        }
        let mut r = r.clone();
        if let Some(h) = ref_hash(store, &r.file) {
            r.hash = h;
        }
        out.push(r);
    }
    out
}

// ---------------------------------------------------------------- the picker

/// The reference list being built, with its caps.
struct Picker<'a> {
    store: &'a Store,
    out: Vec<RefSlot>,
    cap: usize,
}

impl Picker<'_> {
    fn identities(&self) -> usize {
        self.out.iter().filter(|r| matches!(r.role, RefRole::Identity(_))).count()
    }

    fn has(&self, file: &str) -> bool {
        self.out.iter().any(|r| r.file == file)
    }

    fn room_for(&self, role: &RefRole) -> bool {
        self.out.len() < self.cap && (!matches!(role, RefRole::Identity(_)) || self.identities() < MAX_IDENTITY)
    }

    /// Adds a reference unless it is already there, a cap is reached or the file is missing.
    fn add(&mut self, file: String, role: RefRole, job: String, slot: SiteSlot) -> bool {
        if self.has(&file) || !self.room_for(&role) {
            return false;
        }
        let Some(hash) = ref_hash(self.store, &file) else { return false };
        self.out.push(RefSlot { file, hash, role, job, slot, pinned: false });
        true
    }

    /// Keeps a pinned user reference (its hash read again).
    fn keep(&mut self, r: &RefSlot) {
        if self.has(&r.file) || !self.room_for(&r.role) {
            return;
        }
        let Some(hash) = ref_hash(self.store, &r.file) else { return };
        self.out.push(RefSlot { hash, pinned: true, ..r.clone() });
    }
}

/// The references of a shot's frame, in upload order.
pub fn pick(shot: &Shot, scene: &Scene, bible: &BibleIndex, store: &Store) -> Vec<RefSlot> {
    let place = scene.place.as_deref().and_then(|id| bible.place(id));
    let derived = geometry::derive(shot, place);
    pick_with(shot, scene, bible, store, &derived)
}

/// [`pick`] with the shot's derived geometry already computed.
pub fn pick_with(shot: &Shot, scene: &Scene, bible: &BibleIndex, store: &Store, derived: &Derived) -> Vec<RefSlot> {
    let place = scene.place.as_deref().and_then(|id| bible.place(id));
    let pinned: Vec<&RefSlot> = shot.frame.gen.refs.iter().filter(|r| r.pinned && !r.file.trim().is_empty()).collect();
    // Pinned references never push the automatic ones out: they come on top of the soft cap.
    let mut p = Picker { store, out: Vec::new(), cap: MAX_REFS };

    // 1. The background (a pinned plate or layout replaces the automatic one), then the pins.
    let (pinned_bg, pinned_rest): (Vec<&RefSlot>, Vec<&RefSlot>) = pinned.iter().copied().partition(|r| matches!(r.role, RefRole::Plate | RefRole::Layout));
    for r in &pinned_bg {
        p.keep(r);
    }
    if pinned_bg.is_empty() {
        if let Some((file, role, job)) = place.and_then(|pl| background(shot, scene, pl, derived, store)) {
            p.add(file, role, job.to_string(), SiteSlot::Add);
        }
    }
    for r in &pinned_rest {
        p.keep(r);
    }
    p.cap = (SOFT_CAP + pinned.len()).min(MAX_REFS);

    // 2. One identity picture per character on screen.
    let cast = identity_cast(shot, derived);
    for m in &cast {
        if let Some((file, job)) = bible.character(&m.character).and_then(|c| identity_file(c, shot, m, store, None)) {
            p.add(file, RefRole::Identity(m.character.clone()), job, SiteSlot::Character);
        }
    }

    // 3. The style frame.
    if let Some(file) = card_file(store, &bible.style.frame) {
        p.add(file, RefRole::Style, jobs::STYLE.into(), SiteSlot::Style);
    }

    // 4. Expression sheets for strong emotions.
    for m in &cast {
        if m.emotion.strength >= 3 && !matches!(view_of(shot, m), View::Back) {
            if let Some(c) = bible.character(&m.character) {
                if let Some(file) = sheet_file(c, &SheetKind::Expressions, store) {
                    let job = fill(jobs::EXPRESSION, &[("name", &display(c)), ("emotion", emotion_adjective(m.emotion.kind))]);
                    p.add(file, RefRole::Expression(c.id.clone()), job, SiteSlot::Add);
                }
            }
        }
    }

    // 5. Grip or prop sheets for everything held.
    for m in on_screen(shot, derived) {
        for h in &m.holding {
            if let Some((file, role, job)) = held(bible, m, &h.prop, store) {
                p.add(file, role, job, SiteSlot::Add);
            }
        }
    }

    // 6. Colour models when the light is not daylight.
    if matches!(scene.time_of_day, TimeOfDay::Night | TimeOfDay::Dusk) || shot.camera.key == Key::Dark {
        for m in &cast {
            if let Some(c) = bible.character(&m.character) {
                if let Some(file) = sheet_file(c, &SheetKind::ColourModel, store) {
                    p.add(file, RefRole::ColourModel(c.id.clone()), fill(jobs::COLOUR_MODEL, &[("name", &display(c))]), SiteSlot::Add);
                }
            }
        }
    }

    // 7. A second view of the place when the shot sees most of it.
    if matches!(shot.camera.size, ShotSize::Els | ShotSize::Ls | ShotSize::Overhead) {
        if let Some(place) = place {
            add_place_view(&mut p, shot, scene, place, derived);
        }
    }

    // 8. The source manga panel as the layout guide.
    if let Some(file) = panel_take(shot, store) {
        p.add(file, RefRole::Panel, jobs::PANEL.into(), SiteSlot::Add);
    }

    p.out
}

/// The background reference of a shot: (file, role, job).
fn background(shot: &Shot, scene: &Scene, place: &Place, derived: &Derived, store: &Store) -> Option<(String, RefRole, &'static str)> {
    match shot.background {
        BackgroundKind::Matched => {
            let station = station_for(place, shot, derived);
            let state = state_for(place, scene.time_of_day);
            if let Some(f) = plate_file(place, station, state, store, true) {
                return Some((f, RefRole::Plate, jobs::PLATE));
            }
            if let Some(f) = card_file(store, &place.sheet) {
                return Some((f, RefRole::Plate, jobs::PLACE_MASTER));
            }
            plate_file(place, station, state, store, false).map(|f| (f, RefRole::Plate, jobs::PLATE))
        }
        BackgroundKind::Backdrop => card_file(store, &place.sheet).map(|f| (f, RefRole::PlaceView, jobs::BACKDROP)),
        _ => None,
    }
}

/// Smallest difference of two angles in degrees (0..180).
pub(crate) fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.);
    let d = if d.is_finite() { d } else { 0. };
    if d > 180. {
        360. - d
    } else {
        d
    }
}

/// The station a shot is filmed from: the derived one, else the nearest by position and yaw.
pub fn station_for<'a>(place: &'a Place, shot: &Shot, derived: &Derived) -> Option<&'a Station> {
    if let Some(s) = derived.station.as_deref().and_then(|id| place.stations.iter().find(|s| s.id == id)) {
        return Some(s);
    }
    let cam = geometry::cam_pos(&shot.camera, Some(place));
    let (yaw, _) = geometry::look_dir(&shot.camera, &shot.marks, Some(place));
    place
        .stations
        .iter()
        .map(|s| {
            let d = ((s.pos[0] - cam[0]).powi(2) + (s.pos[1] - cam[1]).powi(2) + (s.pos[2] - cam[2]).powi(2)).sqrt();
            let score = d + angle_diff(s.yaw_deg, yaw) / 30.;
            (if score.is_finite() { score } else { f32::MAX }, s)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Words a place state id or name may hold for each time of day, best first.
const STATE_WORDS: &[(TimeOfDay, &[&str])] = &[
    (TimeOfDay::Day, &["day", "noon", "morning", "afternoon"]),
    (TimeOfDay::Dusk, &["dusk", "sunset", "evening", "golden"]),
    (TimeOfDay::Night, &["night", "midnight", "moon"]),
    (TimeOfDay::Dawn, &["dawn", "sunrise", "morning"]),
    (TimeOfDay::Indoor, &["indoor", "interior", "day"]),
];

/// The lighting state of a place for a time of day: by id or name, else the first state.
pub fn state_for(place: &Place, tod: TimeOfDay) -> Option<&PlaceState> {
    let words = STATE_WORDS.iter().find(|(t, _)| *t == tod).map(|(_, w)| *w).unwrap_or(&[]);
    for w in words {
        if let Some(s) = place.states.iter().find(|s| s.id.to_lowercase().contains(w) || s.name.to_lowercase().contains(w)) {
            return Some(s);
        }
    }
    place.states.first()
}

/// The plate of a station in a state (approved only when `approved`).
fn plate_file(place: &Place, station: Option<&Station>, state: Option<&PlaceState>, store: &Store, approved: bool) -> Option<String> {
    let station = station?;
    place
        .plates
        .iter()
        .filter(|p| p.station == station.id && state.is_none_or(|s| p.state == s.id))
        .filter(|p| !approved || p.card.status == Status::Approved)
        .find_map(|p| card_file(store, &p.card))
}

/// Adds another view of the place: the master, else the plate seen most differently.
fn add_place_view(p: &mut Picker<'_>, shot: &Shot, scene: &Scene, place: &Place, derived: &Derived) -> bool {
    if shot.background != BackgroundKind::Matched {
        return false;
    }
    if let Some(f) = card_file(p.store, &place.sheet) {
        if !p.has(&f) {
            return p.add(f, RefRole::PlaceView, jobs::PLACE_VIEW.into(), SiteSlot::Add);
        }
    }
    let here = station_for(place, shot, derived);
    let state = state_for(place, scene.time_of_day);
    let mut others: Vec<(f32, &crate::model::Plate)> = place
        .plates
        .iter()
        .filter(|pl| here.is_none_or(|h| pl.station != h.id) && state.is_none_or(|s| pl.state == s.id) && pl.card.status == Status::Approved)
        .map(|pl| {
            let yaw = place.stations.iter().find(|s| s.id == pl.station).map(|s| s.yaw_deg).unwrap_or(0.);
            (here.map(|h| angle_diff(h.yaw_deg, yaw)).unwrap_or(0.), pl)
        })
        .collect();
    others.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, pl) in others {
        if let Some(f) = card_file(p.store, &pl.card) {
            if !p.has(&f) {
                return p.add(f, RefRole::PlaceView, jobs::PLACE_VIEW.into(), SiteSlot::Add);
            }
        }
    }
    false
}

/// The file of a sheet the user approved (an unreviewed sheet is never a reference: a wrong
/// first draw would carry into every frame).
fn approved_file(store: &Store, card: &Card) -> Option<String> {
    (card.status == Status::Approved).then(|| card_file(store, card)).flatten()
}

/// A character's approved sheet file of a kind.
pub fn sheet_file(c: &Character, kind: &SheetKind, store: &Store) -> Option<String> {
    c.sheets.iter().filter(|s| &s.kind == kind).find_map(|s| approved_file(store, &s.card))
}

/// A character's sheet of a kind that has a picture but is not approved yet.
fn unapproved_sheet(c: &Character, kind: &SheetKind, store: &Store) -> bool {
    c.sheets.iter().any(|s| &s.kind == kind && s.card.status != Status::Approved && card_file(store, &s.card).is_some())
}

fn sheet_words(kind: &SheetKind) -> &'static str {
    match kind {
        SheetKind::Master => "character design",
        SheetKind::Face => "face sheet",
        SheetKind::Turnaround => "turnaround",
        SheetKind::Expressions => "expression sheet",
        SheetKind::Outfit(_) => "outfit sheet",
        SheetKind::Grip(_) => "grip sheet",
        SheetKind::ColourModel => "colour model",
        SheetKind::Back => "back view",
        SheetKind::Unknown => "sheet",
    }
}

/// What the picker left out because it is not approved yet, in plain words, one line each:
/// "Kai's face sheet is not approved yet, so a manga picture of Kai stands in for it." The
/// Frames screen shows these next to the references.
pub fn left_out(shot: &Shot, scene: &Scene, bible: &BibleIndex, store: &Store, derived: &Derived) -> Vec<String> {
    let mut out = Vec::new();
    let cast = identity_cast(shot, derived);
    for m in &cast {
        let Some(c) = bible.character(&m.character) else { continue };
        let name = display(c);
        let view = view_of(shot, m);
        // The best sheet with a picture: approved, it is used; else it is left out.
        let chain = sheet_chain(view, m.outfit.as_deref());
        let first = chain.iter().find(|k| sheet_file(c, k, store).is_some() || unapproved_sheet(c, k, store));
        if let Some(kind) = first.filter(|k| sheet_file(c, k, store).is_none()) {
            let instead = match identity_file(c, shot, m, store, None) {
                Some((f, _)) if crops_for(&c.crops, view, store).iter().any(|cr| cr.file == f) => format!("a manga picture of {name} stands in for it"),
                Some(_) => "another approved sheet stands in for it".to_string(),
                None => format!("no picture of {name} is sent"),
            };
            out.push(format!("{name}'s {} is not approved yet, so {instead}.", sheet_words(kind)));
        }
        if m.emotion.strength >= 3 && view != View::Back && sheet_file(c, &SheetKind::Expressions, store).is_none() && unapproved_sheet(c, &SheetKind::Expressions, store) {
            out.push(format!("{name}'s expression sheet is not approved yet, so it is left out."));
        }
        let dark = matches!(scene.time_of_day, TimeOfDay::Night | TimeOfDay::Dusk) || shot.camera.key == Key::Dark;
        if dark && sheet_file(c, &SheetKind::ColourModel, store).is_none() && unapproved_sheet(c, &SheetKind::ColourModel, store) {
            out.push(format!("{name}'s colour model is not approved yet, so it is left out."));
        }
    }
    for m in on_screen(shot, derived) {
        let Some(c) = bible.character(&m.character) else { continue };
        for h in &m.holding {
            let grip = SheetKind::Grip(h.prop.clone());
            if sheet_file(c, &grip, store).is_none() && unapproved_sheet(c, &grip, store) {
                out.push(format!("{}'s grip sheet is not approved yet, so it is left out.", display(c)));
            }
            if let Some(p) = bible.prop(&h.prop) {
                let has = p.sheets.iter().any(|s| s.card.status == Status::Approved && card_file(store, &s.card).is_some());
                if !has && p.sheets.iter().any(|s| s.card.status != Status::Approved && card_file(store, &s.card).is_some()) {
                    let pname = if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() };
                    out.push(format!("The {pname}'s design sheet is not approved yet, so it is left out."));
                }
            }
        }
    }
    out.dedup();
    out
}

/// Sheets to try for a view, best first.
fn sheet_chain(view: View, outfit: Option<&str>) -> Vec<SheetKind> {
    match view {
        View::Close => vec![SheetKind::Face, SheetKind::Master, SheetKind::Turnaround],
        View::Back => vec![SheetKind::Back, SheetKind::Turnaround, SheetKind::Master],
        View::Wide => {
            let mut v = Vec::new();
            if let Some(o) = outfit.filter(|o| !o.trim().is_empty()) {
                v.push(SheetKind::Outfit(o.to_string()));
            }
            v.extend([SheetKind::Turnaround, SheetKind::Master]);
            v
        }
    }
}

/// How well a crop fits a view (lower is better).
fn crop_rank(c: &RefCrop, view: View) -> u8 {
    let framing = |order: [CropFraming; 3]| order.iter().position(|f| *f == c.framing).unwrap_or(3) as u8;
    let back = c.angle == ViewAngle::Back;
    match view {
        View::Close => framing([CropFraming::Face, CropFraming::Bust, CropFraming::Full]) + if back { 10 } else { 0 },
        View::Back => framing([CropFraming::Full, CropFraming::Bust, CropFraming::Face]) + if back { 0 } else { 10 },
        View::Wide => framing([CropFraming::Full, CropFraming::Bust, CropFraming::Face]) + if back { 10 } else { 0 },
    }
}

/// Approved crops that exist, best first for the view (then by score).
fn crops_for<'a>(crops: &'a [RefCrop], view: View, store: &Store) -> Vec<&'a RefCrop> {
    let mut v: Vec<&RefCrop> = crops.iter().filter(|c| c.approved && safe_rel(&c.file) && store.path(&c.file).is_file()).collect();
    v.sort_by(|a, b| crop_rank(a, view).cmp(&crop_rank(b, view)).then(b.score.total_cmp(&a.score)));
    v
}

/// The job of an identity picture for a view.
fn identity_job(c: &Character, view: View) -> String {
    let t = match view {
        View::Close => jobs::FACE,
        View::Back => jobs::BACK,
        View::Wide => jobs::BODY,
    };
    fill(t, &[("name", &display(c))])
}

/// The identity picture of a mark: (file, job). `avoid` skips one file (a swap after a failure).
fn identity_file(c: &Character, shot: &Shot, m: &Mark, store: &Store, avoid: Option<&str>) -> Option<(String, String)> {
    let view = view_of(shot, m);
    let job = identity_job(c, view);
    let sheets = sheet_chain(view, m.outfit.as_deref()).into_iter().filter_map(|k| sheet_file(c, &k, store));
    let crops = crops_for(&c.crops, view, store).into_iter().map(|cr| cr.file.clone());
    sheets.chain(crops).find(|f| avoid != Some(f.as_str())).map(|f| (f, job))
}

/// The grip sheet of the holder for a prop, else the prop's own sheet, else an approved crop.
fn held(bible: &BibleIndex, holder: &Mark, prop: &str, store: &Store) -> Option<(String, RefRole, String)> {
    let prop_name = bible.prop(prop).map(|p| if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() }).unwrap_or_else(|| prop.replace('_', " "));
    if let Some(c) = bible.character(&holder.character) {
        if let Some(f) = sheet_file(c, &SheetKind::Grip(prop.to_string()), store) {
            return Some((f, RefRole::Grip(prop.to_string()), fill(jobs::GRIP, &[("name", &display(c)), ("prop", &prop_name)])));
        }
    }
    prop_sheet(bible, prop, store).map(|f| (f, RefRole::Prop(prop.to_string()), fill(jobs::PROP, &[("prop", &prop_name)])))
}

/// A prop's own sheet (master first) or its best approved crop.
fn prop_sheet(bible: &BibleIndex, prop: &str, store: &Store) -> Option<String> {
    let p = bible.prop(prop)?;
    let master = p.sheets.iter().filter(|s| s.kind == SheetKind::Master).find_map(|s| approved_file(store, &s.card));
    master.or_else(|| p.sheets.iter().find_map(|s| approved_file(store, &s.card))).or_else(|| crops_for(&p.crops, View::Wide, store).first().map(|c| c.file.clone()))
}

/// The shot's source manga panel, imported into the project as a take (project-relative file).
/// The panel lives outside the project (Studio's crops); it is copied once, content-addressed.
pub fn panel_take(shot: &Shot, store: &Store) -> Option<String> {
    let src = shot.source.iter().find(|s| s.panel.is_some())?;
    let panel = src.panel?;
    let series = store.series().ok()?;
    let file = series.episodes.iter().find_map(|e| {
        let ep = store.episode(&e.id).ok()?;
        let pick = ep.chapters.iter().find(|c| c.chapter_id == src.chapter_id)?;
        let f = crate::source::panel_file(pick, &series.manga_title, src.page, panel);
        f.is_file().then_some(f)
    })?;
    let id = content_hash(&file)?;
    if let Some(t) = store.take(&id) {
        if store.path(&t.file).is_file() {
            return Some(t.file);
        }
    }
    let bytes = std::fs::read(&file).ok()?;
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
    let (w, h) = image::image_dimensions(&file).unwrap_or((0, 0));
    let meta = TakeMeta {
        kind: TakeKind::Image,
        target: Target::Crop { owner: shot.id.clone() },
        source: TakeSource::MangaCrop(src.clone()),
        w,
        h,
        note: "manga panel used as a layout guide".into(),
        ..Default::default()
    };
    match store.add_take(&bytes, &ext, meta) {
        Ok(t) => Some(t.file),
        Err(e) => {
            mv_core::logging::global().warn("anime", &format!("panel not kept as a layout guide: {e:#}"));
            None
        }
    }
}

// ---------------------------------------------------------------- swaps after a failure

/// Lowest priority first: what goes when a swap needs room.
fn drop_order(role: &RefRole) -> u8 {
    match role {
        RefRole::Panel => 0,
        RefRole::PlaceView => 1,
        RefRole::ColourModel(_) => 2,
        RefRole::Expression(_) => 3,
        RefRole::Prop(_) | RefRole::Grip(_) => 4,
        _ => 9,
    }
}

/// Makes room for one more reference by dropping the lowest-priority unpinned one.
fn make_room(v: &mut Vec<RefSlot>) -> bool {
    if v.len() < MAX_REFS {
        return true;
    }
    let victim = v.iter().enumerate().filter(|(_, r)| !r.pinned && drop_order(&r.role) < 9).min_by_key(|(i, r)| (drop_order(&r.role), usize::MAX - i)).map(|(i, _)| i);
    match victim {
        Some(i) => {
            v.remove(i);
            true
        }
        None => false,
    }
}

fn new_slot(store: &Store, file: String, role: RefRole, job: String, slot: SiteSlot) -> Option<RefSlot> {
    let hash = ref_hash(store, &file)?;
    Some(RefSlot { file, hash, role, job, slot, pinned: false })
}

/// The retry rules after a failed check (research 08 §6.5): an identity failure puts a bigger
/// picture of the failing view in that character's slot; a prop failure adds the prop sheet next
/// to the grip; a place failure adds a second view of the place; a style failure moves the style
/// frame to Image 1 (the compiler then repeats the style at the end). Caps still hold.
pub fn swap(slots: &[RefSlot], failure: &Failure, shot: &Shot, scene: &Scene, bible: &BibleIndex, store: &Store) -> Vec<RefSlot> {
    let mut v = slots.to_vec();
    match failure {
        Failure::Identity(id) => {
            let Some(c) = bible.character(id) else { return v };
            let Some(m) = shot.marks.iter().find(|m| &m.character == id) else { return v };
            let at = v.iter().position(|r| r.role == RefRole::Identity(id.clone()));
            let current = at.map(|i| v[i].file.clone());
            let view = view_of(shot, m);
            // A bigger picture of the failing view: the best crop first, then the other sheets.
            let crop = crops_for(&c.crops, view, store).into_iter().map(|cr| cr.file.clone()).find(|f| Some(f) != current.as_ref() && !v.iter().any(|r| &r.file == f));
            let file = crop.or_else(|| identity_file(c, shot, m, store, current.as_deref()).map(|(f, _)| f).filter(|f| !v.iter().any(|r| &r.file == f)));
            let Some(file) = file else { return v };
            let Some(slot) = new_slot(store, file, RefRole::Identity(id.clone()), identity_job(c, view), SiteSlot::Character) else { return v };
            match at {
                Some(i) => v[i] = slot,
                None => {
                    let identities = v.iter().filter(|r| matches!(r.role, RefRole::Identity(_))).count();
                    if identities < MAX_IDENTITY && make_room(&mut v) {
                        let after = v.iter().rposition(|r| matches!(r.role, RefRole::Identity(_) | RefRole::Plate | RefRole::Layout)).map(|i| i + 1).unwrap_or(0);
                        v.insert(after, slot);
                    }
                }
            }
        }
        Failure::Prop(prop) => {
            let Some(file) = prop_sheet(bible, prop, store) else { return v };
            if v.iter().any(|r| r.file == file) {
                return v;
            }
            let name = bible.prop(prop).map(|p| if p.name.trim().is_empty() { p.id.clone() } else { p.name.trim().to_string() }).unwrap_or_else(|| prop.clone());
            let Some(slot) = new_slot(store, file, RefRole::Prop(prop.clone()), fill(jobs::PROP, &[("prop", &name)]), SiteSlot::Add) else { return v };
            if make_room(&mut v) {
                match v.iter().position(|r| r.role == RefRole::Grip(prop.clone())) {
                    Some(i) => v.insert(i + 1, slot),
                    None => v.push(slot),
                }
            }
        }
        Failure::Place => {
            let place = scene.place.as_deref().and_then(|id| bible.place(id));
            let Some(place) = place else { return v };
            let derived = geometry::derive(shot, Some(place));
            // Find the view on a picker that already holds the current list (no cap: room is
            // made below, only when there is a view to add).
            let mut p = Picker { store, out: v.clone(), cap: usize::MAX };
            let mut shot2 = shot.clone();
            shot2.background = BackgroundKind::Matched;
            if add_place_view(&mut p, &shot2, scene, place, &derived) {
                if let Some(slot) = p.out.pop() {
                    if make_room(&mut v) {
                        let at = v.iter().position(|r| r.role == RefRole::Panel).unwrap_or(v.len());
                        v.insert(at, slot);
                    }
                }
            }
        }
        Failure::Style => {
            if let Some(i) = v.iter().position(|r| r.role == RefRole::Style) {
                let s = v.remove(i);
                v.insert(0, s);
            }
        }
    }
    v
}
