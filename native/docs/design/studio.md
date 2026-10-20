# Studio: design changes

Crates: `native/crates/ui-studio` (UI), `mv-recap` (steps and resets), `mv-score` (music), `mv-studio`
(voices, takes), and `python/tts_bridge.py`, `narration_tts.py`, `chatterbox_tts.py`. Source audit:
`audit-studio.json`. Every change uses the mv-ui kit (`COMPONENTS.md`): Phosphor icons (fill weight
for on/selected, bold at 14px and below), `Tokens` only (`tests/no_raw_colors.rs` now guards the
crate), `theme::{icon, radius, space}` scales, kit buttons, chips, segmented controls, switches,
menus, empty/error states, skeletons, toasts and confirmations. No text under 11px is left.

Format: what changed, why, reference.

## Shared pieces (`widgets.rs`)

- One glyph per phase (Read scan, Transcript file-text, Story note-pencil, Audio microphone, Music notes, Video film-strip), per scene kind, per story feel and per narrator delivery; a test checks they are unique. Why: one meaning, one icon everywhere. https://fluent2.microsoft.design/iconography
- Scene tags show the kind's icon and a category hue (no raw hex), with "intensity 7/10" in the tooltip. Why: kinds were told apart by colour alone. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html
- `field_frame`: inline editors get a hover border and the 2px focus ring while focused. Why: focus in the transcript and story editors was invisible. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- `card()` / `column()` surfaces with the LG radius and 16px padding replace 9 different radii. Why: visual drift. https://fluent2.microsoft.design/design-tokens
- `stat_pill(icon, text)` for quiet metadata next to titles. Why: stats competed with titles. https://medium.com/refactoring-ui/7-practical-tips-for-cheating-at-design-40c736799886
- `tab_bar`: segmented tabs with icons, fill weight on the current tab and a live dot on a busy background tab. Why: the AI panel tabs were 11px text with Lucide icons and no state. https://fluent2.microsoft.design/components/web/react/core/tablist/usage
- `open_reset_dialog`: modal that lists exactly what a reset removes (icon, name, size), warns about lost manual edits, opens with nothing focused, Enter never confirms, buttons say the outcome ("Reset 3 steps" / "Keep my work"). Why: resets are destructive and multi-step. https://www.nngroup.com/articles/confirmation-dialog/

## Toolbar and keyboard

- Phase stepper (kit `stepper`) with phase icons, done/running/failed states, an accent connector after done steps and a tooltip per step ("Read · 12/38 pages read · Alt+1"); compact mode keeps names in tooltips. Why: numbers in circles, no running or error state. https://carbondesignsystem.com/components/progress-indicator/usage/
- Setup pill: spinner while checking, filled check "Ready" or warning "Setup · 2", with a tooltip. Why: setup state was an unexplained 11px pill. https://www.nngroup.com/articles/visibility-system-status/
- AI panel toggle is an icon button with a tooltip and selected state. Why: no icon-only control had a tooltip. https://www.nngroup.com/articles/tooltip-guidelines/
- Alt+1 to Alt+6 open a step (they never type text, so they work while editing); Esc closes the open cue, then the Setup sheet. Studio takes keyboard focus on a click when focus was outside it, since kit controls keep focus where it was. Why: no keyboard model at all. https://www.nngroup.com/articles/flexibility-efficiency-heuristic/

## Library column

- Six content-shaped skeleton rows while loading; error view with Try again; empty view with "Open Library". Why: one grey line looked broken. https://www.nngroup.com/articles/skeleton-screens/
- Search field gets a leading magnifier; header shows a download glyph and the chapter count. Why: unlabeled controls. https://www.nngroup.com/articles/icon-usability/
- Tri-state group checkbox (kit `checkbox`, 24px target) always visible; ticked rows get an accent tint distinct from the focused row. Why: hover-only 10px "All/None", 14px hit targets. https://www.nngroup.com/articles/checkboxes-design-guidelines/
- Footer is a batch bar when chapters are ticked: "3 selected" plus Read, Write story, Narrate, Render and Clear icon buttons (disabled with a reason while another AI run goes). Why: a selection did not say what it allows. https://carbondesignsystem.com/components/data-table/usage/#batch-actions
- Each chapter row shows a spinner while it works and a tooltip on its five-dot trail ("Read ✓ · Story –"). Why: an unlabeled dot trail. https://www.nngroup.com/articles/tooltip-guidelines/
- Reset chapter: a trash icon button on each row (shown on hover or when focused, danger on hover, tooltip "Reset chapter", disabled with a reason while the chapter works). It opens the reset dialog with every step's size, the chapter's own settings, leftovers and story notes, and the space freed; a toast reports the result. Why: no way to start a chapter over. https://www.nngroup.com/articles/confirmation-dialog/

## Centre: shared phase header

- Breadcrumb (manga › chapter), the phase icon in a tinted tile, the title, stat pills, the description in `text_3`. Why: muddy hierarchy. https://medium.com/refactoring-ui/7-practical-tips-for-cheating-at-design-40c736799886
- Every phase has its main action in the header, with an icon; Music and Video now too. Disabled buttons are truly disabled (no handler) and say why in the tooltip ("Read the chapter first (Read step)"). Why: fake-disabled buttons fired and then showed an error. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Reset icon button on every step header (danger, tooltip "Reset Story and the steps built from it", disabled with "Nothing to reset yet" or while the chapter works). It clears the step and every later step through `mv_recap::reset_phase`, after the dialog lists them. A Transcript reset puts the AI's text back (the read stays). Why: no way to redo a step without stale later steps. https://www.nngroup.com/articles/confirmation-dialog/
- Problems show as a kit `inline_alert` with one fixing action ("Open Setup", "Open Audio settings", "Stop the run", "Go to Story"); successes are toasts (run finished, audio or video ready, queued, rewritten, reset). Why: one sticky notice bar for everything. https://m3.material.io/components/snackbar/guidelines
- Opening a chapter shows a centre skeleton (title, description, three pages). Why: a blank empty state while loading. https://www.nngroup.com/articles/skeleton-screens/
- No chapter open: "Pick a chapter" empty view, or "Download a chapter to begin" with Open Library, plus the Setup checklist when something is missing. Why: first run was a dead end. https://www.nngroup.com/articles/empty-state-interface-design/

## Read

- Notes card stacks under the page below 720px; "Not read yet" with a dashed circle; spinner and shimmer "Reading this page…" on the page being read; "Read this page again" with the scan icon, disabled while another run goes. Why: thumbnails on narrow windows, text-only states. https://m3.material.io/foundations/layout/applying-layout/window-size-classes
- Start over / Continue from page N when a read stopped part-way. Why: a stopped read had no clear next step. https://www.nngroup.com/articles/visibility-system-status/

## Transcript

- "Edited" is a neutral tag with a pencil (not warning yellow); "Revert to AI text" with an undo arrow and an info toast. Why: an intended state used an alarm colour. https://www.nngroup.com/articles/indicators-validations-notifications/
- Ask AI box: close button, Esc closes, Enter hint, spinner while rewriting, Rewrite disabled until there is an instruction. Why: no way out, no busy state. https://www.nngroup.com/articles/indicators-validations-notifications/

## Story

- Feel chips are kit filter chips with a check, an icon per feel and a one-line tooltip from `style_hint`. Why: ambiguous labels, no selected mark. https://m3.material.io/components/chips/guidelines
- Per-passage "narrator delivery" menu (Follow the scene, Calm, Warm, Happy, Sad, Memory, Tense, Reveal, Excited, Intense, each with intensity); exported on the passage's beat, which `narration_tts.build_plan` follows. Why: no way to direct emotion. https://elevenlabs.io/blog/v3-audiotags
- Story editors sit in the focus-ring frame. Why: editable text looked static. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- Empty "No story yet" says what to do and offers "Read the chapter first" when nothing is read. Why: the empty state pointed at a button elsewhere. https://www.nngroup.com/articles/empty-state-interface-design/

## Audio

- Language and speed are segmented controls. Why: chips used as options. https://m3.material.io/components/segmented-buttons/guidelines
- Voice tiles: readiness with an icon ("Ready · Kokoro", "Kokoro stands in for now", "Not downloaded"), the voice's description, an "Expressive" tag for voices that act, the selected voice marked with the filled `selected_badge` (not an outline star), missing voices dimmed, not selectable, sample disabled, and a "Get" button to Settings → Audio. Why: the outline star read as "not favourited", missing voices were selectable. https://fluent2.microsoft.design/iconography, https://elevenlabs.io/docs/product-guides/voices/voice-library
- Picking a voice says who it applies to (scope line) and confirms with a toast; "Use default" is a ghost button with an undo arrow. Why: silent batch changes. https://www.nngroup.com/articles/visibility-system-status/
- Who narrated: "Narrated by Chatterbox" (success tag) or a warning alert "Narrated by Kokoro: Heart stood in for Narrator F" with Chatterbox's error and Open Setup; live while a job runs (from `tts_bridge._announce_engine`). Why: the user could not tell the stand-in from the real voice. https://www.nngroup.com/articles/visibility-system-status/
- Emotion strength slider per chapter (0–2×, default 1×) saved to `narration_style.json` through `mv_studio::voices::set_emotion_strength`, with a hint for flat / acting / pace-only engines and a toast offering "Regenerate audio". Why: emotion intensity could not be tuned. https://elevenlabs.io/blog/v3-audiotags
- Clip rows: play/stop with a tooltip, duration, a playback progress bar, "No audio yet" with a dashed circle, the passage's delivery and the take number as tags, Regenerate and New take as icon buttons, disabled with reasons while narration runs. Why: no duration, no progress, actions vanished while busy. https://help.elevenlabs.io/hc/en-us/sections/23795014754065-Studio
- Per-page Regenerate and New take voice only that page of the focused chapter (`regen_pages`), never the whole ticked batch; New take moves the page to its next take (`audio/takes.json`), which changes its Chatterbox seeds and cache keys; the new clip is written beside the old one and replaces it only when usable. Why: one row's Regenerate re-narrated every ticked chapter and deleted the clip first. https://help.elevenlabs.io/hc/en-us/articles/30065456369169-Can-I-regenerate-individual-words-in-Studio

## Music

- Compose / Update score is the header's primary action, disabled with a reason (no narration, music off), busy while composing; success toast with the cue count. Why: the same place for the main action on every step. https://www.nngroup.com/articles/consistency-and-standards/
- Settings card: a "Music for this chapter" switch, then rows with icons and hints: Level and Duck as segmented controls, Sound effects and AI spotting as switches, the orchestra row with an icon tile, size and Download / progress / Try again. Why: chips used as on/off switches, unlabeled options. https://www.nngroup.com/articles/toggle-switch-guidelines/
- Out-of-date score and missing narration are inline alerts with "Update score" / "Go to Audio". Why: a coloured sentence with no action. https://www.nngroup.com/articles/error-message-guidelines/
- Timeline: header with duration, cue count and synth/orchestra pills; ruler ticks; cues as soft category-hue blocks with the style's icon and a solid left edge (selected: stronger fill and border), a tooltip per cue; energy bars; lanes with an icon per track, mute and solo as icon toggles (filled when on), −/+ 3 dB icon buttons disabled at the limits; no raw hex or white text. Why: raw colours, M/S letters with no tooltip, colour-only cues. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html, https://m3.material.io/components/icon-buttons/overview
- Cue inspector: icon tile, timing, tags, Play from here, New variation, Close (Esc), and the mood as filter chips with icons. Why: plain chips and buttons without icons. https://m3.material.io/components/chips/guidelines

## Video

- Format (16:9 Landscape / 9:16 Reels) and resolution are segmented controls with icons; Render is in the header; the blocker is a warning alert with "Go to Audio" / "Open Setup". Why: the action sat at the end of a wrapping row, the reason was faint text. https://www.nngroup.com/articles/consistency-and-standards/
- Rendered video: hover border on the poster, metadata pills (length, ratio, size, "Rendered 2 min ago"), Show in folder and Edit in Bhippi with icons, the path as a `path_chip`; reels show their length; scrim tokens replace raw black/white. Why: no metadata, text-only actions. https://m3.material.io/components/cards/guidelines

## AI panel (`side.rs`)

- Activity: empty view with "Read this chapter"; the run card has a status tag with icon (Working / Done / Stopped / Failed), a Stop icon button with tooltip, the model, a progress line, an error alert on failure, Continue / Start over after a stop, met characters as accent tags, and the raw log behind a "Show log (42 lines)" disclosure. Why: an always-open 260px log, text-only status. https://www.nngroup.com/articles/progressive-disclosure/
- Jobs: "Audio and video" header with a "Clear finished" ghost button, status tags with icons, page jobs named "Page 5 · Chapter 3", the narrating engine as a tag (warning when a stand-in speaks), Cancel icon buttons with tooltips. Why: uppercase 11px header, bare text link, untitled x buttons. https://www.nngroup.com/articles/icon-usability/
- Characters: actions appear on hover (Rename, Keep this name toggle with a filled lock when on, Forget character in danger); rename shows Enter / Esc hints and Esc cancels; Forget is immediate with an Undo toast (faces stay on disk, so Undo restores it exactly); merging by renaming says so in a toast; empty view offers "Read this chapter". Why: three noisy buttons per card, a permanent delete with no undo. https://www.nngroup.com/articles/confirmation-dialog/
- Chat unavailable is an empty view with Try again. Why: a dead end. https://www.nngroup.com/articles/error-message-guidelines/

## Setup sheet (`setup_view.rs`)

- Header with a wrench tile, "N items need attention", "Checked 2 min ago" and a refresh icon button (spinner while checking); skeleton rows before the first check. Why: a text-only Re-check hidden in the sheet. https://www.nngroup.com/articles/visibility-system-status/
- Rows with an icon tile, a filled status icon, the path as a copyable chip, and downloaded voice packs as success tags. The voices row tells "Voices downloaded, engine not installed" from "No voices installed". Why: models and engine were one "No voice engine installed" row. https://www.nngroup.com/articles/error-message-guidelines/
- "Set up voices" runs `mv_studio::voice_setup` (Python if needed, the engine, the pack, a load check) with live progress (step, detail, download bar, Stop), Try again on failure, a toast at the end, and a re-check; any setup job started in Settings → Audio shows here too. "Pick other voices" opens Settings → Audio. Shell commands moved under a "Manual install (terminal)" disclosure with copy buttons. Why: the sheet installed nothing and Studio and Settings sent the user back and forth. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/

## Engine and files

- `mv_recap::reset_plan` / `reset_phase` / `chapter_plan` / `reset_chapter` with `Step`, `Footprint`, `ResetPlan`, `ResetReport`: a step and all later steps, measured before removal, settings kept on a step reset; a chapter reset also drops its settings, leftovers and its story-note lines (the pages and the character bank stay); Transcript resets rebuild from the read; temp-dir tests for each, plus an mv-score test that the Music reset removes exactly what mv-score writes. Why: resets must never leave work that describes deleted work. https://www.nngroup.com/articles/confirmation-dialog/
- The Studio library marks (`mark_stages_reset`) are cleared with the files, and the chapter's own voice override goes with a chapter reset. Why: rows kept showing "done".
- `chapter_status` ignores `page_NNNN.new.wav` (a take being voiced). Why: half-made takes counted as clips.
- `mv_studio::voices::{page_take, new_take, takes_path}`; `narration_tts.segment_seed(page, index, text, take)` and `load_takes`; `tts_bridge.run_tts` takes `regen_pages`, mixes each page's take into its seeds, voices into a side file and commits only a usable clip, and announces the engine and stand-in on the job's protocol. Why: New take, safe regenerate and "who narrated".

## Skipped audit items

- Resizable / collapsible library and AI panel (low): needs drag handles and persisted widths; left for a layout pass. The AI panel already collapses.
- "Previous take" history (part of the per-page regenerate item): the new clip replaces the old one once it is good; keeping takes side by side needs storage and a picker. New take plus the safe swap cover the loss-of-work risk.
- Per-passage free intensity slider and pace options (part of the emotion item): the delivery presets carry kind and intensity, and the chapter's Emotion strength scales them; a slider per passage would crowd the story rows.
- Narrator scope as a segmented control (medium): the scope sentence and the toast say who a pick applies to; the default narrator stays in Settings → Audio, one place.
- Pause/resume for clips: the audio service only plays and stops, so the button honestly shows Stop.
- Space and J/K shortcuts: Space activates the focused kit control and J/K would type in the editors; Alt+1–6 and Esc ship instead.
- Resolution in the rendered-video facts: the file does not record it, and the current setting may differ.
- Settings → Models copy that points back at Studio (ui-settings): not this crate; listed as a need.

## Review fixes (adversarial pass)

- Resets: `captions.srt` (sync_map.py, timed to the clips) now goes with Audio, and scene_narrator's `narration.<provider>.md` with Story; before, both survived the step they describe. Why: a reset must leave nothing describing deleted work. https://www.nngroup.com/articles/confirmation-dialog/
- Resets: `job.json` / `job_<n>.json` no longer count as Video output. Every audio job writes one, so an Audio reset on a chapter with no video listed "Video" in the dialog and the toast ("Reset 2 steps"); the chapter reset still sweeps them. Why: the dialog must say exactly what goes. https://www.nngroup.com/articles/confirmation-dialog/
- Resets refuse a blank or relative chapter folder (every step's files would resolve against the app's working directory). Why: a destructive action must never reach outside its folder. https://owasp.org/www-community/attacks/Path_Traversal
- Reset tests: every artifact the app, the pipeline, mv-score and the voices write is placed, each of the six step resets is checked file by file (and against `chapter_status`), and the chapter reset is checked to look brand new while the pages, another chapter of the manga, the character bank and prefs stay untouched.
- The reset confirm re-checks the blockers when it runs and only resets the chapter the dialog was about. Why: work may start while the dialog is open. https://www.nngroup.com/articles/confirmation-dialog/
- A page rewrite that finishes after you opened another chapter is saved into the chapter it was asked for (it used to land in the open chapter's transcript), and blocks that chapter's reset until it lands. Why: silent data loss. https://www.nngroup.com/articles/user-control-and-freedom/
- A batch score blocks resets and shows busy only for the chapters it scores (the busy state used to follow the focus to other chapters); the Music header loads the chapter's music before drawing its Compose button. Why: wrong state shown. https://www.nngroup.com/articles/visibility-system-status/
- New take: the take number is recorded by tts_bridge only once the new clip replaced the old one (it was bumped before the job, so a failed take showed "Take 3" over take 2's clip). Supersedes "New take moves the page to its next take" above. Why: the row must show the take you hear. https://www.nngroup.com/articles/visibility-system-status/
- Per-page jobs: when every page asked for fails, the job fails with the reason (it used to say "Audio ready" over the old clip), and when Chatterbox cannot start on a per-page job the pages keep their clips instead of a Kokoro stand-in mid-chapter. Why: honest status, no mixed voices. https://www.nngroup.com/articles/error-message-guidelines/
- A passage's picked delivery keeps its intensity ("Intense · 9" is said at 9); only its neighbours ramp through the smoothing. Why: the direction the user gave was averaged away. https://elevenlabs.io/blog/v3-audiotags
- Voice tiles say "Engine not installed" (package icon, "Set up") when the voice files are there but the engine is not, instead of "Not downloaded". Why: audit high item, contradictory states with Settings → Audio. https://www.nngroup.com/articles/error-message-guidelines/
- "Use default" shows when any chapter a pick applies to has its own voice (it followed the focused chapter only). Why: the button's scope must match its action. https://www.nngroup.com/articles/consistency-and-standards/
- Rendered-video facts read the ratio from the files (reels folder = 9:16), not the Format setting. Why: wrong state shown after changing Format. https://m3.material.io/components/cards/guidelines
- Resetting a chapter that is not open no longer stops a clip of another chapter (only the reset chapter's own clips and score stop). Why: an action should not touch unrelated work. https://www.nngroup.com/articles/user-control-and-freedom/
- Esc only closes a selected cue while the Music step shows it (a cue left selected there swallowed Esc on every other step). Why: one key, one visible effect. https://www.nngroup.com/articles/flexibility-efficiency-heuristic/
- Phase header: the title keeps 260px and the actions wrap under it (and within themselves) on narrow centres, instead of the stat pills running under the buttons at 1280px with both side columns open. Why: layout robustness. https://m3.material.io/foundations/layout/applying-layout/window-size-classes

- Audio rows read the narrator from memory instead of prefs.json and config.json; rows redraw every animation frame while a job spinner turns, so each visible row read two files per frame. The video ratio is read once per load. Why: smooth frames during narration. https://web.dev/articles/rail

### Checked, no change needed

- Reset dialog: opens with the dialog's own focus (no button focused), Enter maps to `on_ok`, which returns false; Tab reaches "Keep my work" first.
- Debug-build panics: no kit builder gets a second `.hover()` / `.tooltip()`; `group_hover` on icon buttons is not asserted by GPUI.
- Hidden row actions (Reset chapter, character actions) are not Tab stops while hidden (GPUI skips hidden elements' tab stops), and the focused chapter's row always shows its reset.
- Lucide paths and raw colours: none (guarded by `tests/no_raw_colors.rs`).

### Skipped in this pass

- A failed AI run or job keeps its "failed" mark on the stepper until the next run or "Clear finished", even after a reset (the job list is history; filtering it by reset time needs a timestamp per job).

## Review fixes (second adversarial pass)

- Studio takes keyboard focus when it opens, and takes it back whenever the focused control goes away (an editor of another step after Alt+1–6, the closed Ask box, a finished or cancelled rename, editors rebuilt after a reset). Before, Alt+1–6 did nothing until the first click, and after one Alt+N from inside an editor the next shortcut reached no view. Why: shortcuts must not depend on where the mouse last was. https://www.w3.org/WAI/ARIA/apg/practices/keyboard-interface/
- Editors write back only what the user typed. An editor made before its page was read still held the blank text; any later save (switching step, typing elsewhere) wrote that blank over the AI's transcript and marked the page Edited. A refresh now rebuilds untouched editors from the fresh data (pages read during a run show up in the Transcript step at once), keeps an edit in progress when a run finishes (it used to be dropped), and never moves an in-progress story edit onto another passage when the AI rewrote the story. Why: silent data loss. https://www.nngroup.com/articles/slips/
- Header actions follow the job that is running: Video offers "Cancel render" only for a render (with narration running, Render is disabled: "Wait for the narration to finish"); Audio offers "Cancel narration" only for narration (a render disables Generate with its reason); clip-row buttons name the job they wait for. Before, "Cancel render" stopped the narration. Why: a control must do what it says. https://www.nngroup.com/articles/consistency-and-standards/
- A per-page Regenerate pressed while the setup check is still running no longer leaves the row spinning "Voicing this page…" (nothing was queued). Why: honest status. https://www.nngroup.com/articles/visibility-system-status/
- Resetting a chapter that is not open no longer rebuilds the open chapter's editors (it dropped what was being typed there). Why: an action must not touch unrelated work. https://www.nngroup.com/articles/user-control-and-freedom/
- A step reset clears the library marks of that step and every later one, not only the steps that still had files (a mark could outlive files removed by hand and keep the row "done"). Why: wrong state shown. https://www.nngroup.com/articles/visibility-system-status/
- A reset that stops a clip it is about to delete waits (up to 0.6 s, off the UI thread) for the player to let go of the file; the stop is asynchronous. Why: avoid a spurious "could not be removed (in use?)". https://www.nngroup.com/articles/error-message-guidelines/
- Resets remove a link inside the chapter folder (symlink or Windows junction) as a link: a directory junction used to fail ("could not be removed") and leave the step half reset. Links were never followed; two new Windows tests pin that (a junction pointing outside the chapter, and a file held open by another program: reported, the rest still goes, a second reset finishes). Why: a destructive action must stay inside its folder and finish. https://cwe.mitre.org/data/definitions/59.html
- Characters: Rename, Keep this name and Forget stay visible while the keyboard is in use. Hidden elements are no Tab stops, so without a mouse they could not be reached at all. Why: WCAG 2.1.1 Keyboard. https://www.w3.org/WAI/WCAG22/Understanding/keyboard.html

### Checked in this pass, no change needed

- Reset dialog: GPUI focuses the dialog's own handle on open (no button), Enter maps to `on_ok`, which returns false; Esc and the backdrop cancel.
- No kit builder gets a second `.hover()` / `.tooltip()` (GPUI asserts both, plus `on_hover` and `on_drag`); `group_hover` and `invisible()` are not asserted. Hidden elements return before registering mouse handlers and tab stops.
- `tts_bridge.run_tts` per-page Regenerate (scratch harness with a fake engine): a failed page keeps its clip and fails the job with the reason, a crashed take's `page_NNNN.new.wav` is discarded, a good one replaces the clip in place, the other pages are reused, a page without a clip is voiced straight into place, and a failed new take records no take.
- Every step reset and the chapter reset against every artifact the app, the pipeline, mv-score and the voices write (existing tests), and `chapter_status` afterwards.

### Skipped in this pass

- The Chat dock keeps focus when the AI panel is closed while typing in it (its focus handle is private to ui-assistant); a click anywhere in Studio gives focus back.
- Story-note lines are matched by chapter label ("- [Chapter 3 p"): two downloaded chapters with the same label share them, so resetting one forgets both chapters' notes.
- Python missing: the Setup row offers only the terminal command. `voice_setup` can install a private Python, but the pipeline runs on the system Python that `discover_python` finds, so a one-click fix there would not make Studio runnable (needs mv-studio / Settings agreement on one interpreter).
