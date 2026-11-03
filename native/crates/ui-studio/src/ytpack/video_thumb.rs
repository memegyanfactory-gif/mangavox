//! Which made thumbnail a manga's videos show (`mv_recap::thumb::Picks`): the one picked for the
//! manga, else the newest made on the Thumbnails page, and a video's own pick over both. After a
//! change the manga's base and every existing video thumbnail (manga page, reader, phone) are drawn
//! again in the background, and the old pictures leave the image cache.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gpui_kit::*;
use mv_recap::thumb::{self, Picks};

/// Bumped after a manga's video thumbnails were drawn again, for views that show them.
#[derive(Default)]
pub(crate) struct VideoThumbsChanged(pub u64);
impl Global for VideoThumbsChanged {}

/// [`in_use`] per manga, read again after a pick or a gallery change ([`apply`]).
static IN_USE: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);

/// The made thumbnail the manga's videos use now (picked, else the newest 16:9 one). Cached:
/// views call it on every render.
pub(crate) fn in_use(manga_id: &str) -> Option<PathBuf> {
    if let Some(hit) = IN_USE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(manga_id).cloned()) {
        return hit;
    }
    let found = thumb::manga_pick(manga_id);
    IN_USE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(manga_id.to_string(), found.clone());
    found
}

fn forget(manga_id: &str) {
    if let Some(m) = IN_USE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(manga_id);
    }
}

/// Draws the manga's base and video thumbnails again now (blocking; for work that has no App,
/// such as the assistant's tools). Returns the files that changed.
pub(crate) fn redraw_blocking(manga_id: &str) -> anyhow::Result<Vec<PathBuf>> {
    forget(manga_id);
    let base_changed = thumb::sync_picked(manga_id)?;
    let mut changed = thumb::refresh_badges(manga_id);
    if base_changed {
        changed.push(mv_recap::media::manga_dir(manga_id).join(thumb::BASE_PICKED));
    }
    forget(manga_id);
    Ok(changed)
}

/// The prompt a made thumbnail was made with (its gallery entry).
fn prompt_of(manga_id: &str, image: &Path) -> Option<String> {
    let id = image.file_stem()?.to_string_lossy().into_owned();
    super::thumbs::Gallery::load(manga_id).thumbs.into_iter().find(|t| t.id == id).map(|t| t.prompt).filter(|p| !p.trim().is_empty())
}

/// The Brain learns a thumbnail was made with `prompt` on `model` in `aspect`.
pub(crate) fn learn_made(prompt: &str, model: &str, aspect: &str) {
    use mv_core::brain::{observe, Kind};
    observe(Kind::Prompt, prompt, "Made a thumbnail with it", 0.2, "Thumbnails page");
    observe(Kind::Style, &format!("{model} · {aspect}"), "Used to make a thumbnail", 0.4, "Thumbnails page");
}

/// The Brain learns a made thumbnail was thrown away.
pub(crate) fn learn_deleted(prompt: &str) {
    mv_core::brain::observe(mv_core::brain::Kind::Prompt, prompt, "Deleted the thumbnail made with it", -1., "Thumbnails page");
}

fn learn_picked(manga_id: &str, image: &Path) {
    if let Some(p) = prompt_of(manga_id, image) {
        mv_core::brain::observe(mv_core::brain::Kind::Prompt, &p, "Picked its thumbnail for a video", 1., "Thumbnails page");
    }
}

/// `image` for every video of the manga (`None`: back to the newest made).
pub(crate) fn pick_for_manga(manga_id: &str, image: Option<PathBuf>, cx: &mut App) {
    if let Some(i) = &image {
        learn_picked(manga_id, i);
    }
    let dir = mv_recap::media::manga_dir(manga_id);
    let mut p = Picks::load(&dir);
    p.manga = image;
    if let Err(e) = p.save(&dir) {
        eprintln!("[studio] video thumbnail pick: {e:#}");
    }
    apply(manga_id, cx);
}

/// `image` for one video (`None`: it uses the manga's again).
pub(crate) fn pick_for_video(manga_id: &str, video: &Path, image: Option<PathBuf>, cx: &mut App) {
    if let Some(i) = &image {
        learn_picked(manga_id, i);
    }
    let dir = mv_recap::media::manga_dir(manga_id);
    let mut p = Picks::load(&dir);
    let key = thumb::video_key(&dir, video);
    match image {
        Some(i) => p.videos.insert(key, i),
        None => p.videos.remove(&key),
    };
    if let Err(e) = p.save(&dir) {
        eprintln!("[studio] video thumbnail pick: {e:#}");
    }
    apply(manga_id, cx);
}

/// The image a video shows: its own pick, else the manga's made thumbnail, else the base drawn
/// from the cover.
pub(crate) fn for_video(manga_id: &str, video: &Path) -> Option<PathBuf> {
    let dir = mv_recap::media::manga_dir(manga_id);
    Picks::load(&dir).for_video(&dir, video).or_else(|| in_use(manga_id)).or_else(|| thumb::pick_base(&dir).map(|b| b.0))
}

/// The made thumbnails changed or another was picked: draws the base and the video thumbnails
/// again in the background.
pub(crate) fn apply(manga_id: &str, cx: &mut App) {
    forget(manga_id);
    let id = manga_id.to_string();
    cx.spawn(async move |cx| {
        let res = mv_ui::bg::blocking(move || redraw_blocking(&id)).await.and_then(|r| r);
        let _ = cx.update(|cx| match res {
            Ok(changed) => redrawn(changed, cx),
            Err(e) => eprintln!("[studio] video thumbnails: {e:#}"),
        });
    })
    .detach();
}

/// After [`redraw_blocking`]: the old pictures leave the image cache and views read again.
pub(crate) fn redrawn(changed: Vec<PathBuf>, cx: &mut App) {
    for p in changed {
        ImageSource::from(p).remove_asset(cx);
    }
    let n = cx.try_global::<VideoThumbsChanged>().map(|g| g.0).unwrap_or(0);
    cx.set_global(VideoThumbsChanged(n + 1));
}
