//! The eight steps of an open project (design 9.4–9.11). Each step is its own view (entity)
//! made by its module's `open`; a step whose feature is not compiled in shows a "not part of
//! this build" screen instead (design 2.2, 10.2). The page owns which step is current.
use std::rc::Rc;

use gpui_kit::*;
use mv_ui::theme::space;
use mv_ui::{empty_view, ph, Ph};

mod animate;
mod shared;
#[cfg(feature = "bible")]
mod bible;
#[cfg(feature = "cut")]
mod cut;
#[cfg(feature = "bible")]
mod floorplan;
#[cfg(feature = "frames")]
mod frames;
#[cfg(feature = "shots")]
mod inspector;
#[cfg(feature = "story")]
mod manga;
#[cfg(feature = "cut")]
mod preview;
#[cfg(feature = "shots")]
mod shots;
#[cfg(feature = "sound")]
mod sound;
#[cfg(feature = "story")]
mod story;

/// One step of the step bar, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Step {
    #[default]
    Manga,
    Story,
    Bible,
    Shots,
    Frames,
    /// "Coming next": explained, never done.
    Animate,
    Sound,
    Cut,
}

impl Step {
    pub const ALL: [Step; 8] = [Step::Manga, Step::Story, Step::Bible, Step::Shots, Step::Frames, Step::Animate, Step::Sound, Step::Cut];

    pub fn label(self) -> &'static str {
        match self {
            Step::Manga => "Manga",
            Step::Story => "Story",
            Step::Bible => "Bible",
            Step::Shots => "Shots",
            Step::Frames => "Frames",
            Step::Animate => "Animate",
            Step::Sound => "Sound",
            Step::Cut => "Cut",
        }
    }

    pub fn icon(self) -> Ph {
        match self {
            Step::Manga => ph::BOOK_OPEN_TEXT,
            Step::Story => ph::NOTE_PENCIL,
            Step::Bible => ph::USERS,
            Step::Shots => ph::VIDEO_CAMERA,
            Step::Frames => ph::IMAGE,
            Step::Animate => ph::PLAY_CIRCLE,
            Step::Sound => ph::WAVEFORM,
            Step::Cut => ph::SCISSORS,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Step> {
        Step::ALL.get(i).copied()
    }

    /// The word saved for this step in the project's `work/ui.json`.
    pub fn key(self) -> &'static str {
        match self {
            Step::Manga => "manga",
            Step::Story => "story",
            Step::Bible => "bible",
            Step::Shots => "shots",
            Step::Frames => "frames",
            Step::Animate => "animate",
            Step::Sound => "sound",
            Step::Cut => "cut",
        }
    }

    pub fn from_key(key: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.key() == key)
    }

    /// The step's screen is compiled into this build.
    pub fn built(self) -> bool {
        match self {
            Step::Manga | Step::Story => cfg!(feature = "story"),
            Step::Bible => cfg!(feature = "bible"),
            Step::Shots => cfg!(feature = "shots"),
            Step::Frames => cfg!(feature = "frames"),
            Step::Animate => true,
            Step::Sound => cfg!(feature = "sound"),
            Step::Cut => cfg!(feature = "cut"),
        }
    }
}

/// Switches the page to another step.
pub type Go = Rc<dyn Fn(Step, &mut Window, &mut App)>;

/// What every step view gets from the page.
#[allow(dead_code)] // read by the step screens (W3); a build without their features reads only `go`
#[derive(Clone)]
pub struct StepCtx {
    /// Series id (`mv_anime::Store::open`).
    pub project: String,
    /// Episode id ("ep01").
    pub episode: String,
    /// Switches the page to another step.
    pub go: Go,
}

/// The view of `step`, or the "not part of this build" screen when its feature is off.
pub fn open(step: Step, ctx: StepCtx, window: &mut Window, cx: &mut App) -> AnyView {
    match step {
        #[cfg(feature = "story")]
        Step::Manga => manga::open(ctx, window, cx),
        #[cfg(feature = "story")]
        Step::Story => story::open(ctx, window, cx),
        #[cfg(feature = "bible")]
        Step::Bible => bible::open(ctx, window, cx),
        #[cfg(feature = "shots")]
        Step::Shots => shots::open(ctx, window, cx),
        #[cfg(feature = "frames")]
        Step::Frames => frames::open(ctx, window, cx),
        Step::Animate => animate::open(ctx, window, cx),
        #[cfg(feature = "sound")]
        Step::Sound => sound::open(ctx, window, cx),
        #[cfg(feature = "cut")]
        Step::Cut => cut::open(ctx, window, cx),
        // Reached only when a step's feature is off.
        #[allow(unreachable_patterns)]
        _ => cx.new(|_| NotBuilt { step }).into(),
    }
}

/// A step body centred in the page area.
fn centred(step: Step, body: &'static str) -> Div {
    div().size_full().flex().items_center().justify_center().p(px(space::XXL)).child(empty_view(step.icon(), step.label(), body))
}

/// A step whose feature is not compiled into this build.
struct NotBuilt {
    step: Step,
}

impl Render for NotBuilt {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        centred(self.step, "This step is not part of this build.")
    }
}
