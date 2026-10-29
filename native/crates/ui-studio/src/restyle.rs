//! The dashboard's "Music & subtitles" sheet: change the music (keep, new take, mute, how loud) and
//! the subtitles (template, place, size) of chapters that already went through Generate, for one
//! chapter or every made chapter of a manga, then re-make just the music and video.
use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::Step;
use mv_score::{Level, Source, Style};
use mv_ui::theme::{icon as icon_px, radius, space};
use mv_ui::toast::{self, Toast};
use mv_ui::{btn, filter_chip, glyphs, icon_button, muted, ph, ph_icon, segmented, size, switch, ButtonKind, Tokens, Weight};

use crate::data::{ChapterItem, MangaGroup};
use crate::generate::question;
use crate::page::{chref, plural, StudioPage};
use crate::pipeline;
use crate::video::{self, SubtitleSetting};

/// What happens to the music.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MusicChoice {
    /// Keep the score (only the loudness may change).
    Keep,
    /// Pick new music (another take, maybe another genre).
    New,
    /// No background music.
    Mute,
}

/// Where the subtitles sit: the template's own place, or a band of the frame.
pub(crate) const PLACES: [(&str, Option<f32>); 4] = [("Template", None), ("Top", Some(14.)), ("Middle", Some(50.)), ("Bottom", Some(80.))];
/// Subtitle size multipliers.
pub(crate) const SIZES: [(&str, f32); 4] = [("S", 0.8), ("M", 1.0), ("L", 1.25), ("XL", 1.5)];

/// The `PLACES` index of a caption position.
pub(crate) fn place_index(pos: Option<(f32, f32)>) -> usize {
    match pos {
        None => 0,
        Some((_, y)) if y < 33. => 1,
        Some((_, y)) if y < 66. => 2,
        _ => 3,
    }
}

/// The `SIZES` index nearest to a size multiplier.
pub(crate) fn size_index(s: f32) -> usize {
    SIZES.iter().enumerate().min_by(|a, b| (a.1 .1 - s).abs().total_cmp(&(b.1 .1 - s).abs())).map(|(i, _)| i).unwrap_or(1)
}

/// Moves the subtitles to the `PLACES` band `place`, keeping a position fine-tuned in the caption
/// studio when it is already in that band.
pub(crate) fn set_place(s: &mut SubtitleSetting, place: usize) {
    if place_index(s.pos) != place {
        s.pos = PLACES.get(place).and_then(|p| p.1).map(|y| (50., y));
    }
}

/// Sets the size, keeping a fine-tuned size that is nearest to the same `SIZES` step.
pub(crate) fn set_size(s: &mut SubtitleSetting, size: f32) {
    if size_index(s.size) != size_index(size) {
        s.size = size.clamp(0.4, 2.5);
    }
}

/// The open sheet.
pub(crate) struct Restyle {
    pub chapters: Vec<ChapterItem>,
    /// "Ch. 4", or "all 12 made chapters".
    pub scope: String,
    pub music: MusicChoice,
    pub genre: Option<Style>,
    pub source: Source,
    pub level: Level,
    pub subs: bool,
    pub style: String,
    pub place: usize,
    pub size: f32,
    /// Also make these the defaults for chapters generated later.
    pub remember: bool,
}

impl Restyle {
    /// Prefilled from the first chapter's current music and subtitles.
    fn new(chapters: Vec<ChapterItem>, scope: String, remember: bool) -> Restyle {
        let first = chapters.first().cloned();
        let m = first.as_ref().map(|c| mv_score::load_settings(&chref(c))).unwrap_or_default();
        let s = first.as_ref().map(video::subtitle_setting_for).unwrap_or_default();
        Restyle {
            chapters,
            scope,
            music: if m.enabled { MusicChoice::Keep } else { MusicChoice::Mute },
            genre: m.genre,
            source: m.source,
            level: m.level,
            subs: s.enabled,
            style: s.style.clone(),
            place: place_index(s.pos),
            size: s.size,
            remember,
        }
    }

    /// The chapter's subtitle setting with the sheet's choices applied.
    fn subtitles(&self, mut s: SubtitleSetting) -> SubtitleSetting {
        s.enabled = self.subs;
        s.style = self.style.clone();
        set_place(&mut s, self.place);
        set_size(&mut s, self.size);
        s
    }

    /// The chapter's music settings with the sheet's choices applied.
    fn music(&self, mut m: mv_score::MusicSettings) -> mv_score::MusicSettings {
        match self.music {
            MusicChoice::Keep => {}
            MusicChoice::New => {
                m.enabled = true;
                m.take = m.take.wrapping_add(1);
                m.genre = self.genre;
                m.source = self.source;
                // A new genre for the whole chapter replaces per-scene picks.
                if self.genre.is_some() {
                    m.styles.clear();
                }
            }
            MusicChoice::Mute => m.enabled = false,
        }
        if self.music != MusicChoice::Mute {
            m.level = self.level;
        }
        m
    }
}

impl StudioPage {
    /// Opens the sheet for `chapters` ("Ch. 4", "all 12 made chapters").
    pub(crate) fn open_restyle(&mut self, chapters: Vec<ChapterItem>, scope: String, remember: bool, cx: &mut Context<Self>) {
        if chapters.is_empty() {
            toast::show(cx, Toast::info("No chapter has a video yet"));
            return;
        }
        self.restyle = Some(Restyle::new(chapters, scope, remember));
        cx.notify();
    }

    /// Saves the choices on every chapter and re-makes the music and video of the made ones.
    fn apply_restyle(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.restyle.take() else { return };
        for c in &r.chapters {
            let cr = chref(c);
            let before = mv_score::load_settings(&cr);
            let after = r.music(before.clone());
            if after != before {
                if let Err(e) = mv_score::save_settings(&cr, &after) {
                    self.fail(format!("Could not save the music for {}: {e}", c.label()));
                }
            }
            let own = r.subtitles(video::subtitle_setting_for(c));
            video::save_chapter_setting(&c.output_dir, Some(&own));
        }
        if r.remember {
            video::save_default(&r.subtitles(video::default_setting()));
            let mut a = crate::generate::Answers::load(&self.prefs);
            a.captions = r.subs;
            a.subtitle_style = Some(r.style.clone());
            a.music = r.music != MusicChoice::Mute;
            a.save();
        }
        // Only chapters with a video are re-made now (the rest use the choices when generated).
        // Music is made again when its settings changed; the video always is.
        let made: Vec<ChapterItem> = r.chapters.iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).cloned().collect();
        let (reels, land): (Vec<ChapterItem>, Vec<ChapterItem>) = made.into_iter().partition(|c| self.statuses.get(&c.id).is_some_and(|s| !s.reels.is_empty()));
        self.sync_pipeline(cx);
        let mut n = 0;
        for (list, format) in [(land, mv_recap::FORMAT_LANDSCAPE), (reels, mv_recap::FORMAT_REELS)] {
            if !list.is_empty() {
                let (a, w, k) = pipeline::enqueue(&list, Step::Video, Some(Step::Video), format, Some(r.subs), true);
                n += a + w + k;
            }
        }
        let msg = if n == 0 { "Saved · used the next time these chapters are generated".to_string() } else { format!("Re-making {n} video{} with the new music and subtitles", plural(n)) };
        toast::show(cx, Toast::success(msg).icon(glyphs::MUSIC));
        if n > 0 {
            self.side_open = true;
            self.side = crate::page::Side::Activity;
        }
        cx.notify();
    }

    fn restyle_edit(&mut self, f: impl FnOnce(&mut Restyle), cx: &mut Context<Self>) {
        if let Some(r) = self.restyle.as_mut() {
            f(r);
        }
        cx.notify();
    }

    /// The sheet over the Studio page.
    pub(crate) fn restyle_sheet(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.restyle.as_ref()?;
        let n = r.chapters.len();
        let made = r.chapters.iter().filter(|c| self.statuses.get(&c.id).is_some_and(|s| s.video.is_some())).count();

        // Music.
        let choices = [(MusicChoice::Keep, "Keep", ph::CHECK), (MusicChoice::New, "New music", ph::SHUFFLE), (MusicChoice::Mute, "Mute", ph::SPEAKER_SLASH)];
        let pick = choices.iter().position(|c| c.0 == r.music).unwrap_or(0);
        let music_seg = segmented("rs-music", choices.iter().map(|&(_, l, i)| (Some(i), l)), pick).on_change(cx.listener(move |this, ix: &usize, _, cx| {
            let m = choices[*ix].0;
            this.restyle_edit(|r| r.music = m, cx);
        }));
        let genres = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .child(filter_chip("rs-genre-auto", "Auto (follows the story)", r.genre.is_none()).icon(ph::SPARKLE).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.restyle_edit(|r| r.genre = None, cx))))
            .children(Style::ALL.iter().enumerate().map(|(i, &st)| {
                filter_chip(("rs-genre", i), st.label(), r.genre == Some(st)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.restyle_edit(|r| r.genre = Some(st), cx)))
            }));
        let sources = [(Source::Library, "Studio library"), (Source::Compose, "Synth score")];
        let source_seg = segmented("rs-source", sources.iter().map(|&(_, l)| (None, l)), sources.iter().position(|s| s.0 == r.source).unwrap_or(0)).small().on_change(cx.listener(move |this, ix: &usize, _, cx| {
            let s = sources[*ix].0;
            this.restyle_edit(|r| r.source = s, cx);
        }));
        let level_seg = segmented("rs-level", Level::ALL.iter().map(|l| (None, l.label())), Level::ALL.iter().position(|l| *l == r.level).unwrap_or(1)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let l = Level::ALL[*ix];
            this.restyle_edit(|r| r.level = l, cx);
        }));
        let music_note = match r.music {
            MusicChoice::Keep => "The score stays; only its loudness follows the setting below.",
            MusicChoice::New => "Picks different tracks for every chapter (pick a genre, or let each scene choose).",
            MusicChoice::Mute => "Narration only, no background music.",
        };
        let music = div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(div().flex().child(music_seg))
            .child(muted(music_note))
            .when(r.music == MusicChoice::New, |d| d.child(genres).child(div().flex().items_center().gap(px(space::SM)).child(muted("Sound")).child(source_seg)))
            .when(r.music != MusicChoice::Mute, |d| d.child(div().flex().items_center().gap(px(space::SM)).child(muted("Loudness under the voice")).child(level_seg)));

        // Subtitles.
        let favs = video::favorites();
        let all = video::styles();
        let mut ids: Vec<String> = vec![];
        for id in favs.iter().cloned().chain([r.style.clone(), video::catalog().default.clone()]).chain(all.iter().map(|s| s.id.clone())) {
            if !ids.contains(&id) && all.iter().any(|s| s.id == id) {
                ids.push(id);
            }
            if ids.len() >= 14 {
                break;
            }
        }
        let chips = div().flex().flex_wrap().gap(px(6.)).children(ids.iter().enumerate().map(|(i, id)| {
            let st = video::find_style(id);
            let id = id.clone();
            filter_chip(("rs-sub", i), st.label.clone(), id == r.style).when(favs.contains(&id), |c| c.icon(ph::STAR)).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.restyle_edit(|r| r.style = id, cx);
            }))
        }));
        let place_seg = segmented("rs-place", PLACES.iter().map(|p| (None, p.0)), r.place).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let i = *ix;
            this.restyle_edit(|r| r.place = i, cx);
        }));
        let size_seg = segmented("rs-size", SIZES.iter().map(|s| (None, s.0)), size_index(r.size)).small().on_change(cx.listener(|this, ix: &usize, _, cx| {
            let s = SIZES[*ix].1;
            this.restyle_edit(|r| r.size = s, cx);
        }));
        let subs = div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::SM))
                    .child(switch("rs-subs", r.subs).on_click(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.restyle_edit(|r| r.subs = on, cx);
                    })))
                    .child(muted(if r.subs { video::find_style(&r.style).summary() } else { "No subtitles".to_string() })),
            )
            .when(r.subs, |d| {
                d.child(chips)
                    .child(div().flex().flex_wrap().items_center().gap(px(space::SM)).child(muted("Place")).child(place_seg).child(muted("Size")).child(size_seg))
            });

        let remember = div()
            .flex()
            .items_center()
            .gap(px(space::SM))
            .child(switch("rs-remember", r.remember).on_click(cx.listener(|this, on: &bool, _, cx| {
                let on = *on;
                this.restyle_edit(|r| r.remember = on, cx);
            })))
            .child(muted("Also use these for chapters generated later"));

        let card = div()
            .id("rs-sheet")
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(720.))
            .max_h(relative(0.92))
            .rounded(px(radius::XL))
            .bg(Tokens::surface())
            .border_1()
            .border_color(Tokens::border_strong())
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::MD))
                    .p(px(space::LG))
                    .border_b_1()
                    .border_color(Tokens::border())
                    .child(div().flex().flex_none().items_center().justify_center().size(px(40.)).rounded(px(radius::LG)).bg(Tokens::accent_soft()).child(ph_icon(glyphs::MUSIC, Weight::Duotone, Tokens::accent(), icon_px::XL)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_size(px(size::LG)).font_weight(FontWeight::SEMIBOLD).text_color(Tokens::text_1()).child(format!("Music & subtitles · {}", r.scope)))
                            .child(muted(if made == n {
                                format!("{n} video{} will be made again with these choices (the narration is kept).", plural(n))
                            } else {
                                format!("{made} of {n} chapter{} have a video and will be made again; the rest use these choices when generated.", plural(n))
                            })),
                    )
                    .child(icon_button("rs-close", ph::X, "Close").kbd("escape").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.restyle = None;
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .id("rs-body")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap(px(space::LG))
                    .p(px(space::LG))
                    .child(question(glyphs::MUSIC, "Background music", "Change it, make new music, or turn it off.", music.into_any_element()))
                    .child(question(ph::SUBTITLES, "Subtitles", "Template, where they sit and how big.", subs.into_any_element()))
                    .child(remember),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap(px(space::SM))
                    .px(px(space::LG))
                    .py(px(space::MD))
                    .border_t_1()
                    .border_color(Tokens::border())
                    .bg(Tokens::surface_2())
                    .child(btn("rs-cancel", "Not now", ButtonKind::Ghost).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.restyle = None;
                        cx.notify();
                    })))
                    .child(
                        btn("rs-go", if made > 0 { format!("Apply & re-make {made} video{}", plural(made)) } else { "Save".to_string() }, ButtonKind::Primary)
                            .icon(ph::ARROWS_CLOCKWISE)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.apply_restyle(cx))),
                    ),
            );
        Some(
            div()
                .id("rs-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::XL))
                .bg(Tokens::canvas().alpha(0.78))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.restyle = None;
                        cx.notify();
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }

    /// The made chapter videos of `g`, in chapter order.
    pub(crate) fn made_videos(&self, g: &MangaGroup) -> Vec<(ChapterItem, PathBuf)> {
        g.chapters.iter().filter_map(|c| self.statuses.get(&c.id).and_then(|s| s.video.clone()).filter(|p| p.is_file()).map(|p| (c.clone(), p))).collect()
    }
}

/// A title usable as a file name.
pub(crate) fn safe_file_name(s: &str) -> String {
    let t: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        "Recap".into()
    } else {
        t.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{place_index, safe_file_name, size_index, video, Level, MusicChoice, Restyle, Source, Style, SubtitleSetting, PLACES};

    #[test]
    fn places_and_sizes_round_trip() {
        for (i, p) in PLACES.iter().enumerate() {
            assert_eq!(place_index(p.1.map(|y| (50., y))), i);
        }
        assert_eq!(size_index(1.0), 1);
        assert_eq!(size_index(1.4), 3);
        assert_eq!(size_index(0.5), 0);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(safe_file_name("Solo: Leveling?/2"), "Solo Leveling 2");
        assert_eq!(safe_file_name("  "), "Recap");
    }

    #[test]
    fn new_music_bumps_the_take_and_mute_turns_it_off() {
        let base = mv_score::MusicSettings::default();
        let mut r = Restyle {
            chapters: vec![],
            scope: String::new(),
            music: MusicChoice::New,
            genre: Some(Style::Sad),
            source: Source::Library,
            level: Level::Quiet,
            subs: true,
            style: video::catalog().default.clone(),
            place: 3,
            size: 1.25,
            remember: false,
        };
        let m = r.music(base.clone());
        assert_eq!((m.take, m.genre, m.level, m.enabled), (base.take + 1, Some(Style::Sad), Level::Quiet, true));
        r.music = MusicChoice::Mute;
        assert!(!r.music(base.clone()).enabled);
        let s = r.subtitles(SubtitleSetting::default());
        assert_eq!((s.pos, s.size), (Some((50., 80.)), 1.25));
    }
}
