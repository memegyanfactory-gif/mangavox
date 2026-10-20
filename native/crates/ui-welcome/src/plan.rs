//! The setup's flow without any UI: which screens a choice of uses shows, where Back and
//! Continue lead, where a resumed setup starts, and what the voice download costs. One
//! decision per screen, and screens only for what the user said they will do (progressive
//! disclosure, NN/g: <https://www.nngroup.com/articles/progressive-disclosure/>).
use mv_studio::voices::{self, VoicePack};
use mv_ui::Route;

/// What the user said they will do in MangaVox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Use {
    Manga,
    Watch,
    Studio,
}

impl Use {
    pub const ALL: [Use; 3] = [Use::Manga, Use::Watch, Use::Studio];

    /// Id stored in `onboarding.uses`.
    pub fn id(self) -> &'static str {
        match self {
            Use::Manga => "manga",
            Use::Watch => "watch",
            Use::Studio => "studio",
        }
    }

    pub fn from_id(id: &str) -> Option<Use> {
        Use::ALL.into_iter().find(|u| u.id() == id)
    }
}

/// The setup screens, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Uses,
    Manga,
    Watch,
    Voices,
    Done,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Welcome, Step::Uses, Step::Manga, Step::Watch, Step::Voices, Step::Done];

    /// Id stored in `onboarding.step` / `onboarding.steps`.
    pub fn id(self) -> &'static str {
        match self {
            Step::Welcome => "welcome",
            Step::Uses => "uses",
            Step::Manga => "manga",
            Step::Watch => "watch",
            Step::Voices => "voices",
            Step::Done => "done",
        }
    }

    pub fn from_id(id: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// Screens shown for `uses`. Studio turns manga into videos, so it needs manga sources too.
pub fn flow(uses: &[Use]) -> Vec<Step> {
    let has = |u: Use| uses.contains(&u);
    let mut v = vec![Step::Welcome, Step::Uses];
    if has(Use::Manga) || has(Use::Studio) {
        v.push(Step::Manga);
    }
    if has(Use::Watch) {
        v.push(Step::Watch);
    }
    if has(Use::Studio) {
        v.push(Step::Voices);
    }
    v.push(Step::Done);
    v
}

/// The screen after `step`.
pub fn next(step: Step, uses: &[Use]) -> Step {
    let f = flow(uses);
    f.iter().position(|s| *s == step).and_then(|i| f.get(i + 1).copied()).unwrap_or(Step::Done)
}

/// The screen before `step` (the welcome screen has none).
pub fn prev(step: Step, uses: &[Use]) -> Step {
    let f = flow(uses);
    match f.iter().position(|s| *s == step) {
        Some(i) if i > 0 => f[i - 1],
        _ => Step::Welcome,
    }
}

/// Where a setup left part-way resumes: the saved screen when it is still part of the flow,
/// else the uses screen (or the welcome screen when nothing was chosen yet).
pub fn resume_at(saved: &str, uses: &[Use]) -> Step {
    match Step::from_id(saved) {
        Some(s) if flow(uses).contains(&s) => s,
        _ if uses.is_empty() => Step::Welcome,
        _ => Step::Uses,
    }
}

/// `(position, total)` of `step` among the screens between Welcome and Done, for the
/// progress dots; `None` on those two.
pub fn progress(step: Step, uses: &[Use]) -> Option<(usize, usize)> {
    let middle: Vec<Step> = flow(uses).into_iter().filter(|s| !matches!(s, Step::Welcome | Step::Done)).collect();
    middle.iter().position(|s| *s == step).map(|i| (i, middle.len()))
}

/// The page the app opens on when the setup ends.
pub fn landing(uses: &[Use]) -> Route {
    if uses.contains(&Use::Manga) {
        Route::Library
    } else if uses.contains(&Use::Watch) {
        Route::AnimeHome
    } else if uses.contains(&Use::Studio) {
        Route::Studio
    } else {
        Route::Library
    }
}

/// Voice choices offered, in order: full Kokoro, Kokoro Lite, Piper (English or Hindi) and
/// Chatterbox. Built from `voices::PACKS`, so sizes always match what is downloaded.
pub const VOICE_CHOICES: [&str; 5] = ["kokoro", "kokoro-lite", "piper-en-amy", "piper-hi-priyamvada", "chatterbox"];

pub fn voice_pack(id: &str) -> Option<&'static VoicePack> {
    voices::find_pack(id)
}

/// Bytes a voice setup downloads: the pack's files plus its engine (a rough figure for the
/// engine, which is Python packages).
pub fn voice_download(pack: &VoicePack) -> u64 {
    pack.size() + voices::engine_size(pack)
}

/// The voice pack to recommend: Kokoro, or Kokoro Lite on a PC with under 8 GB of memory or
/// under 2 GB of free disk.
pub fn recommended_voice(ram_bytes: Option<u64>, free_bytes: Option<u64>) -> &'static str {
    const GB: u64 = 1_000_000_000;
    let low_ram = ram_bytes.is_some_and(|r| r < 8 * GB);
    let low_disk = free_bytes.is_some_and(|f| f < 2 * GB);
    if low_ram || low_disk {
        "kokoro-lite"
    } else {
        "kokoro"
    }
}

/// Whether `need` bytes fit in `free` with a 500 MB margin (unknown free space fits).
pub fn fits(need: u64, free: Option<u64>) -> bool {
    free.is_none_or(|f| f >= need + 500_000_000)
}

#[cfg(test)]
mod tests {
    use super::{fits, flow, landing, next, prev, progress, recommended_voice, resume_at, voice_download, voice_pack, Step, Use, VOICE_CHOICES};
    use mv_ui::Route;

    #[test]
    fn screens_follow_the_chosen_uses() {
        assert_eq!(flow(&[]), vec![Step::Welcome, Step::Uses, Step::Done]);
        assert_eq!(flow(&[Use::Watch]), vec![Step::Welcome, Step::Uses, Step::Watch, Step::Done]);
        // Studio needs manga sources and voices.
        assert_eq!(flow(&[Use::Studio]), vec![Step::Welcome, Step::Uses, Step::Manga, Step::Voices, Step::Done]);
        let all = flow(&Use::ALL);
        assert_eq!(all, Step::ALL.to_vec());
        assert!(all.len() <= 6);
    }

    #[test]
    fn back_and_continue_walk_the_flow() {
        let uses = [Use::Manga, Use::Studio];
        assert_eq!(next(Step::Welcome, &uses), Step::Uses);
        assert_eq!(next(Step::Uses, &uses), Step::Manga);
        assert_eq!(next(Step::Manga, &uses), Step::Voices);
        assert_eq!(next(Step::Voices, &uses), Step::Done);
        assert_eq!(next(Step::Done, &uses), Step::Done);
        assert_eq!(prev(Step::Voices, &uses), Step::Manga);
        assert_eq!(prev(Step::Welcome, &uses), Step::Welcome);
        // A screen no longer in the flow (a use was unticked) continues to Done.
        assert_eq!(next(Step::Watch, &uses), Step::Done);
    }

    #[test]
    fn a_resumed_setup_starts_where_it_was_left() {
        assert_eq!(resume_at("watch", &[Use::Watch]), Step::Watch);
        assert_eq!(resume_at("watch", &[Use::Manga]), Step::Uses);
        assert_eq!(resume_at("", &[]), Step::Welcome);
        assert_eq!(resume_at("nonsense", &[Use::Manga]), Step::Uses);
        assert_eq!(Use::from_id("studio"), Some(Use::Studio));
        assert_eq!(Step::from_id("voices"), Some(Step::Voices));
    }

    #[test]
    fn progress_counts_only_the_middle_screens() {
        let uses = [Use::Manga, Use::Watch];
        assert_eq!(progress(Step::Welcome, &uses), None);
        assert_eq!(progress(Step::Uses, &uses), Some((0, 3)));
        assert_eq!(progress(Step::Watch, &uses), Some((2, 3)));
        assert_eq!(progress(Step::Done, &uses), None);
    }

    #[test]
    fn the_app_opens_on_the_first_chosen_section() {
        assert_eq!(landing(&[]), Route::Library);
        assert_eq!(landing(&[Use::Watch, Use::Studio]), Route::AnimeHome);
        assert_eq!(landing(&[Use::Studio]), Route::Studio);
        assert_eq!(landing(&[Use::Studio, Use::Manga]), Route::Library);
    }

    #[test]
    fn voice_choices_exist_with_real_sizes() {
        for id in VOICE_CHOICES {
            let p = voice_pack(id).unwrap_or_else(|| panic!("{id} is not a voice pack"));
            assert!(voice_download(p) > p.size());
        }
        // Kokoro: 337 MB of model and voices plus about 60 MB of engine.
        let k = voice_pack("kokoro").unwrap();
        assert_eq!(k.size(), 353_746_785);
        assert_eq!(voice_download(k), 353_746_785 + 60_000_000);
        // Chatterbox is the big one (engine and weights, about 8 GB).
        assert!(voice_download(voice_pack("chatterbox").unwrap()) > 8_000_000_000);
    }

    #[test]
    fn lite_voices_are_recommended_on_small_pcs() {
        const GB: u64 = 1_000_000_000;
        assert_eq!(recommended_voice(Some(16 * GB), Some(100 * GB)), "kokoro");
        assert_eq!(recommended_voice(None, None), "kokoro");
        assert_eq!(recommended_voice(Some(4 * GB), Some(100 * GB)), "kokoro-lite");
        assert_eq!(recommended_voice(Some(16 * GB), Some(GB)), "kokoro-lite");
        assert!(fits(GB, None));
        assert!(fits(GB, Some(2 * GB)));
        assert!(!fits(GB, Some(GB)));
    }
}
