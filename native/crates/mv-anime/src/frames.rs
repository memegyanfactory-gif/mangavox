//! FRAMES (design 6.6): the request a shot's still would be made with now, whether a card is
//! outdated, enqueuing frame jobs and running them (one submission per variant, each stored
//! as a take; the first take is selected only when nothing was).
//!
//! The picture channel (design 5.5): a frame is outdated when the request built now hashes
//! differently from the one that made its selected take. The request holds the final prompt, the
//! content hash of every reference file, the model, the aspect and the size, and nothing else:
//! a voice or a timing change never outdates a picture; a new face sheet or a moved camera does.
//!
//! The prompt: `prompt_ai` is always the compiled text; a user's own text (`prompt_user`) pins
//! the prompt until "Reset" ([`reset_prompt`]) brings the compiled one back.

use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};

use crate::geometry;
use crate::hash;
use crate::model::{
    BibleIndex, Card, Compiled, Derived, GenRequest, GenSpec, ProjectSettings, RefRole, RefSlot, SceneShots, Scene, Shot, Status, Take, TakeKind, TakeMeta, TakeSource, Target,
};
use crate::queue::{self, JobCtx, JobKind};
use crate::store::{self, Store};
use crate::{compile, slots};

/// The request template of a shot's frame (part of the request, so a new template outdates).
pub const TEMPLATE: &str = "nb21.frame@1";
/// The picture model when the project names none (the site's model id without `imagen-`).
pub const DEFAULT_MODEL: &str = "nano-banana-2-1";
/// The frame shape when the project names none.
pub const DEFAULT_ASPECT: &str = "16:9";
/// The picture size when the project names none (the only free one).
pub const DEFAULT_RES: &str = "1K";
/// Pictures per Generate when nothing says otherwise.
pub const DEFAULT_VARIANTS: u8 = 2;
/// Most pictures one Generate makes.
pub const MAX_VARIANTS: u8 = 4;

/// Everything about a shot's next picture, built now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prepared {
    /// The frame's spec with `prompt_ai` and `refs` as they would be sent (the user's prompt
    /// and pinned or hand-picked references kept).
    pub spec: GenSpec,
    pub compiled: Compiled,
    /// Exactly what is sent; its hash is the picture channel.
    pub request: GenRequest,
    pub derived: Derived,
    /// Sheets that would have been references but are not approved yet, so something else
    /// (an approved manga crop, or nothing) stands in for them, in plain words for the panel
    /// ("Kai's face sheet is not approved yet, …"). Empty when the user picked the references.
    pub left_out: Vec<String>,
}

/// The text sent: the user's own prompt when pinned, else the compiled one.
pub fn final_prompt(spec: &GenSpec) -> &str {
    match spec.prompt_user.as_deref() {
        Some(t) if !t.trim().is_empty() => t,
        _ => &spec.prompt_ai,
    }
}

/// Pins the user's own prompt (remembering which compiled prompt it replaced). Typing the
/// compiled text back un-pins it.
pub fn pin_prompt(spec: &mut GenSpec, text: &str) {
    if text == spec.prompt_ai || text.trim().is_empty() {
        reset_prompt(spec);
        return;
    }
    if spec.prompt_user.is_none() || spec.prompt_user_base.is_none() {
        spec.prompt_user_base = Some(hash::sha256_hex(spec.prompt_ai.as_bytes()));
    }
    spec.prompt_user = Some(text.to_string());
}

/// "Reset": the compiled prompt is sent again.
pub fn reset_prompt(spec: &mut GenSpec) {
    spec.reset_prompt()
}

/// The shot changed since the user pinned their prompt (the panel offers "Rewrite from shot /
/// Keep mine").
pub fn prompt_moved(spec: &GenSpec) -> bool {
    spec.prompt_moved()
}

/// A short stable label of a reference's role, sent in the request.
pub fn role_label(role: &RefRole) -> String {
    match role {
        RefRole::Plate => "plate".into(),
        RefRole::Layout => "layout".into(),
        RefRole::Identity(id) => format!("identity:{id}"),
        RefRole::Style => "style".into(),
        RefRole::Expression(id) => format!("expression:{id}"),
        RefRole::Prop(id) => format!("prop:{id}"),
        RefRole::Grip(id) => format!("grip:{id}"),
        RefRole::ColourModel(id) => format!("colour_model:{id}"),
        RefRole::PlaceView => "place_view".into(),
        RefRole::Panel => "panel".into(),
        RefRole::User => "user".into(),
        RefRole::Unknown => "unknown".into(),
    }
}

/// The request a spec sends (template, model, final prompt, reference hashes, aspect, size).
pub fn request_for(spec: &GenSpec) -> GenRequest {
    GenRequest {
        template: TEMPLATE.into(),
        model: spec.model.clone(),
        prompt: final_prompt(spec).to_string(),
        refs: spec.refs.iter().map(|r| (r.hash.clone(), role_label(&r.role), r.slot)).collect(),
        aspect: spec.aspect.clone(),
        res: spec.res.clone(),
    }
}

fn or<'a>(a: &'a str, b: &'a str, c: &'a str) -> String {
    [a, b, c].iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or(c).to_string()
}

/// Builds a shot's next picture from its data (no queue, no site). The references are picked
/// again unless the user chose them by hand (`refs_user`), in which case their files are
/// hashed again.
pub fn prepare_shot(shot: &Shot, scene: &Scene, bible: &BibleIndex, settings: &ProjectSettings, store: &Store) -> Prepared {
    let place = scene.place.as_deref().and_then(|id| bible.place(id));
    let derived = geometry::derive(shot, place);
    let gen = &shot.frame.gen;
    let refs = if gen.refs_user { slots::refresh(store, &gen.refs) } else { slots::pick_with(shot, scene, bible, store, &derived) };
    let mut spec = gen.clone();
    spec.model = or(&gen.model, &settings.picture_model, DEFAULT_MODEL);
    spec.aspect = or(&gen.aspect, &settings.aspect, DEFAULT_ASPECT);
    spec.res = or(&gen.res, &settings.picture_res, DEFAULT_RES);
    // The prompt names the frame shape that is actually requested.
    let compiled = if spec.aspect == gen.aspect {
        compile::frame_prompt(shot, scene, bible, &derived, &refs)
    } else {
        let mut s = shot.clone();
        s.frame.gen.aspect = spec.aspect.clone();
        compile::frame_prompt(&s, scene, bible, &derived, &refs)
    };
    spec.prompt_ai = compiled.text.clone();
    spec.refs = refs;
    if spec.variants == 0 {
        spec.variants = if settings.variants == 0 { DEFAULT_VARIANTS } else { settings.variants.min(MAX_VARIANTS) };
    }
    let request = request_for(&spec);
    let left_out = if gen.refs_user { Vec::new() } else { slots::left_out(shot, scene, bible, store, &derived) };
    Prepared { spec, compiled, request, derived, left_out }
}

/// Where a shot lives: its scene file, its index there, its scene, the bible and the settings.
struct Located {
    /// The key of its scene file (what [`Store::scene_shots`] and
    /// [`Store::update_scene_shots`] take).
    file: String,
    shots: SceneShots,
    index: usize,
    scene: Scene,
    bible: BibleIndex,
    settings: ProjectSettings,
}

impl Located {
    fn shot(&self) -> &Shot {
        &self.shots.shots[self.index]
    }
}

fn gone(shot: &str) -> anyhow::Error {
    anyhow!("Shot {shot} is not in this episode any more; it may have been removed when its scene was planned again.")
}

/// The key of the scene file holding `shot` (reads only the shot files).
fn scene_file_of(store: &Store, ep: &str, shot: &str) -> Result<String> {
    store.all_shots(ep)?.into_iter().find(|(_, v)| v.iter().any(|s| s.id == shot)).map(|(k, _)| k).ok_or_else(|| gone(shot))
}

fn locate(store: &Store, ep: &str, shot: &str) -> Result<Located> {
    if shot.trim().is_empty() {
        bail!("No shot was given.");
    }
    for scene_id in store.all_shots(ep)?.into_iter().filter(|(_, v)| v.iter().any(|s| s.id == shot)).map(|(k, _)| k) {
        let shots = store.scene_shots(ep, &scene_id)?;
        let Some(index) = shots.shots.iter().position(|s| s.id == shot) else { continue };
        let story = store.story(ep)?;
        let scene = story.scenes.into_iter().find(|s| s.id == shots.scene || s.id == scene_id).unwrap_or_else(|| Scene { id: scene_id.clone(), ..Default::default() });
        let bible = store.bible()?;
        let settings = store.series().map(|s| s.settings).unwrap_or_default();
        return Ok(Located { file: scene_id, shots, index, scene, bible, settings });
    }
    Err(gone(shot))
}

/// Changes shot `shot` in its scene file as it is on disk now, under that file's own lock: the
/// file is read fresh, `f` runs on it (with the shot's index) and the result is saved, so an edit
/// saved meanwhile by a screen or another job is never lost. `f` must be quick and save nothing;
/// when it answers an error, the file is left as it was. When the shot moved to another scene
/// file meanwhile, that file is changed instead.
fn edit_shot<R>(store: &Store, ep: &str, file: &str, shot: &str, f: impl FnOnce(&mut SceneShots, usize) -> Result<R>) -> Result<R> {
    let mut f = Some(f);
    let mut file = file.to_string();
    for attempt in 0..2 {
        let mut out: Option<Result<R>> = None;
        let mut run = |shots: &mut SceneShots| {
            let Some(index) = shots.shots.iter().position(|s| s.id == shot) else { return };
            let Some(f) = f.take() else { return };
            let before = shots.clone();
            let r = f(shots, index);
            if r.is_err() {
                *shots = before;
            }
            out = Some(r);
        };
        let saved = store.update_scene_shots(ep, &file, &mut run);
        match (saved, out) {
            (Ok(_), Some(r)) => return r,
            (Err(e), Some(_)) => return Err(e),
            (saved, None) => {
                if attempt == 1 {
                    return Err(saved.err().unwrap_or_else(|| gone(shot)));
                }
                // Not in that file any more (or the file is gone): find the shot again.
                file = scene_file_of(store, ep, shot)?;
            }
        }
    }
    Err(gone(shot))
}

/// A shot's next picture, built from the project on disk.
pub fn prepare(store: &Store, ep: &str, shot: &str) -> Result<Prepared> {
    let loc = locate(store, ep, shot)?;
    Ok(prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store))
}

/// The request the engine would send for the shot's frame now (its hash is the picture channel).
/// An unknown shot gives an empty request.
pub fn current_request(store: &Store, ep: &str, shot: &str) -> GenRequest {
    match prepare(store, ep, shot) {
        Ok(p) => p.request,
        Err(e) => {
            mv_core::logging::global().warn("anime", &format!("frame request of {shot}: {e:#}"));
            GenRequest::default()
        }
    }
}

/// The selected take was made from a different request than the current one. A card the user
/// chose to keep (`locked`), a card with nothing selected, or a selected take that is not a
/// shot frame (an upload) is never outdated.
pub fn outdated(card: &Card, store: &Store) -> bool {
    if card.locked {
        return false;
    }
    let Some(take) = card.selected.as_deref().and_then(|id| store.take(id)) else { return false };
    let Target::ShotFrame { episode, shot } = &take.target else { return false };
    let Ok(loc) = locate(store, episode, shot) else { return false };
    let now = hash::request_hash(&prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store).request);
    outdated_given(card, Some(&take), &now, accepted(&loc.shots, &take.id).as_deref())
}

/// [`outdated`] when the caller already has the selected take and the hash of the request built
/// now (a screen that prepared every shot). It does not know what the user accepted on
/// approval: a screen that has the scene's shots file uses [`outdated_given`].
pub fn outdated_for(card: &Card, selected: Option<&Take>, request_hash: &str) -> bool {
    outdated_given(card, selected, request_hash, None)
}

/// [`outdated_for`], with the request hash the user accepted for the selected picture when they
/// approved it ([`accepted`]): an approved picture whose accepted request is the current one is
/// not outdated.
pub fn outdated_given(card: &Card, selected: Option<&Take>, request_hash: &str, accepted_hash: Option<&str>) -> bool {
    if card.locked {
        return false;
    }
    let Some(take) = selected else { return false };
    if !matches!(take.target, Target::ShotFrame { .. }) || take.request_hash == request_hash {
        return false;
    }
    !(card.status == Status::Approved && accepted_hash == Some(request_hash))
}

/// Writes the compiled prompt and the picked references into the shot (the user's own prompt
/// and hand-picked references are kept), so the panel shows what Generate would send.
pub fn recompile(store: &Store, ep: &str, shot: &str) -> Result<Prepared> {
    // Everything slow (the bible, the hashes) first; then only the two fields, on the file as
    // it is now.
    let loc = locate(store, ep, shot)?;
    let prepared = prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store);
    let changed = {
        let gen = &loc.shot().frame.gen;
        gen.prompt_ai != prepared.spec.prompt_ai || (!gen.refs_user && gen.refs != prepared.spec.refs)
    };
    if changed {
        edit_shot(store, ep, &loc.file, shot, |shots, i| {
            let gen = &mut shots.shots[i].frame.gen;
            gen.prompt_ai = prepared.spec.prompt_ai.clone();
            if !gen.refs_user {
                gen.refs = prepared.spec.refs.clone();
            }
            Ok(())
        })?;
    }
    Ok(prepared)
}

/// Pins the user's own prompt on a shot (`None` or the compiled text = Reset).
pub fn set_prompt(store: &Store, ep: &str, shot: &str, text: Option<&str>) -> Result<()> {
    let loc = locate(store, ep, shot)?;
    let prepared = prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store);
    edit_shot(store, ep, &loc.file, shot, |shots, i| {
        let gen = &mut shots.shots[i].frame.gen;
        gen.prompt_ai = prepared.spec.prompt_ai;
        match text {
            Some(t) => pin_prompt(gen, t),
            None => reset_prompt(gen),
        }
        Ok(())
    })
}

/// Selects one of the frame's takes (restore from history).
pub fn select_take(store: &Store, ep: &str, shot: &str, take: &str) -> Result<()> {
    if shot.trim().is_empty() {
        bail!("No shot was given.");
    }
    let file = scene_file_of(store, ep, shot)?;
    edit_shot(store, ep, &file, shot, |shots, i| {
        let frame = &mut shots.shots[i].frame;
        if !frame.takes.iter().any(|t| t == take) {
            bail!("That picture is not one of this shot's pictures.");
        }
        if frame.selected.as_deref() != Some(take) {
            frame.selected = Some(take.to_string());
            if frame.status == Status::Approved {
                frame.status = Status::NeedsReview;
            }
        }
        Ok(())
    })
}

/// Approves the selected picture (no automatic approval in Beta 1) and accepts it as it is: the
/// request the engine would send now is remembered with it ([`accept`]), so an "outdated" mark
/// clears until the shot, its references or the settings change again. The Frames screen calls
/// this when the user approves.
pub fn approve(store: &Store, ep: &str, shot: &str) -> Result<()> {
    let loc = locate(store, ep, shot)?;
    let now = hash::request_hash(&prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store).request);
    // The picture approved is the one selected now (the user may have picked another meanwhile).
    edit_shot(store, ep, &loc.file, shot, |shots, i| {
        let Some(take) = shots.shots[i].frame.selected.clone() else { bail!("Pick a picture for this shot first, then approve it.") };
        shots.shots[i].frame.status = Status::Approved;
        accept(shots, &take, &now);
        Ok(())
    })
}

/// Key in a scene's shots file (`SceneShots.extra`): `{take id: request hash}` of the pictures
/// the user approved as they are, though the request had changed since they were drawn.
pub const ACCEPTED: &str = "accepted_requests";

/// Remembers that the user accepted picture `take` for the request hashing to `request_hash`
/// (a screen that approves a frame in its own copy of the shots file calls this before saving).
pub fn accept(shots: &mut SceneShots, take: &str, request_hash: &str) {
    let cards: Vec<&str> = shots.shots.iter().flat_map(|s| s.frame.takes.iter().map(String::as_str)).collect();
    let mut map = shots.extra.get(ACCEPTED).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    // Entries of pictures no shot has any more go.
    map.retain(|k, _| cards.contains(&k.as_str()) && k != take);
    // A shot accepts one request: its other pictures' entries go too.
    if let Some(s) = shots.shots.iter().find(|s| s.frame.takes.iter().any(|t| t == take)) {
        map.retain(|k, _| !s.frame.takes.contains(k));
    }
    map.insert(take.to_string(), serde_json::Value::String(request_hash.to_string()));
    shots.extra.insert(ACCEPTED.to_string(), serde_json::Value::Object(map));
}

/// The request hash the user accepted for picture `take`, if any.
pub fn accepted(shots: &SceneShots, take: &str) -> Option<String> {
    shots.extra.get(ACCEPTED)?.get(take)?.as_str().map(str::to_string)
}

/// Display numbers ("3.4") of an episode's shots, by scene order then shot order.
fn shot_numbers(store: &Store, ep: &str) -> Vec<(String, String)> {
    let order: Vec<String> = match (store.episode(ep), store.story(ep)) {
        (Ok(e), _) if !e.scene_order.is_empty() => e.scene_order,
        (_, Ok(s)) => s.scenes.into_iter().map(|s| s.id).collect(),
        _ => Vec::new(),
    };
    let all = store.all_shots(ep).unwrap_or_default();
    let mut out = Vec::new();
    for (i, scene) in order.iter().enumerate() {
        if let Some(shots) = all.get(scene) {
            out.extend(shots.iter().enumerate().map(|(j, s)| (s.id.clone(), format!("{}.{}", i + 1, j + 1))));
        }
    }
    out
}

/// How many pictures one job makes.
fn count(asked: u8, spec: u8) -> u8 {
    let n = if asked > 0 { asked } else if spec > 0 { spec } else { DEFAULT_VARIANTS };
    n.clamp(1, MAX_VARIANTS)
}

/// One Frame job per shot.
pub fn enqueue_frames(store: &Store, ep: &str, shots: &[String], variants: u8) {
    let numbers = shot_numbers(store, ep);
    let settings_variants = store.series().map(|s| s.settings.variants).unwrap_or(0);
    let variants = count(variants, settings_variants);
    for id in shots.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let label = match numbers.iter().find(|(s, _)| s == id) {
            Some((_, n)) => format!("Picture for shot {n}"),
            None => format!("Picture for shot {id}"),
        };
        queue::enqueue(JobKind::Frame { episode: ep.to_string(), shot: id.to_string(), variants }, &store.id, label);
    }
}

/// The reference files in upload order; a missing one stops the job with what to do.
fn resolve(store: &Store, refs: &[RefSlot]) -> Result<Vec<(PathBuf, RefSlot)>> {
    refs.iter()
        .map(|r| {
            let ok = !r.file.trim().is_empty() && !r.file.replace('\\', "/").split('/').any(|p| p == ".." || p.contains(':'));
            let path = store.path(&r.file);
            if !ok || !path.is_file() {
                let what = if r.job.trim().is_empty() { role_label(&r.role) } else { r.job.clone() };
                bail!("A reference picture of this shot is missing ({what}). Make it again in the Bible step, or remove it from the shot's references, then try again.");
            }
            Ok((path, r.clone()))
        })
        .collect()
}

/// Stores one picture as a take of the shot's frame.
fn save_take(store: &Store, bytes: &[u8], req: &GenRequest, request_hash: &str, ep: &str, shot: &str, batch: &str) -> Result<String> {
    let img = image::load_from_memory(bytes).map_err(|_| anyhow!("Magnific sent back something that is not a picture, so it was not kept. Try again."))?;
    let ext = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => "jpg",
        Ok(image::ImageFormat::WebP) => "webp",
        _ => "png",
    };
    // Magnific's own id of the picture and who pressed Generate, kept with the take and in the
    // list of pictures MangaVox made (never in the prompt).
    let what = match shot_numbers(store, ep).into_iter().find(|(s, _)| s == shot) {
        Some((_, n)) => format!("Frame {n}"),
        None => format!("Frame {shot}"),
    };
    let mark = store::magnific_picture_made(store, bytes, &req.model, &what, &req.prompt).unwrap_or_default();
    let meta = TakeMeta {
        kind: TakeKind::Image,
        target: Target::ShotFrame { episode: ep.to_string(), shot: shot.to_string() },
        request: Some(req.clone()),
        request_hash: request_hash.to_string(),
        source: TakeSource::MagnificWeb { model: req.model.clone(), res: req.res.clone(), free: true, site_id: mark.site_id.clone() },
        w: img.width(),
        h: img.height(),
        seconds: 0.,
        batch: Some(batch.to_string()),
        note: if mark.site_id.is_empty() || mark.pressed_by_app { String::new() } else { store::PRESSED_AT_WINDOW.to_string() },
    };
    Ok(store.add_take(bytes, ext, meta)?.id)
}

/// What the Frame job read of the shot when it started: where it lives and its picture settings
/// as saved then (before the project's defaults fill them in).
struct Started {
    file: String,
    gen: GenSpec,
}

/// Puts the job's value of one picture setting back, only where the setting still holds what the
/// job read at its start: a value the user chose while the picture was drawing is kept.
fn put_back<T: Clone + PartialEq>(now: &mut T, at_start: &T, made_with: &T) {
    if *now == *at_start {
        *now = made_with.clone();
    }
}

/// Adds new takes to the shot, on the shot file as it is now and under its own lock, so edits
/// made meanwhile are kept: takes appended, the first selected only when nothing was, status
/// "needs review". The settings that made them (prompt, references, model, shape, size) are
/// written back only where the shot still holds what the job read at its start.
fn record(store: &Store, ep: &str, shot: &str, started: &Started, prepared: &Prepared, new: &[String]) -> Result<()> {
    if new.is_empty() {
        return Ok(());
    }
    edit_shot(store, ep, &started.file, shot, |shots, i| {
        let frame = &mut shots.shots[i].frame;
        for id in new {
            if !frame.takes.contains(id) {
                frame.takes.push(id.clone());
            }
        }
        if frame.selected.is_none() {
            frame.selected = new.first().cloned();
        }
        // More variants to compare do not take back an approval: the approved picture is still
        // the selected one (as with bible cards).
        if frame.status != Status::Approved {
            frame.status = Status::NeedsReview;
        }
        let (gen, start, made) = (&mut frame.gen, &started.gen, &prepared.spec);
        put_back(&mut gen.prompt_ai, &start.prompt_ai, &made.prompt_ai);
        if !gen.refs_user && !start.refs_user {
            put_back(&mut gen.refs, &start.refs, &made.refs);
        }
        put_back(&mut gen.model, &start.model, &made.model);
        put_back(&mut gen.aspect, &start.aspect, &made.aspect);
        put_back(&mut gen.res, &start.res, &made.res);
        Ok(())
    })
}

/// The Frame job: builds the request, resolves the reference files, then asks the picture
/// backend for one picture per variant (separate submissions), storing each as a take as soon
/// as it arrives. A failure keeps every earlier take (the shot's and this run's).
pub async fn run_job(cx: &JobCtx<'_>, episode: &str, shot: &str, variants: u8) -> Result<()> {
    let store = cx.store;
    (cx.progress)(0., "Getting the shot ready");
    let loc = locate(store, episode, shot)?;
    let started = Started { file: loc.file.clone(), gen: loc.shot().frame.gen.clone() };
    let prepared = prepare_shot(loc.shot(), &loc.scene, &loc.bible, &loc.settings, store);
    drop(loc);
    let files = resolve(store, &prepared.spec.refs)?;
    let n = count(variants, prepared.spec.variants);
    let req = prepared.request.clone();
    let request_hash = hash::request_hash(&req);
    let batch = format!("{shot}-{}", store::now_ms());
    let mut made: Vec<String> = Vec::new();
    for i in 0..n {
        if cx.cancel.is_cancelled() {
            bail!("Stopped.");
        }
        let progress = cx.progress;
        let at = move |part: f32| (i as f32 + part) / n as f32;
        progress(at(0.), &format!("Picture {} of {n}: sending to Magnific", i + 1));
        let on_status = Box::new(move |s: &str| progress(at(0.5), &format!("Picture {} of {n}: {s}", i + 1)));
        let answer = tokio::select! {
            r = cx.run.image.generate(&req, &files, 1, on_status) => r,
            _ = cx.cancel.cancelled() => bail!("Stopped."),
        };
        match answer {
            Ok(images) => {
                let mut fresh = Vec::new();
                for bytes in &images {
                    fresh.push(save_take(store, bytes, &req, &request_hash, episode, shot, &batch)?);
                }
                // Kept at once, so a later failure cannot lose this picture.
                record(store, episode, shot, &started, &prepared, &fresh)?;
                made.extend(fresh);
            }
            Err(e) => return Err(anyhow::Error::new(e)),
        }
    }
    if made.is_empty() {
        bail!("Magnific answered without a picture. Try again.");
    }
    (cx.progress)(1., &format!("{} new picture{}", made.len(), if made.len() == 1 { "" } else { "s" }));
    Ok(())
}
