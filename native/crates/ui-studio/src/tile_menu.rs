//! The right-click menu of a dashboard chapter tile: open the chapter, make what is missing, redo
//! from a step, watch the video, open the output folder, pick it. On a tile that is one of several
//! picks, the Generate and Pick actions apply to every picked chapter of that manga.
use std::rc::Rc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mv_recap::{PhaseStatus, Step};
use mv_ui::theme::{icon as icon_px, size};
use mv_ui::{glyphs, ph, ph_icon, Ph, Tokens, Weight};

use crate::data::ChapterItem;
use crate::page::{Load, Phase, StudioPage};

/// The "Redo from…" group: each step and what the menu calls it.
pub(crate) const REDO_ITEMS: [(Step, &str); 5] = [
    (Step::Read, "Redo from Read (start over)"),
    (Step::Story, "Redo from Story"),
    (Step::Audio, "Redo from Audio"),
    (Step::Music, "New music take"),
    (Step::Video, "Redo Video"),
];

/// Why "Redo from `step`" cannot run for a chapter in state `st`: nothing it is built from is made
/// yet (Make missing steps covers that). `None`: it can run.
pub(crate) fn redo_blocked(step: Step, st: &PhaseStatus) -> Option<&'static str> {
    match step {
        Step::Read => (st.read_pages == 0 && !Phase::ALL.iter().any(|p| p.done(st))).then_some("Nothing is made yet"),
        Step::Transcript | Step::Story => (st.read_pages == 0).then_some("Nothing has been read yet"),
        Step::Audio => (!st.story).then_some("There is no story yet"),
        Step::Music | Step::Video => (st.audio_files == 0).then_some("There is no narration yet"),
    }
}

/// Why "Make missing steps" has nothing to do for a chapter: it is already queued, or its video is
/// made (making a step again is a redo). `None`: it can run.
pub(crate) fn missing_blocked(st: &PhaseStatus, queued: bool) -> Option<&'static str> {
    if queued {
        Some("Already in the queue")
    } else if st.video.is_some() {
        Some("Every step is made; redo one instead")
    } else {
        None
    }
}

/// Whether an action over `total` chapters runs, `ok` of which can take it: `Err` holds the reason
/// it is disabled (`why`, from the first chapter that cannot), `Ok` an optional hint naming how
/// many it applies to when not all of them.
pub(crate) fn verdict(ok: usize, total: usize, why: Option<&str>) -> Result<Option<String>, String> {
    match ok {
        0 => Err(why.unwrap_or("Nothing to do").to_string()),
        k if k < total => Ok(Some(format!("{k} of {total} chapters"))),
        _ => Ok(None),
    }
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

enum Entry {
    Heading(SharedString),
    Separator,
    Action { label: SharedString, icon: Ph, hint: Option<SharedString>, disabled: bool, run: Handler },
}

fn action(label: impl Into<SharedString>, icon: Ph, verdict: Result<Option<String>, String>, run: impl Fn(&mut Window, &mut App) + 'static) -> Entry {
    let (hint, disabled) = match verdict {
        Ok(h) => (h, false),
        Err(why) => (Some(why), true),
    };
    Entry::Action { label: label.into(), icon, hint: hint.map(SharedString::from), disabled, run: Rc::new(run) }
}

fn row(label: &SharedString, icon: Ph, hint: &Option<SharedString>, disabled: bool) -> Div {
    let fg = if disabled { Tokens::text_disabled() } else { Tokens::text_1() };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .min_h(px(30.))
        .text_size(px(size::MD))
        .text_color(fg)
        .child(ph_icon(icon, Weight::Regular, if disabled { fg } else { Tokens::text_2() }, icon_px::MD))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(label.clone()))
                .when_some(hint.clone(), |d, h| d.child(div().truncate().text_size(px(size::XS)).text_color(Tokens::text_3()).child(h))),
        )
}

fn fill(menu: PopupMenu, entries: Vec<Entry>) -> PopupMenu {
    entries.into_iter().fold(menu, |m, e| match e {
        Entry::Heading(l) => m.label(l),
        Entry::Separator => m.separator(),
        Entry::Action { label, icon, hint, disabled, run } => m.item(
            PopupMenuItem::element(move |_, _| row(&label, icon, &hint, disabled))
                .disabled(disabled)
                .when(!disabled, |p| p.on_click(move |_, w, cx| run(w, cx))),
        ),
    })
}

impl StudioPage {
    /// The right-click menu of chapter `c`'s tile, built when it opens (so it shows the state then).
    pub(crate) fn tile_menu(&self, c: &ChapterItem, cx: &mut Context<Self>) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let item = c.clone();
        move |menu, _, cx| {
            let Some(page) = weak.upgrade() else { return menu };
            let entries = page.read(cx).tile_menu_entries(&item, &weak);
            fill(menu.min_w(px(260.)), entries)
        }
    }

    /// What the menu acts on: every picked chapter of the manga when `c` is one of the picks,
    /// else `c` alone.
    fn tile_menu_targets(&self, c: &ChapterItem) -> Vec<ChapterItem> {
        if self.dash_pick.contains(&c.id) {
            if let Load::Ready(groups) = &self.library {
                if let Some(g) = groups.iter().find(|g| g.chapters.iter().any(|x| x.id == c.id)) {
                    let picked: Vec<ChapterItem> = g.chapters.iter().filter(|x| self.dash_pick.contains(&x.id)).cloned().collect();
                    if !picked.is_empty() {
                        return picked;
                    }
                }
            }
        }
        vec![c.clone()]
    }

    fn tile_menu_entries(&self, c: &ChapterItem, weak: &WeakEntity<Self>) -> Vec<Entry> {
        let targets = self.tile_menu_targets(c);
        let n = targets.len();
        let many = n > 1;
        let status = |x: &ChapterItem| self.statuses.get(&x.id).cloned().unwrap_or_default();
        let queued = |x: &ChapterItem| self.queue.for_chapter(&x.id).is_some_and(|i| i.state.active());
        // On a multi-pick, the actions on one chapter name it.
        let own_hint = many.then(|| c.label());
        let mut out = vec![Entry::Heading(if many { format!("{n} chapters").into() } else { c.label().into() })];

        let id = c.id.clone();
        let w = weak.clone();
        out.push(action("Open", glyphs::EDIT, Ok(own_hint.clone()), move |_, cx| {
            let _ = w.update(cx, |this, cx| this.go_to(&id, None, cx));
        }));

        // Make missing steps: the chapters with something missing that are not queued yet.
        let fresh: Vec<ChapterItem> = targets.iter().filter(|x| missing_blocked(&status(x), queued(x)).is_none()).cloned().collect();
        let why = targets.iter().find_map(|x| missing_blocked(&status(x), queued(x)));
        let w = weak.clone();
        out.push(action("Make missing steps", ph::ROCKET_LAUNCH, verdict(fresh.len(), n, why), move |_, cx| {
            let fresh = fresh.clone();
            let _ = w.update(cx, |this, cx| this.open_generate(fresh, None, cx));
        }));

        // Redo from…: the same sheet as the pick bar's Redo, with that step.
        out.push(Entry::Separator);
        out.push(Entry::Heading("Redo from…".into()));
        for ((step, label), (_, _, icon)) in REDO_ITEMS.into_iter().zip(crate::generate::REDO) {
            let can: Vec<ChapterItem> = targets.iter().filter(|x| redo_blocked(step, &status(x)).is_none()).cloned().collect();
            let why = targets.iter().find_map(|x| redo_blocked(step, &status(x)));
            let w = weak.clone();
            out.push(action(label, icon, verdict(can.len(), n, why), move |_, cx| {
                let can = can.clone();
                let _ = w.update(cx, |this, cx| this.open_generate(can, Some(step), cx));
            }));
        }

        out.push(Entry::Separator);
        let video = status(c).video;
        let title = format!("{} · {}", c.manga_title, c.label());
        let watch = match &video {
            Some(_) => Ok(own_hint.clone()),
            None => Err("No video yet".to_string()),
        };
        out.push(action("Watch video", ph::PLAY_CIRCLE, watch, move |w, cx| {
            if let Some(v) = &video {
                crate::phases::play_video(&title, v, w, cx);
            }
        }));
        let dir = c.output_dir.clone();
        out.push(action("Open output folder", ph::FOLDER_OPEN, Ok(own_hint), move |_, cx| {
            let _ = std::fs::create_dir_all(&dir);
            cx.open_with_system(&dir);
        }));

        // Pick / Unpick, like a click on the tile (a multi-pick unpicks all of them).
        let picked = self.dash_pick.contains(&c.id);
        let label = match (picked, many) {
            (true, true) => format!("Unpick {n} chapters"),
            (true, false) => "Unpick".to_string(),
            _ => "Pick".to_string(),
        };
        let (w, ids, item) = (weak.clone(), targets.iter().map(|x| x.id.clone()).collect::<Vec<_>>(), c.clone());
        out.push(action(label, if picked { ph::X } else { ph::CHECK_SQUARE }, Ok(None), move |_, cx| {
            let _ = w.update(cx, |this, cx| {
                if picked {
                    for id in &ids {
                        this.dash_pick.remove(id);
                    }
                } else {
                    this.dash_anchor = Some(item.id.clone());
                    if this.dash_pick.insert(item.id.clone()) {
                        this.append_if_generating(&item, cx);
                    }
                }
                cx.notify();
            });
        }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{missing_blocked, redo_blocked, verdict, REDO_ITEMS};
    use mv_recap::{PhaseStatus, Step};

    #[test]
    fn nothing_made_offers_only_make_missing() {
        let st = PhaseStatus { total_pages: 5, ..Default::default() };
        assert!(missing_blocked(&st, false).is_none());
        for (s, _) in REDO_ITEMS {
            assert!(redo_blocked(s, &st).is_some(), "{s:?} has nothing to redo");
        }
    }

    #[test]
    fn a_redo_needs_what_its_step_is_built_from() {
        let read = PhaseStatus { read_pages: 5, total_pages: 5, transcript: true, ..Default::default() };
        assert!(redo_blocked(Step::Read, &read).is_none());
        assert!(redo_blocked(Step::Story, &read).is_none());
        assert_eq!(redo_blocked(Step::Audio, &read), Some("There is no story yet"));
        assert!(redo_blocked(Step::Video, &read).is_some());
        let narrated = PhaseStatus { story: true, audio_files: 5, ..read.clone() };
        for (s, _) in REDO_ITEMS {
            assert!(redo_blocked(s, &narrated).is_none(), "{s:?} can run once narrated");
        }
        // A half-read chapter can start over or rewrite its story.
        let half = PhaseStatus { read_pages: 2, total_pages: 5, ..Default::default() };
        assert!(redo_blocked(Step::Read, &half).is_none() && redo_blocked(Step::Story, &half).is_none());
    }

    #[test]
    fn make_missing_skips_made_and_queued_chapters() {
        let done = PhaseStatus { read_pages: 3, total_pages: 3, transcript: true, story: true, audio_files: 3, video: Some("/v.mp4".into()), ..Default::default() };
        assert!(missing_blocked(&done, false).is_some());
        assert_eq!(missing_blocked(&PhaseStatus::default(), true), Some("Already in the queue"));
    }

    #[test]
    fn a_multi_pick_runs_when_any_chapter_can() {
        assert_eq!(verdict(3, 3, None), Ok(None));
        assert_eq!(verdict(2, 3, Some("x")), Ok(Some("2 of 3 chapters".into())));
        assert_eq!(verdict(0, 3, Some("There is no story yet")), Err("There is no story yet".into()));
    }
}
