//! The director's shot recipes: named, reusable ways to film a beat, as seen cut by cut in real
//! TV anime and taught in film craft (`native/docs/design/anime.md`). A cut that follows a recipe
//! takes its size, angle, lens, camera, how much is animated and how the frame is built from it,
//! unless the director says otherwise. The cheapest motion that sells the shot is the default,
//! as in real TV anime: most talk is a held drawing, a slow camera or mouth flaps.

pub struct Recipe {
    pub id: &'static str,
    pub name: &'static str,
    /// When the director reaches for it.
    pub when: &'static str,
    pub size: &'static str,
    pub angle: &'static str,
    pub lens: &'static str,
    /// How the frame is built: an illustrator draws from this line.
    pub composition: &'static str,
    pub camera: &'static str,
    pub motion: &'static str,
    /// How long it usually runs (seconds).
    pub seconds: (f32, f32),
    /// The scene kinds it suits ("any" for all).
    pub kinds: &'static [&'static str],
}

const ANY: &[&str] = &["any"];
const TALK: &[&str] = &["dialogue", "dread", "narration", "flashback"];

pub static RECIPES: &[Recipe] = &[
    // ---- establishing a place
    Recipe { id: "aerial-establish", name: "Aerial of the city or land", when: "the first image of a new place or time: where in the world we are", size: "establishing", angle: "bird's eye", lens: "wide", composition: "the whole district from far above, strong perspective lines leading into the distance, sky or haze, no characters", camera: "pan_right", motion: "camera", seconds: (3., 5.), kinds: ANY },
    Recipe { id: "street-establish", name: "Street-level approach", when: "the second step of an establishing sequence: the way to the place", size: "extreme long", angle: "eye level", lens: "wide", composition: "the street or grounds leading to the place, a few tiny passers-by, the destination in the distance", camera: "pan_right", motion: "camera", seconds: (2., 4.), kinds: ANY },
    Recipe { id: "site-high-dutch", name: "The site from a steep tilted high angle", when: "the exact spot the scene plays in, made dynamic though nothing moves", size: "establishing", angle: "high dutch", lens: "wide", composition: "looking steeply down at the site, the frame tilted so fences and walls run as diagonals, the key object (a gate, a door) glowing in a lower third, tiny figures for scale", camera: "tilt_down", motion: "camera", seconds: (4., 7.), kinds: ANY },
    Recipe { id: "building-low", name: "The building from below", when: "cutting to a new interior: its outside first", size: "establishing", angle: "low", lens: "wide", composition: "the building's front from the pavement against the sky, verticals converging upward", camera: "tilt_up", motion: "camera", seconds: (3., 4.), kinds: ANY },
    Recipe { id: "room-high-corner", name: "The room from a high corner", when: "the first cut inside a room, while the talk begins", size: "long", angle: "high", lens: "wide", composition: "the whole room from an upper corner, furniture giving depth, the people small, their faces unreadable", camera: "pan_left", motion: "camera", seconds: (6., 12.), kinds: TALK },
    // ---- conversation
    Recipe { id: "master-two-shot", name: "Wide two-shot master", when: "the start of a conversation: who is where; several lines play on it", size: "long", angle: "eye level", lens: "normal", composition: "both speakers in full figure, off-centre, each on their own side, an out-of-focus object large at one frame edge, the place readable behind", camera: "pan_right", motion: "camera", seconds: (6., 14.), kinds: TALK },
    Recipe { id: "dirty-ots", name: "Dirty over-the-shoulder", when: "one person speaks to another: the speaker faces us past the listener", size: "over-the-shoulder", angle: "eye level", lens: "tele", composition: "the speaker's face in the far two thirds with eyes on the upper third line, the listener's head and shoulder large, dark and out of focus on the near side", camera: "fix", motion: "lip", seconds: (3., 9.), kinds: TALK },
    Recipe { id: "reverse-ots", name: "The answering over-the-shoulder", when: "the reply: the mirror of the shot before, each person staying on their own side of the frame", size: "over-the-shoulder", angle: "eye level", lens: "tele", composition: "mirror of the previous over-the-shoulder: the other face in the far two thirds, the first speaker's shoulder soft in front, eyelines meeting", camera: "fix", motion: "lip", seconds: (3., 9.), kinds: TALK },
    Recipe { id: "clean-single", name: "Clean single", when: "a line that matters, or a reaction that must be read", size: "medium close-up", angle: "eye level", lens: "tele", composition: "one face, alone in the frame, with looking room on the side they look to, the background soft", camera: "fix", motion: "lip", seconds: (2., 5.), kinds: TALK },
    Recipe { id: "listener-reaction", name: "The listener while the other speaks", when: "a line heard off screen over the one it is about or the one it lands on", size: "close-up", angle: "eye level", lens: "tele", composition: "the listener's face, mouth closed, the eyes carrying the reaction, shallow focus", camera: "fix", motion: "hold", seconds: (2., 6.), kinds: TALK },
    Recipe { id: "group-backs-pan", name: "Pan across the group's backs", when: "many short lines from many people (a vote, a murmur, an argument): no mouth is shown", size: "group", angle: "eye level", lens: "tele", composition: "the group from behind or three-quarter back, large cropped figures in front, one or two faces seen in the gaps between them", camera: "pan_right", motion: "camera", seconds: (6., 10.), kinds: TALK },
    Recipe { id: "tiny-figures-take", name: "Tiny figures in a big space", when: "a discussion that also shows how small the people are; a long take of many lines", size: "extreme long", angle: "eye level", lens: "wide", composition: "the group small in the lower third, the cave, hall or sky towering over them, darkness or space pressing in", camera: "pull_out", motion: "camera", seconds: (8., 16.), kinds: TALK },
    Recipe { id: "circle-from-above", name: "The circle from above", when: "a briefing, a decision the whole group makes", size: "group", angle: "high", lens: "wide", composition: "the group standing in a ring seen from above, the leader set apart, long shadows on the ground", camera: "pan_right", motion: "camera", seconds: (5., 9.), kinds: TALK },
    Recipe { id: "split-reactions", name: "Reactions in split panels", when: "everyone reacts to the same thing at once", size: "group", angle: "eye level", lens: "normal", composition: "three to five faces, each in its own angled panel with a thin white border, lit by the same light", camera: "fix", motion: "hold", seconds: (3., 6.), kinds: &["dialogue", "dread", "action"] },
    // ---- emphasis and feeling
    Recipe { id: "ecu-eyes", name: "Extreme close-up of the eyes", when: "a realisation, a decision, terror", size: "extreme close-up", angle: "eye level", lens: "tele", composition: "the eyes fill the frame, the hair's shadow across the brow, a bead of sweat or a tear, the pupils small in fear", camera: "push_in", motion: "camera", seconds: (2., 6.), kinds: ANY },
    Recipe { id: "ecu-mouth", name: "The mouth, the eyes cut off", when: "shame, rage or grief the character hides: the eyes are not shown", size: "extreme close-up", angle: "low", lens: "tele", composition: "the lower face only, the eyes cut off by the top of the frame, teeth set or lips trembling, lit from below", camera: "push_in", motion: "lip", seconds: (3., 8.), kinds: ANY },
    Recipe { id: "body-insert", name: "A hand, a fist, the feet", when: "a feeling shown without the face: a fist tightens, a hand grips a strap, feet stop", size: "insert", angle: "eye level", lens: "tele", composition: "the hand or feet large and crisp, everything else soft", camera: "push_in", motion: "camera", seconds: (1.5, 3.5), kinds: ANY },
    Recipe { id: "object-insert", name: "Held insert of an object", when: "an object that matters (a weapon, a stone, a letter), often with a line over it", size: "insert", angle: "high", lens: "tele", composition: "the object large in the hand or on the table, sharp, the background a soft blur", camera: "fix", motion: "hold", seconds: (2.5, 10.), kinds: ANY },
    Recipe { id: "low-hero-sky", name: "Low angle against the sky", when: "resolve, a vow, a moment of strength", size: "medium close-up", angle: "low", lens: "normal", composition: "from below, the character centred against open sky or a plain ceiling, nothing else in the frame", camera: "push_in", motion: "lip", seconds: (3., 7.), kinds: ANY },
    Recipe { id: "thought-still", name: "What they are thinking of, as a still", when: "a person or place named in an inner monologue or narration", size: "long", angle: "eye level", lens: "normal", composition: "a quiet still of the place or person thought of (a hospital bed by a window, a sister at her desk), nobody moving, in its own light", camera: "push_in", motion: "camera", seconds: (2., 4.5), kinds: ANY },
    Recipe { id: "warm-memory", name: "A warm memory cut into a cold scene", when: "what a character stands to lose, the moment before they lose it", size: "medium", angle: "eye level", lens: "normal", composition: "a bright, warm, high-key picture of the happy memory, soft edges, the opposite of the scene's colour", camera: "fix", motion: "hold", seconds: (2.5, 4.), kinds: &["dread", "action", "flashback"] },
    Recipe { id: "intro-inserts", name: "A character introduced in pieces", when: "the first appearance of an important character: two or three of these (the body, the hair, a hand) before the face", size: "insert", angle: "eye level", lens: "tele", composition: "one part of the character only (the torso and bag, the hair from behind, a hand on a strap), the face not yet shown", camera: "tilt_up", motion: "camera", seconds: (2., 4.), kinds: ANY },
    // ---- dread and scale
    Recipe { id: "symmetric-idol", name: "Dead-centre on the thing", when: "the statue, the throne, the door: something sacred or wrong", size: "close-up", angle: "low", lens: "normal", composition: "dead centre and perfectly symmetrical, filling the frame, a halo or arch behind, the corners falling to black", camera: "push_in", motion: "camera", seconds: (4., 12.), kinds: &["dread", "narration", "dialogue"] },
    Recipe { id: "too-big", name: "Too big for the frame", when: "the size of a monster or a building against people", size: "long", angle: "low", lens: "wide", composition: "the people small, from behind, in a lower corner; the giant's legs or body cropped by the top of the frame so it does not fit", camera: "tilt_up", motion: "camera", seconds: (4., 8.), kinds: &["dread", "action"] },
    Recipe { id: "through-the-threat", name: "Through a dark foreground shape", when: "being watched; isolation; the threat closer than they know", size: "long", angle: "high", lens: "tele", composition: "the people small and lit in the distance; half the frame a black, out-of-focus foreground silhouette of the threat", camera: "pull_out", motion: "camera", seconds: (6., 10.), kinds: &["dread", "dialogue"] },
    Recipe { id: "terror-closeup", name: "Tilted terror close-up", when: "panic, horror at what is seen", size: "close-up", angle: "low dutch", lens: "wide", composition: "the face from below and tilted, looming and distorted by the wide lens, eyes wide with tiny pupils, mouth open, the skin taking the scene's colour", camera: "shake", motion: "lip", seconds: (1.5, 4.), kinds: &["dread", "action"] },
    Recipe { id: "tall-tilt", name: "Tilt up a tall subject", when: "a tall manga panel: a tower, a monster, a statue, a figure from the feet to the face", size: "full", angle: "low", lens: "wide", composition: "starts on the feet or the base and travels up the whole body to the face, the top held at the end", camera: "tilt_up", motion: "camera", seconds: (4., 8.), kinds: ANY },
    Recipe { id: "ground-foot", name: "Ground level at the foot", when: "something huge arrives: the step lands by the lens", size: "insert", angle: "ground level", lens: "wide", composition: "a huge foot or weapon landing in the foreground, dust rolling toward the lens, small figures beyond", camera: "shake", motion: "pose", seconds: (1.5, 3.), kinds: &["dread", "action"] },
    Recipe { id: "clock-cutaway", name: "The clock", when: "something that counts down or marks time (flames going out, a door closing): the same framing each time it returns", size: "insert", angle: "bird's eye", lens: "normal", composition: "the object alone, always framed the same way, only its state changed since last time", camera: "fix", motion: "hold", seconds: (1.5, 5.), kinds: &["dread", "action", "dialogue"] },
    Recipe { id: "topdown-tableau", name: "Top-down tableau", when: "a death, an aftermath, a ritual", size: "full", angle: "bird's eye", lens: "normal", composition: "straight down, the body or the circle dead centre, the pattern of the floor or the blood radiating from it", camera: "pull_out", motion: "camera", seconds: (5., 10.), kinds: &["dread", "action", "flashback"] },
    Recipe { id: "into-the-light", name: "Silhouettes entering the light", when: "crossing a threshold: a gate, a door, the unknown", size: "long", angle: "eye level", lens: "normal", composition: "the figures as dark silhouettes from behind against the glowing opening", camera: "push_in", motion: "camera", seconds: (3., 6.), kinds: ANY },
    // ---- action
    Recipe { id: "fight-geography", name: "Where everyone is", when: "before the blows: both sides and the ground between them in one frame", size: "long", angle: "high", lens: "wide", composition: "both sides in one frame with the space between them clear, the exits and obstacles readable", camera: "fix", motion: "pose", seconds: (2., 4.), kinds: &["action"] },
    Recipe { id: "lunge-at-lens", name: "The lunge at the lens", when: "the attack itself", size: "medium", angle: "dutch", lens: "wide", composition: "the attacker coming at the camera along a diagonal, the weapon arm huge in front, streaks of speed lines instead of scenery", camera: "track_in", motion: "full", seconds: (0.8, 2.5), kinds: &["action"] },
    Recipe { id: "impact", name: "The hit lands", when: "the moment of contact", size: "insert", angle: "eye level", lens: "tele", composition: "the point of contact only, the contact drawing held a few frames, one white flash, debris and sparks flying outward", camera: "shake", motion: "pose", seconds: (0.4, 1.2), kinds: &["action", "dread"] },
    Recipe { id: "shout-speedlines", name: "The shout against speed lines", when: "the fighter's face in the middle of the charge: energy without animation", size: "close-up", angle: "dutch", lens: "wide", composition: "the shouting face tilted in the frame, hair blown back, the background nothing but speed lines", camera: "shake", motion: "lip", seconds: (0.8, 2.), kinds: &["action"] },
    Recipe { id: "darting-eyes", name: "Darting eyes", when: "frantic thinking: four to eight of these in a row, each looking another way", size: "extreme close-up", angle: "eye level", lens: "tele", composition: "the eyes only, looking sharply to one side, sweat on the skin", camera: "fix", motion: "hold", seconds: (0.4, 0.6), kinds: &["action", "dread"] },
    Recipe { id: "dust-settles", name: "The dust settles", when: "after a blast or a collapse: what is left", size: "long", angle: "eye level", lens: "normal", composition: "the space full of drifting dust and smoke, shapes and bodies emerging as it thins", camera: "pull_out", motion: "camera", seconds: (2., 5.), kinds: &["action", "dread"] },
    Recipe { id: "reveal-preparation", name: "The preparation before a reveal", when: "the held moment before a power, a weapon or a creature is shown: the audience is made to wait", size: "full", angle: "low", lens: "normal", composition: "the character's whole body in a decisive pose, a breath held, the light gathering, nothing revealed yet", camera: "push_in", motion: "pose", seconds: (1.5, 3.5), kinds: &["action", "dread", "dialogue"] },
    Recipe { id: "reveal-partial", name: "What is this?", when: "between the preparation and the full reveal: a detail too close or too abstract to read at once", size: "extreme close-up", angle: "eye level", lens: "tele", composition: "an abstract fragment of the thing being revealed (a texture, a feather, an eye, a glow) filling the frame, its whole shape withheld", camera: "pull_out", motion: "camera", seconds: (1., 2.5), kinds: &["action", "dread"] },
    Recipe { id: "pose-not-face", name: "The feeling in the body", when: "an emotion shown by posture, hands or silhouette instead of the face", size: "full", angle: "high", lens: "normal", composition: "the whole body or a silhouette, the face turned away, in shadow or cut off; the pose tells the feeling", camera: "fix", motion: "hold", seconds: (2., 5.), kinds: ANY },
    Recipe { id: "stunned-slowmo", name: "Stunned, in slow motion", when: "a character overwhelmed by what they see: time slows and the sound falls away, then a hard cut back", size: "medium close-up", angle: "eye level", lens: "tele", composition: "the face partly hidden (hair over one eye, or the eyes out of frame), the world behind blurred and slowed", camera: "push_in", motion: "pose", seconds: (2.5, 5.), kinds: &["dread", "action", "dialogue"] },
    // ---- narration, memory, endings
    Recipe { id: "narration-drift", name: "A slow drift for the narrator", when: "narration or exposition: one picture per thought, joined by dissolves", size: "extreme long", angle: "high", lens: "wide", composition: "one wide picture that shows what the narrator speaks of, calm, nothing in it needing to move", camera: "pan_right", motion: "camera", seconds: (5., 10.), kinds: &["narration", "montage"] },
    Recipe { id: "memory-still", name: "A washed-out memory", when: "a flashback beat: one remembered moment as a still", size: "medium", angle: "eye level", lens: "normal", composition: "the remembered moment as a soft still, colour drained almost to grey, the edges darkened", camera: "push_in", motion: "camera", seconds: (3.5, 8.), kinds: &["flashback", "montage"] },
    Recipe { id: "held-last-image", name: "The last image, held", when: "the end of the episode: its cliffhanger on one image (the eyes, the object that stands for it)", size: "extreme close-up", angle: "eye level", lens: "tele", composition: "the single image the episode ends on, filling the frame, held", camera: "push_in", motion: "camera", seconds: (4., 8.), kinds: ANY },
];

pub fn recipe(id: &str) -> Option<&'static Recipe> {
    let id = id.trim().to_lowercase().replace([' ', '_'], "-");
    RECIPES.iter().find(|r| r.id == id)
}

impl Recipe {
    pub fn suits(&self, kind: &str) -> bool {
        self.kinds.contains(&"any") || self.kinds.contains(&kind)
    }
}

/// The recipes that suit a scene of `kind`, one per line, for the director.
pub fn listing(kind: &str) -> String {
    RECIPES
        .iter()
        .filter(|r| r.suits(kind))
        .map(|r| format!("- {}: {} ({} / {} / {} lens / {} / {}; {:.0}-{:.0} s) — {}", r.id, r.when, r.size, r.angle, r.lens, r.camera, r.motion, r.seconds.0, r.seconds.1.max(1.), r.composition))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anime::{norm_angle, norm_camera, norm_lens, norm_motion, norm_size, MOTIONS, SCENE_KINDS};

    #[test]
    fn every_recipe_speaks_the_boards_language() {
        let mut ids = std::collections::HashSet::new();
        for r in RECIPES {
            assert!(ids.insert(r.id), "{} twice", r.id);
            assert_eq!(norm_size(r.size), r.size, "{}", r.id);
            assert_eq!(norm_angle(r.angle), r.angle, "{}", r.id);
            assert_eq!(norm_lens(r.lens), r.lens, "{}", r.id);
            assert_eq!(norm_camera(r.camera), r.camera, "{}", r.id);
            assert!(MOTIONS.contains(&r.motion), "{}", r.id);
            assert_eq!(norm_motion(r.motion, r.camera), r.motion, "{}: a still under a moving camera is \"camera\", a fixed one \"hold\"", r.id);
            assert!(r.seconds.0 > 0. && r.seconds.0 < r.seconds.1, "{}", r.id);
            assert!(r.kinds.iter().all(|k| *k == "any" || SCENE_KINDS.contains(k)), "{}", r.id);
            assert!(!r.composition.is_empty() && !r.when.is_empty());
        }
    }

    #[test]
    fn every_kind_of_scene_has_shots_to_choose_from() {
        for kind in SCENE_KINDS {
            let n = RECIPES.iter().filter(|r| r.suits(kind)).count();
            assert!(n >= 12, "{kind} has only {n} recipes");
            assert!(listing(kind).lines().count() == n);
        }
        assert_eq!(recipe("no-such-shot").map(|r| r.id), None);
        assert_eq!(recipe("Dirty OTS").map(|r| r.id), Some("dirty-ots"));
        assert_eq!(recipe(" dirty_ots ").map(|r| r.size), Some("over-the-shoulder"));
        // Talk has ways to hide the mouths; action has the lunge.
        assert!(listing("dialogue").contains("group-backs-pan") && !listing("dialogue").contains("lunge-at-lens"));
        assert!(listing("action").contains("lunge-at-lens"));
    }
}
