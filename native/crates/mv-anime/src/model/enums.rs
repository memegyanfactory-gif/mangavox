//! Every plain-word enum of the model (design 5.3). Stored as snake_case words; a word this
//! build does not know (written by a newer app) reads as `Unknown` instead of failing the file.

use serde::{Deserialize, Serialize};

/// Declares a plain-word enum with the model's derives, snake_case serde names, an `Unknown`
/// fallback and the given default variant.
macro_rules! words {
    ($(#[$m:meta])* $name:ident (default $def:ident) { $($(#[$vm:meta])* $v:ident),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vm])* $v,)+
            /// A word written by a newer MangaVox.
            #[serde(other)]
            Unknown,
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$def
            }
        }
    };
}

words! {
    /// Shot size, widest to closest, plus the special framings.
    ShotSize (default Ms) { Els, Ls, Fs, Cowboy, Ms, Mcu, Cu, Ecu, Insert, Ots, TwoShot, Pov, Overhead }
}

words! {
    /// Camera angle.
    Angle (default Eye) { Eye, Low, High, Top, Worm, Dutch }
}

words! {
    /// Camera move of one shot (the animatic animates it).
    Move (default Hold) {
        Hold, PushIn, PullOut, PanLeft, PanRight, TiltUp, TiltDown, TruckLeft, TruckRight, Shake, Whip, CrashZoom, RackFocus, BgSlide,
    }
}

words! {
    /// Speed of a camera move.
    Speed (default Medium) { Creep, Slow, Medium, Fast, Snap }
}

words! {
    /// What kind of scene it is; drives the pacing table.
    SceneKind (default Dialogue) { Dialogue, Action, Dread, Narration, Flashback, Montage }
}

words! {
    /// The scene's tone.
    Tone (default Drama) { Drama, Romance, Action, Fight, Horror, Comedy, Calm, Mystery }
}

words! {
    /// A line's or a mark's emotion (strength lives in `Emotion`).
    EmotionKind (default Neutral) {
        Neutral, Joy, Sadness, Anger, Fear, Surprise, Disgust, Resolve, Pride, Grief, Despair, Relief, Awe, Tender, Menace,
    }
}

words! {
    /// What a line is; SFX lines are kept but never voiced.
    LineKind (default Speech) { Speech, Thought, Narration, Sfx }
}

words! {
    /// Delivery pace of a line (voice speed slow 0.9, normal 1.0, fast 1.1).
    Pace (default Normal) { Slow, Normal, Fast }
}

words! {
    /// How the speaker's mouth is handled in a shot.
    MouthPolicy (default Closed) { Offscreen, Back, Hidden, Closed, Flap, Generate }
}

words! {
    /// How much of a shot moves.
    MotionKind (default Hold) { Hold, MouthOnly, CameraOnly, Partial, Full }
}

words! {
    /// Where a shot's motion is made (Beta 2; always `Auto` now).
    MotionRoute (default Auto) { Auto, Compositor, Kling25Web, LocalH3, Paid }
}

words! {
    /// What stands behind the characters.
    BackgroundKind (default Matched) { Matched, Backdrop, None }
}

words! {
    /// How much of a character the camera sees.
    Visibility (default Full) { Full, Partial, Back, Ots, Offscreen }
}

words! {
    /// Body pose of a mark.
    Pose (default Stand) { Stand, Sit, Kneel, Lie, Run, Fight }
}

words! {
    /// Which hand holds a prop.
    Hand (default Right) { Left, Right, Both }
}

words! {
    /// A character's role in the story.
    Role (default Neutral) { Hero, Ally, Threat, Neutral }
}

words! {
    /// Coverage pattern a run of shots follows.
    Pattern (default Conversation) { Arrival, Conversation, ThreatReveal, Stunned, Reveal, SuddenDeath, Chase }
}

words! {
    /// How a shot ends into the next.
    Transition (default Cut) { Cut, Dissolve, White, Black }
}

words! {
    /// Time of day of a scene (picks the place state).
    TimeOfDay (default Day) { Day, Dusk, Night, Dawn, Indoor }
}

words! {
    /// Interior or exterior place.
    PlaceKind (default Interior) { Interior, Exterior }
}

words! {
    /// What a landmark on a floor plan is.
    LandmarkKind (default Feature) { Door, Window, Wall, Furniture, Prop, Feature, Light }
}

words! {
    /// Which side of a character a reference crop shows.
    ViewAngle (default Front) { Front, ThreeQuarter, Side, Back }
}

words! {
    /// How much of a character a reference crop holds.
    CropFraming (default Face) { Face, Bust, Full }
}

words! {
    /// Which upload slot of the Magnific generator a reference goes into.
    SiteSlot (default Add) { Add, Style, Character }
}

words! {
    /// Depth of field.
    Depth (default Normal) { Deep, Normal, Shallow }
}

words! {
    /// Exposure key.
    Key (default Normal) { Dark, Normal, Bright }
}

words! {
    /// Film grain.
    Grain (default Clean) { Clean, Light, Heavy }
}

words! {
    /// Lighting setup of a shot (`Auto` = from the place state).
    LightSetup (default Auto) { Auto, Backlit, Window, UnderLight, Silhouette, Top, Rim }
}

words! {
    /// Review status of a card (frame, sheet, plate, voice).
    Status (default Draft) { Draft, NeedsReview, Approved, Rejected }
}

words! {
    /// How serious a check result is.
    Severity (default Info) { Error, Warning, Info }
}

words! {
    /// What a take's file is.
    TakeKind (default Image) { Image, Audio, Video }
}

words! {
    /// Where a shot comes from: a manga panel or added for the anime.
    Origin (default Manga) { Manga, Anime }
}
