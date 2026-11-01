//! `app_guide`: how MangaVox is organised and the order of steps, so any model (a small local one
//! included) can find its way without the whole tool table in its prompt.
use serde_json::{json, Value};

pub const TOPICS: &[&str] = &["overview", "manga", "studio", "export", "motion", "video", "media", "youtube", "safety"];

const OVERVIEW: &str = "MangaVox is a desktop app with three sections: Manga (library, sources, reader, downloads), Anime / Movies (search, play, watch history) and Studio (turns downloaded manga chapters into narrated recap videos, plus custom motion-graphic videos). Pages: library, browse, history, downloads, anime, movies, discover, favorites, media-history, studio, youtube, thumbnails, settings.";

const FLOW: &[&str] = &[
    "1. Find the manga: list_library (already in the library) or manga_search (sources) → manga_chapters with source + id.",
    "2. Keep it: manga_add_to_library (manga id from manga_chapters).",
    "3. Download the chapters: manga_download (exact chapters), follow with manga_downloads.",
    "4. Studio sees downloaded chapters: studio_library (with manga for each chapter's steps).",
    "5. Make recaps: studio_generate (all steps Read → Transcript → Story → Audio → Music → Video), or studio_run_steps for a range / a single step. Follow with studio_status; pause/resume/cancel/retry with studio_queue.",
    "6. Finish: studio_export (whole video with optional hook, parts, or reels), studio_play to watch, studio_youtube_package + studio_thumbnails for YouTube.",
];

fn topic(t: &str) -> (&'static str, Vec<&'static str>) {
    match t {
        "manga" => (
            "Manga: the library holds manga the user keeps; sources (plugins) are searched for new titles; chapters are downloaded through one app-wide queue.",
            vec![
                "list_library: the library (query, offset/limit).",
                "manga_sources / manga_search: search sources; results give source + id.",
                "manga_chapters: a title's chapters with download state (manga, or source + id).",
                "manga_add_to_library, open_manga (title or id), open_reader (manga + chapter).",
                "manga_download (big downloads ask first), manga_downloads (progress), manga_downloads_control (pause/resume/retry_failed/cancel), manga_delete_downloads (asks first).",
            ],
        ),
        "studio" => (
            "Studio turns downloaded chapters into recap videos in six steps per chapter: Read (OCR of pages and panels) → Transcript → Story (AI script) → Audio (narration voice) → Music → Video (render). Steps already done are skipped; redoing a step overwrites it and every later step.",
            vec![
                "studio_library: manga with downloaded chapters; with manga: each chapter's done steps.",
                "studio_generate: queue all steps for exact chapters (voice, format landscape/reels, captions, music, AI provider/model). redo_from asks first.",
                "studio_run_steps: run a range of steps (from … until), e.g. only Read → Story, or a single step (from = until).",
                "studio_status: queue state, current step, progress, errors. studio_queue: pause/resume/cancel/retry, *_all, clear_finished.",
                "studio_voices, studio_look (preset, effects, colour, watermark, logo), studio_captions (style, size, place).",
            ],
        ),
        "export" => (
            "Export joins every made chapter video of a manga in order, with the look's watermark.",
            vec![
                "studio_export: kind whole (optional AI hook intro) / parts / reels; overwrite only after the user agreed.",
                "studio_play: watch a chapter's video, an export or any video file in the app's player.",
                "studio_thumbnails: list thumbnail ideas and made thumbnails; studio_thumbnail_make spends Magnific credits (always asks).",
            ],
        ),
        "motion" => (
            "Custom motion-graphic videos (intros, trailers, character spotlights, lyric-style text) separate from the recap pipeline: describe scenes as a JSON spec and the app renders it.",
            vec![
                "motion_assets: what can go in a spec (a chapter's pages and panels, covers, Studio files, music, sound effects, voices).",
                "motion_preview: render one frame (at) or a 3-second clip to check the spec; fix the errors it reports.",
                "motion_compose: render the whole video into the Studio motion folder; returns the path and a preview frame. Then studio_play to watch it.",
                "Images: {\"manga\": title, \"chapter\": \"12\", \"page\": 3} (optional \"panel\": 1), {\"cover\": title}, {\"studio\": \"relative/path.png\"} or an absolute file path.",
            ],
        ),
        "video" => (
            "The Video editor edits one chapter's recap video: its timeline of clips, layers, captions, music and look, then renders.",
            vec!["video_open (manga + chapter), video_overview / video_timeline, video_edit_clip(s), video_split_clip, video_move_clip, video_add_layer, video_set_captions, video_set_music, video_set_look, video_render, video_export."],
        ),
        "media" => (
            "Anime, movies and series: search and open title pages; the user presses play there, or media_play starts a stream.",
            vec!["search_anime / open_anime, search_movies / open_movie, play_title, media_sources, media_play, media_download, media_downloads, get_watch_history."],
        ),
        "youtube" => (
            "The connected YouTube channel and the YouTube package of a recap.",
            vec!["youtube_channel, youtube_video, youtube_update_video, youtube_schedule, studio_youtube_package (title, description, thumbnail idea)."],
        ),
        "safety" => (
            "Act only on the manga and chapters the user named. Deletes, overwrites, big downloads, redoing steps and anything that spends credits (Magnific thumbnails) ask the user in the chat first; if they decline, do not retry. Never invent ids: list or search first.",
            vec![],
        ),
        _ => (OVERVIEW, FLOW.to_vec()),
    }
}

/// The guide for `topic` (empty or unknown: the overview with the main flow).
pub fn guide(t: &str) -> Value {
    let t = t.trim().to_lowercase();
    let key = if TOPICS.contains(&t.as_str()) { t.as_str() } else { "overview" };
    let (about, steps) = topic(key);
    let mut v = json!({ "topic": key, "about": about, "steps": steps });
    if key == "overview" {
        v["moreTopics"] = json!(TOPICS[1..].join(", "));
        v["tip"] = json!("Tools you do not have yet: find_tools {\"query\": \"what you want to do\"}.");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_has_text_and_unknown_falls_back() {
        for t in TOPICS {
            let g = guide(t);
            assert_eq!(g["topic"], *t);
            assert!(!g["about"].as_str().unwrap().is_empty());
        }
        let g = guide("nonsense");
        assert_eq!(g["topic"], "overview");
        assert!(g["steps"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("manga_download")));
    }
}
