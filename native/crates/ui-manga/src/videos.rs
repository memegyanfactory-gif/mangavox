//! A manga's Studio videos in the manga area (its page and the reader): the chapter videos in
//! reading order, "Play all" (a playlist the player plays back to back, no re-encode), and the
//! one shared 16:9 thumbnail with a local chapter badge (`mv_recap::thumb`: one AI image per
//! manga, every video's badge drawn here). Studio videos are local files with no catalog id, so
//! the player keeps them out of watch history.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_core::repo::chapter::Chapter;
use mv_recap::media::{self, ChapterVideo};
use mv_recap::thumb::{self, BaseKind};
use mv_ui::theme::{icon as icon_px, radius};
use mv_ui::{ph, ph_icon, Tokens, Weight};

use crate::logic;
use crate::reader::open_player;
use crate::recap::RecapOutputs;

/// The manga's cover file, else the first page of one of its downloaded chapters.
pub(crate) fn cover_file(cover_url: Option<&str>, chapters: &[Chapter]) -> Option<PathBuf> {
    cover_url.and_then(mv_core::covers::cached_cover).filter(|p| p.is_file()).or_else(|| chapters.iter().filter(|c| c.download_status == "complete").find_map(first_page))
}

fn first_page(c: &Chapter) -> Option<PathBuf> {
    let rd = std::fs::read_dir(c.pages_dir.as_deref()?).ok()?;
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["jpg", "jpeg", "png", "webp"].contains(&e.to_ascii_lowercase().as_str()))).collect();
    v.sort();
    v.into_iter().next()
}

/// The chapters Studio made a video of, in reading order, one per chapter number. Of two copies
/// of a chapter number the earlier added stays, as in Studio's list (`ui_studio::data`: the
/// library in chapter number then added order, the copy with a video kept), so Play all and
/// Studio's Play / Export use the same files.
pub(crate) fn chapter_videos(chapters: &[Chapter], recaps: &HashMap<String, RecapOutputs>) -> Vec<ChapterVideo> {
    let mut by_added: Vec<&Chapter> = chapters.iter().collect();
    by_added.sort_by(|a, b| a.added_at.cmp(&b.added_at));
    let mut v: Vec<ChapterVideo> = by_added
        .into_iter()
        .filter_map(|c| {
            let r = recaps.get(&c.id)?;
            Some(ChapterVideo { chapter_id: c.id.clone(), number: c.number, label: logic::chapter_label(c), video: r.video.clone()?, thumbnail: Some(r.dir.join(media::THUMBNAIL)).filter(|p| p.is_file()) })
        })
        .collect();
    media::sort_videos(&mut v);
    // The same chapter from two sources is one video in the list.
    v.dedup_by(|a, b| a.number.is_some() && a.number == b.number);
    v
}

/// The thumbnails drawn for a manga's videos.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Thumbs {
    /// Chapter id -> its `thumbnail.jpg`.
    pub chapters: HashMap<String, PathBuf>,
    /// The continuous video's ("Ch. 1–12"), when there are several videos.
    pub full: Option<PathBuf>,
    /// The manga has its AI image from the cover.
    pub ai: bool,
    /// They are drawn on a thumbnail made on Studio's Thumbnails page (which wins over the AI
    /// image).
    pub picked: bool,
}

impl Thumbs {
    /// The one picture that stands for all of the manga's videos.
    pub fn shared(&self, videos: &[ChapterVideo]) -> Option<PathBuf> {
        self.full.clone().or_else(|| videos.first().and_then(|v| self.chapters.get(&v.chapter_id).cloned()))
    }
}

/// Draws (or reuses) every video's thumbnail on the manga's one base image. Local only, no
/// credits; blocking.
pub(crate) fn make_thumbs(manga_id: &str, title: &str, cover: Option<&Path>, videos: &[ChapterVideo]) -> Result<Thumbs> {
    let dir = media::manga_dir(manga_id);
    // A thumbnail made or picked on Studio's Thumbnails page is the base when there is one.
    if let Err(e) = thumb::sync_picked(manga_id) {
        eprintln!("[manga] picked thumbnail: {e:#}");
    }
    let mut chapters = HashMap::new();
    for v in videos {
        let Some(out) = v.video.parent() else { continue };
        chapters.insert(v.chapter_id.clone(), thumb::chapter_thumbnail(&dir, cover, title, out, &thumb::badge_label(&[v.number]))?);
    }
    let full = if videos.len() > 1 {
        let numbers: Vec<Option<f64>> = videos.iter().map(|v| v.number).collect();
        Some(thumb::full_thumbnail(&dir, cover, title, &thumb::badge_label(&numbers))?)
    } else {
        None
    };
    let ai = dir.join(thumb::BASE_AI).metadata().is_ok_and(|m| m.len() > 0);
    Ok(Thumbs { chapters, full, ai, picked: thumb::pick_base(&dir).is_some_and(|b| b.1 == BaseKind::Picked) })
}

/// Paints the manga's one AI image (Magnific, spends credits): `force` replaces it (Regenerate).
pub(crate) async fn paint(manga_id: String, title: String, cover: PathBuf, force: bool) -> Result<PathBuf> {
    mv_ui::bg::run(async move { thumb::paint_base(&manga_id, &title, &cover, force).await }).await.and_then(|r| r)
}

/// "Make AI thumbnail · ~75 credits" and the like, for buttons and tooltips.
pub(crate) fn cost_hint() -> String {
    thumb::paint_cost().map(|c| format!(" · ~{} credits", c.round())).unwrap_or_default()
}

/// A thumbnail file was drawn again: drop the old picture from the image cache.
pub(crate) fn forget_images<'a>(paths: impl IntoIterator<Item = &'a PathBuf>, cx: &mut App) {
    for p in paths {
        ImageSource::from(p.clone()).remove_asset(cx);
    }
}

/// Plays every chapter video back to back in reading order, after Studio's check for changes not
/// rendered yet: Studio's joined `full_recap.mp4` (what its dashboard plays) while it was made from
/// exactly these files as they are now, else a playlist of them (nothing is re-encoded).
pub(crate) fn play_all(manga_id: &str, manga_title: &str, videos: &[ChapterVideo], window: &mut Window, cx: &mut App) {
    let files: Vec<PathBuf> = videos.iter().map(|v| v.video.clone()).collect();
    let title = format!("{manga_title} · {}", thumb::badge_label(&videos.iter().map(|v| v.number).collect::<Vec<_>>()));
    let (id, list) = (manga_id.to_string(), videos.to_vec());
    crate::recap::guarded(
        files.clone(),
        move |w, cx| {
            if let Some(full) = media::joined_if_current(&media::full_recap_path(&id), &files) {
                return open_player(title, &full, w, cx);
            }
            match media::write_playlist(&media::manga_dir(&id), &list) {
                Ok(p) => open_player(title, &p, w, cx),
                Err(e) => mv_ui::toast::show(cx, mv_ui::toast::Toast::error(format!("Could not start Play all: {e:#}"))),
            }
        },
        window,
        cx,
    );
}

/// A 16:9 video picture `w` px wide with a play mark; a quiet placeholder while it is drawn.
pub(crate) fn thumb_card(path: Option<PathBuf>, w: f32, play: bool) -> Div {
    let h = w * 9. / 16.;
    div()
        .relative()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .rounded(px(radius::MD))
        .overflow_hidden()
        .bg(Tokens::surface_3())
        .map(|d| match path {
            Some(p) => d.child(img(p).size_full().object_fit(ObjectFit::Cover)),
            None => d.flex().items_center().justify_center().child(ph_icon(ph::FILM_STRIP, Weight::Regular, Tokens::text_3(), icon_px::MD)),
        })
        .when(play, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px((h * 0.42).clamp(16., 44.))).rounded_full().flex().items_center().justify_center().bg(Tokens::glass_strong()).child(ph_icon(ph::PLAY, Weight::Fill, Tokens::text_1(), if h < 60. { icon_px::XS } else { icon_px::MD }))),
            )
        })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the GPUI prelude has its own `test` attribute.
    use super::{chapter_videos, cover_file, media, Chapter, HashMap, Path, PathBuf, RecapOutputs, Thumbs};

    fn ch(id: &str, n: Option<f64>) -> Chapter {
        Chapter {
            id: id.into(),
            manga_id: "m".into(),
            source_plugin: String::new(),
            source_id: id.into(),
            number: n,
            title: None,
            language: "en".into(),
            page_count: None,
            download_status: "complete".into(),
            ocr_status: String::new(),
            tts_status: String::new(),
            video_status: String::new(),
            read_progress: 0,
            is_read: false,
            pages_dir: None,
            added_at: String::new(),
        }
    }

    fn recap(dir: &Path, video: bool) -> RecapOutputs {
        RecapOutputs { dir: dir.to_path_buf(), video: video.then(|| dir.join(media::VIDEO)), sync_map: None }
    }

    #[test]
    fn videos_in_reading_order_and_one_shared_thumbnail() {
        let t = tempfile::tempdir().unwrap();
        let chapters = vec![ch("c10", Some(10.)), ch("x", None), ch("c2", Some(2.)), ch("c2b", Some(2.)), ch("c1.5", Some(1.5)), ch("none", Some(3.))];
        let mut recaps = HashMap::new();
        for c in &chapters {
            let d = t.path().join(&c.id);
            std::fs::create_dir_all(&d).unwrap();
            recaps.insert(c.id.clone(), recap(&d, c.id != "none"));
        }
        let v = chapter_videos(&chapters, &recaps);
        // By number with decimals, unnumbered last, one per number, only chapters with a video.
        assert_eq!(v.iter().map(|v| v.chapter_id.as_str()).collect::<Vec<_>>(), ["c1.5", "c2", "c10", "x"]);
        assert_eq!(v[0].label, "Ch. 1.5");
        // A drawn thumbnail is picked up.
        std::fs::write(t.path().join("c2").join(media::THUMBNAIL), "jpg").unwrap();
        assert_eq!(chapter_videos(&chapters, &recaps)[1].thumbnail, Some(t.path().join("c2").join(media::THUMBNAIL)));
        // The shared picture: the continuous video's, else the first chapter's.
        let th = Thumbs { chapters: HashMap::from([("c1.5".to_string(), PathBuf::from("a.jpg"))]), full: None, ai: false, picked: false };
        assert_eq!(th.shared(&v), Some(PathBuf::from("a.jpg")));
        assert_eq!(Thumbs { full: Some(PathBuf::from("f.jpg")), ..th }.shared(&v), Some(PathBuf::from("f.jpg")));
    }

    #[test]
    fn a_chapter_added_twice_plays_the_copy_studio_uses() {
        // Studio keeps the earlier added copy of a chapter number (both have a video), whatever
        // order the manga page lists them in.
        let t = tempfile::tempdir().unwrap();
        let mut late = ch("late", Some(4.));
        late.added_at = "2026-02-01".into();
        let mut early = ch("early", Some(4.));
        early.added_at = "2026-01-01".into();
        let chapters = vec![late, early];
        let mut recaps = HashMap::new();
        for c in &chapters {
            let d = t.path().join(&c.id);
            std::fs::create_dir_all(&d).unwrap();
            recaps.insert(c.id.clone(), recap(&d, true));
        }
        let v = chapter_videos(&chapters, &recaps);
        assert_eq!(v.iter().map(|v| v.chapter_id.as_str()).collect::<Vec<_>>(), ["early"]);
        assert_eq!(v[0].video, t.path().join("early").join(media::VIDEO));
    }

    #[test]
    fn cover_falls_back_to_a_page() {
        let t = tempfile::tempdir().unwrap();
        let pages = t.path().join("pages");
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("002.jpg"), "x").unwrap();
        std::fs::write(pages.join("001.png"), "x").unwrap();
        std::fs::write(pages.join("notes.txt"), "x").unwrap();
        let mut c = ch("c", Some(1.));
        c.pages_dir = Some(pages.to_string_lossy().into_owned());
        assert_eq!(cover_file(None, &[c.clone()]), Some(pages.join("001.png")));
        let cover = t.path().join("cover.jpg");
        std::fs::write(&cover, "x").unwrap();
        assert_eq!(cover_file(cover.to_str(), &[c]), Some(cover));
        assert_eq!(cover_file(None, &[]), None);
    }
}
