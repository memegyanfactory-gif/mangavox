# Player (ui-player) design changes

One line per change: what, why, reference. Code: `native/crates/ui-player/src/{view,ui,logic,backend}.rs`.

## Settings honoured (Settings → Playback)

- Read `playback.autoPlay`, `autoPlayTimer`, `preDownloadNext`, `playback.skipIntro` and `playback.subtitleLanguage` from `config.json` on every start and retry; the switches were ignored before. Why: a setting that does nothing breaks trust (NN/g heuristic 4, consistency and standards). https://www.nngroup.com/articles/ten-usability-heuristics/
- Auto-play next: when the Up Next card shows, a countdown of the chosen length (10 to 60 s) runs, then the next episode plays; it waits while paused, runs on the frozen last frame, and Cancel stops it. Why: Google Cast autoplay checklist (countdown, Play now, a way to stop). https://developers.google.com/cast/docs/design_checklist/cast-autoplay
- Skip intro automatically: an opening (AniSkip OP / mixed OP) is skipped once as it starts, with an "Skipped intro" toast and Undo that seeks back without re-skipping. Why: automatic actions must be reversible (Material 3 snackbar with action). https://m3.material.io/components/snackbar/guidelines
- Pre-download next: at 75% of the episode the next episode's source is looked up and its torrent warmed (`backend::prefetch`), so Play now starts at once. Why: Cast checklist asks for the next item to be ready when the countdown ends. https://developers.google.com/cast/docs/design_checklist/cast-autoplay
- Subtitle language: mpv `slang` is set to every code of the chosen language (`es` → `es,spa`) before loading, so embedded and added tracks follow the setting instead of always English. Why: mpv picks tracks by `slang`. https://mpv.io/manual/stable/#options-slang

## Up Next card (new)

- Glass card bottom-right from the credits start (known AniSkip outro) or `max(countdown, 30)` s before the end: next episode still, "UP NEXT · S1 · E4", episode name, filled Play now (kbd N), Cancel, countdown ring with seconds. Why: Cast checklist, "at least 30 s before the end or when credits start". https://developers.google.com/cast/docs/design_checklist/cast-autoplay
- Still and name come from Jikan + Kitsu stills (anime) or Cinemeta / metahub (series); skeleton placeholders while they load, the show's artwork as fallback. Why: skeletons for content of known shape. https://www.nngroup.com/articles/skeleton-screens/
- The card replaces "Skip outro" while visible. Why: one end-of-episode action at a time, as Netflix swaps Skip Credits for Next Episode (NN/g heuristic 8, minimalist design). https://www.nngroup.com/articles/ten-usability-heuristics/
- Seeking back before the credits hides the card and resets the countdown. Why: the countdown must never fire on a scene the viewer went back to watch (WCAG 2.2.1 timing adjustable). https://www.w3.org/WAI/WCAG22/Understanding/timing-adjustable
- A failed next-episode lookup stops the countdown and shows an error toast with Retry instead of retrying every frame. Why: say what failed and offer the recovery. https://www.nngroup.com/articles/error-message-guidelines/

## Bottom bar

- Transport glyphs are Phosphor fill (play, pause, skip forward, speaker); line glyphs on video use bold (rewind/forward arcs, gauge, corners, subtitles off). Why: filled = solid transport and on states; heavier strokes stay legible over moving pictures. https://fluent2.microsoft.design/iconography
- Sizes from `theme::icon`: transport HERO 32px in a 52px target, the rest XL 24px in 44px. Why: 44x44 pt minimum target. https://developer.apple.com/design/human-interface-guidelines/accessibility and https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum
- Every control has a tooltip with its shortcut chip (Play · Space, Back 10 seconds · J, Mute · M, Next episode · N, Full screen · F, Back · Esc). Why: icon-only buttons need labels; show shortcuts where the command lives. https://www.nngroup.com/articles/icon-usability/ and https://developer.apple.com/design/human-interface-guidelines/keyboards
- One "Audio & subtitles" button with the two-column menu; the separate Audio language button and menu are gone. Why: two controls listed the same audio tracks (heuristic 8). https://www.nngroup.com/articles/ten-usability-heuristics/
- The subtitles glyph is filled while subtitles are on. Why: state visible at a glance, not only inside the menu (heuristic 1). https://www.nngroup.com/articles/ten-usability-heuristics/
- The speed button shows the rate ("1.5×") whenever it is not 1×; menu labels use the × sign. Why: visibility of system status. https://www.nngroup.com/articles/ten-usability-heuristics/
- "Next episode" pill became an icon control with tooltip (and a spinner while the lookup runs). Why: the Up Next card now carries the label; YouTube / Netflix use an icon here. https://support.google.com/youtube/answer/7631406
- Hover, pressed and focus-visible states on every control (light state layers, kit focus ring, Tab stops). Why: Material 3 state layers; WCAG 2.4.7. https://m3.material.io/foundations/interaction/states/state-layers and https://www.w3.org/WAI/WCAG22/Understanding/focus-visible
- Scrub track uses `Tokens::track()` (3:1 on the dark gradient), buffered and hover fills are light tints, intro / credits markers use categorical hues instead of the accent and status amber. Why: WCAG 1.4.11 non-text contrast; status colours are for status only. https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast
- Volume slider: tokenised track, tooltip with the level; the warning colour above 100% stays (amplified audio can distort). Why: status colour used for a real state. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color

## Menus

- Audio & subtitles and Speed menus are glass panels (`glass_strong`, `glass_border`, radius LG) with rows matching `mv_ui::menu_row` (radius SM, hover layer, accent check, semibold selected). Why: one menu style across the app; selection not by colour alone. https://m3.material.io/components/menus/guidelines and https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Column heads use Phosphor (waveform, subtitles, gauge); "Off" uses subtitles-slash; unknown languages and failed flag images fall back to a globe. Why: one glyph per meaning (`glyphs::SUBTITLES`). https://developer.apple.com/design/human-interface-guidelines/sf-symbols

## Keyboard

- Added YouTube's standard keys: C subtitles on/off, 0 to 9 jump to that tenth, Home / End, Shift+< / Shift+> speed, , / . frame step, ? shortcut sheet. Why: the de-facto player keys people bring along. https://support.google.com/youtube/answer/7631406
- "?" opens a shortcut sheet (glass panel, key caps via `mv_ui::kbd`); Esc closes the sheet, then menus, then full screen, then the player. Why: discoverable shortcuts. https://developer.apple.com/design/human-interface-guidelines/keyboards
- Focus fix: kit controls no longer move focus on click, so every click in the overlay (and in the fallback in-window controls) hands focus back to the player root (`keep_focus`), keeping shortcuts alive. Why: mv-ui COMPONENTS.md focus rule. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible

## Loading screen

- Artwork for every source kind: metahub background for IMDb titles, the MAL / AniList banner (or cover) for anime, fetched in the player. Why: anime loaded on plain black. https://developer.apple.com/design/human-interface-guidelines/loading
- Kit spinner, `shimmer_text` status and kit `progress_bar` (determinate while caching, sweeping otherwise) replace the hand-rolled ring, dots and line; under Reduce motion they slow down or hold still, and the backdrop appears without its fade. Why: one loading language app-wide. https://developer.apple.com/design/human-interface-guidelines/loading
- Peer stats show while torrent metadata is still arriving (info hash read from the magnet). Why: visibility of system status during the longest wait. https://www.nngroup.com/articles/ten-usability-heuristics/
- The forever-counting seconds counter is gone; after 20 s at 0 peers (30 s for other streams) a plain-language hint and a primary "Try another source" appear. Why: offer the way out when waiting will not help. https://www.nngroup.com/articles/error-message-guidelines/
- Title, episode and status use the type ramp (TITLE, BODY_LARGE, BODY). Why: shared typography tokens. https://fluent2.microsoft.design/typography

## Error screen

- Raw anyhow chains are mapped to plain titles and advice ("No one is sharing this file right now", "Couldn't reach the stream", "This video can't be played"...), with the raw text under a "Details" disclosure plus Copy, and logged to `main.log`. Why: human-readable, precise, constructive; progressive disclosure for the technical part. https://www.nngroup.com/articles/error-message-guidelines/ and https://www.nngroup.com/articles/progressive-disclosure/
- Kit `error_view` layout; primary "Try another source" (back to the title's source list) when another source can help, secondary Try again; local files and engine failures get Try again + Back. Why: one primary action, recovery first. https://www.nngroup.com/articles/error-message-guidelines/
- The top bar (Back, window buttons) stays on the error screen. Why: the window could not be moved or closed from the error state before (heuristic 3, user control and freedom). https://www.nngroup.com/articles/ten-usability-heuristics/

## Top bar and feedback

- Back is a glass round control with tooltip and Esc chip; title / episode use the ramp (SUBTITLE, BODY). Why: labelled icon control; shared type. https://www.nngroup.com/articles/icon-usability/
- Window buttons use the shell TitleBar's caption glyphs and its danger fill behind Close, with tooltips (Minimize, Maximize / Restore down, Close). Why: entering the player must not change the caption buttons. https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design
- The ad-hoc top-bar message pill is replaced by kit toasts mounted in the overlay window (above the bottom bar while it shows): "Resuming at 12:34" with Start over, "Skipped intro" with Undo, next-episode progress and errors with Retry; player toasts are dismissed on exit. Why: one feedback component with actions. https://m3.material.io/components/snackbar/guidelines
- Skip intro pill: radius MD, glass fill, filled skip glyph, tooltip with S, focus ring. Why: Netflix-style outlined pill, labelled shortcut. https://developer.apple.com/design/human-interface-guidelines/keyboards

## Tokens, motion, hygiene

- Every colour from `Tokens` (video black from `scrim`, gradients from `scrim` alphas, glass surfaces, light state layers from `text_1` alphas); a crate test (`tests/no_raw_colors.rs`) fails on raw colour literals or Lucide `icons/` paths. Why: Appearance presets reach the player. https://fluent2.microsoft.design/design-tokens
- Radii from `theme::radius` (menus LG, pills MD, badges XL); control fade uses `duration::SLOW`. Why: one shape and motion scale. https://m3.material.io/styles/motion/overview
- Under Reduce motion the bars, menus, Skip and Up Next fade without sliding and the centre indicator stops growing. Why: replace motion with dissolves. https://developer.apple.com/design/human-interface-guidelines/motion
- Example harness (`examples/play.rs`) uses `Tokens::base()`.

## Audit items skipped or changed

- `PlayRequest.artwork` in mv-ui `nav.rs` (low): not mine to edit. Same result done locally (`backend::artwork`); the field would still save a Jikan request and let Continue Watching pass exact art.
- "Try another source" opening the detail page with the sources panel open: the player goes Back; the cached detail page keeps its source list open when playback started from it (`StreamState::Picking`). From other entry points (Continue watching) it returns there instead. Needs a route flag in ui-media to always reopen the list.
- `brand/play-fill.svg`, `pause-fill`, `skip-forward-fill` (missing icons): not needed, Phosphor fill weights cover them.
- Separate audio button "only when 2+ audio tracks and no subs": removed entirely; the merged menu always lists both.
- Flags (shared need: local flag SVGs in mv-ui): kept on flagcdn with a globe fallback until mv-ui ships a local set.
- Confirm for destructive actions: the player has none besides closing the app window, which matches the shell title bar (no confirm).

## Adversarial review (second pass)

- "Resuming at 12:34" toast now waits for the first frame instead of appearing when the source resolves: mpv cannot seek before the file is loaded, so Start over did nothing during a long torrent load, and the toast often expired before the picture showed. Why: an action offered must work when offered (NN/g heuristic 1, visibility of system status). https://www.nngroup.com/articles/ten-usability-heuristics/
- Up Next no longer counts down to an episode that does not exist: when the next-episode lookup (run at 75%) has failed, as on a season's last episode, the card stays hidden; N and the Next button still retry. Before, the card counted down and then showed an error toast. Why: never promise an action the system cannot do (Cast autoplay checklist assumes a next item). https://developers.google.com/cast/docs/design_checklist/cast-autoplay
- The auto-play countdown holds while the Audio & subtitles / Speed menu or the shortcut sheet is open, so the episode never changes mid-choice. Why: WCAG 2.2.1 timing adjustable. https://www.w3.org/WAI/WCAG22/Understanding/timing-adjustable
- Menus are drawn above the Skip pill and the Up Next card. They open over the same corner, and the occluding card was covering the menu's lower rows and taking their clicks. Why: the surface the viewer just opened is the topmost (Material 3 menus). https://m3.material.io/components/menus/guidelines
- Menus shrink to the room between the bar and the top bar's Back row on short windows (the 480 px minimum), and the lists scroll (Speed too). Before, a 420 px menu ran under the top bar and lost its column heads. Why: content must stay visible at the minimum size (WCAG 1.4.10 reflow). https://www.w3.org/WAI/WCAG22/Understanding/reflow
- Shift+< / Shift+> match the typed character first, so layouts with their own `<` key (German, Nordic, where `>` is Shift+<) no longer slow down when asked to speed up; `,` / `.` frame-step only unshifted. Why: shortcuts must follow the user's layout. https://learn.microsoft.com/en-us/windows/apps/design/input/keyboard-accelerators
- Ctrl+K stays on the player: the shell's command palette opened in the app window under the video, took keyboard focus and silenced every player shortcut until Escape. Why: focus must never move to something hidden (WCAG 2.4.11 focus not obscured). https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html
- Error mapping: mpv's generic "loading failed" for a stream now reads "Couldn't open the stream / the link may have expired" with Try another source (it said the file was damaged or in an unsupported format); for a local file it stays "This video can't be played" without suggesting another source; mpv audio / video output failures read as a device problem, not a source problem. Why: precise, constructive error messages. https://www.nngroup.com/articles/error-message-guidelines/
- Hovering an open menu button now brightens its highlight instead of dimming it (the hover layer was lighter than the open state). Why: Material 3 state layers stack hover on selected. https://m3.material.io/foundations/interaction/states/state-layers
- Unmute with the volume dragged to 0 restores 50% instead of muting silence; the button already showed "Unmute" there. Why: the control must do what its label says (YouTube behaves this way). https://support.google.com/youtube/answer/7631406
- Shortcut sheet: "0 – 9" for the tenth jumps (it showed "0 / 9", which reads as "0 or 9"); "Skip intro, recap or outro" for S, which skips any AniSkip segment. Why: labels must match behaviour. https://www.nngroup.com/articles/ten-usability-heuristics/
- Correction to "Subtitle language" above: `slang` steers the embedded track mpv picks at load; external addon subtitles are added unselected and listed in the menu, they are not switched on automatically.
- Clippy: `is_multiple_of` for the poll counters, collapsed match guard in `overlay.rs`; ui-player is clippy-clean.

### Review items left as they are

- Shell (app/src/shell.rs, not mine): guard `toggle_palette` on focus routes (player, reader) so the player does not need its own Ctrl+K binding; Alt+Left and Ctrl+1/2/3 leave the player through the shell without `exit()`, so a player toast (Undo a skip) can outlive the player by a few seconds.
- A transient failure of the 75% lookup hides Up Next for that episode (no automatic second try); N retries.
- External subtitles are not auto-selected for the preferred language (would turn subtitles on by default for English films with the default "en"; needs a product decision).
- Toasts and the Skip pill can touch at the 720 px minimum width when both show (resume toast during an intro).
