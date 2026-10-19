# Settings (native/crates/ui-settings) design changes

One line per change: what, why, reference. Code: `native/crates/ui-settings/src/{lib,ui,tabs,audio,storage,system,cli_tools}.rs` and `src/models/{view,scan,jobs}.rs`.

## Rail and page frame

- Every destination is a nav_item-shaped row (active container, 3px accent bar, hover, pressed, focus ring, Tab stop) with a 22px colour-coded icon tile; the selected tile turns solid with a fill-weight glyph. Why: 15 destinations scan faster by colour and shape; filled marks selection. https://developer.apple.com/design/human-interface-guidelines/sidebars and https://fluent2.microsoft.design/iconography
- Icons are Phosphor, one per meaning: App mode = app window (the toggle glyph read as a switch that is off), AI providers = circuit head (the sparkle collided with Studio's wand), Model Library = `glyphs::MODEL`, Playback = play circle, Sources = `glyphs::SOURCES`. Why: distinct, recognisable icons. https://www.nngroup.com/articles/icon-usability/
- Search field under the "Settings" title filters the rail by label, description and keywords ("subtitle", "resource limit", "chatterbox"); Enter opens the first match; no match shows a compact `empty_view` with example words. Why: 15 pages are too many to guess. https://support.apple.com/guide/mac-help/mchl8d10839d
- Group "Projects and media" renamed "Device" (Storage, System). Why: the label did not describe its pages. https://www.nngroup.com/articles/navigation-ia-tests/
- Help & feedback has a trailing arrow-up-right and a tooltip saying it opens the browser. Why: links that leave the app must say so. https://www.nngroup.com/articles/icon-usability/
- CLI tools shows an accent dot when an update is pending. Why: visibility of system status. https://www.nngroup.com/articles/visibility-system-status/
- Page heading is a 40px duotone hue tile matching the rail item, the title and a one-line description. Why: macOS panes repeat the pane icon; hierarchy by size and weight. https://www.refactoringui.com/previews/labels-are-a-last-resort
- Pages are kit `setting_section` / `setting_row` with hairline dividers; rows that name a thing get a leading 16px icon (`icon_row`), folders a middle-truncated `path_chip` with copy (`path_row`). Why: one grouped-section pattern for every page instead of two looks. https://developer.apple.com/design/human-interface-guidelines/layout
- Page changes fade in with `mv_ui::appear` (instant under reduced motion). Why: a short transition shows that content changed. https://developer.apple.com/design/human-interface-guidelines/motion
- Embedded panels (Sources, Addons, Providers) show skeleton rows while they load instead of a second centred title. Why: skeletons show the layout that is coming. https://www.nngroup.com/articles/skeleton-screens/
- All feedback is `toast::show` (success 4 s, errors longer); `notice_bar` and the per-page notes are gone. Why: one brief, auto-dismissing feedback channel. https://m3.material.io/components/snackbar/guidelines
- `ui_settings::current_tab(&AnyView, cx)` and a public `Tab::keywords()` for the shell's breadcrumb and command palette. Why: the shell could not name the Settings page in the breadcrumb. https://www.nngroup.com/articles/breadcrumbs/

## General, Playback, App mode, Studio

- Every control saves as it changes; the three "Save changes" buttons are gone. Text fields save on blur or Enter, show a passive "Saved" check beside the field for 1.8 s, and validate the email under the field. Why: switches must take effect immediately; feedback belongs next to the element. https://www.nngroup.com/articles/toggle-switch-guidelines/ and https://www.nngroup.com/articles/indicators-validations-notifications/
- Leaving General saves an edited field. Why: a click on the rail does not always blur the input, and edits were lost silently. https://www.nngroup.com/articles/user-control-and-freedom/
- Playback: three kit `switch`es and three selects that save on pick; the countdown select is disabled with a hint while auto-play is off. Why: dependent controls should say why they cannot be used. https://www.nngroup.com/articles/toggle-switch-guidelines/
- App mode: one Studio switch with a toast, plus Studio and Assistant rows with On/Off tags. Why: show what the mode adds. https://www.nngroup.com/articles/visibility-system-status/
- Studio resolution is the kit `segmented` control (720p / 1080p, check on the selected one) instead of two buttons with the selected one as Primary. Why: a selection is not a call to action. https://m3.material.io/components/segmented-buttons/overview
- Studio speed is a 0.5–2.0× slider in 0.05 steps with a live "1.00×" value and a play button that speaks the default narrator at that speed. Why: free text validated only on Save; let people hear the setting. https://m3.material.io/components/sliders/guidelines
- "Choose in Audio" button has the waveform glyph and a caret. Why: icon plus label; shows it navigates. https://www.nngroup.com/articles/icon-usability/

## Appearance

- Each preset tile is a miniature of the app drawn in that preset's real colours from `mv_ui::theme::PRESETS` (sidebar, card, accent button, secondary accent), with name, description and a "Default" tag. Why: judge a theme before picking it. https://developer.apple.com/design/human-interface-guidelines/dark-mode
- The active tile has an accent border, accent-soft fill and the kit `selected_badge` (filled check) instead of the word "Active". Why: filled marks selection; never colour alone. https://fluent2.microsoft.design/iconography and https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Picking a preset calls `theme::set_palette` so it applies at once, and Reduce motion calls `motion::set_reduced`. Why: the presets and motion setting were saved but did nothing. https://www.nngroup.com/articles/visibility-system-status/

## Audio

- Top of the page is the default narrator: 52px hero tile, the voice name with a FILLED gold star "Default" tag, engine / language / style facts, a Ready status tag (or "stands in" / "Can't speak yet" pills with icons) and a Preview button that spins while it renders. Why: the most-used setting first; status in words and shape. https://elevenlabs.io/docs/product-guides/voices/voice-library
- When the narrator cannot speak (or a stand-in speaks for it) an `inline_alert` names the problem and has a "Set up Kokoro/Chatterbox/Piper" action. Why: errors must offer the fix. https://www.nngroup.com/articles/error-message-guidelines/
- One card per engine (Kokoro, Chatterbox, Piper): duotone icon tile, Recommended / Emotion tags, status tag (Ready, Needs engine, Not installed, Installing, Setup failed, Checking), tagline, quality and speed `meter`s, size (voices + engine), languages, voice count and GPU or CPU need. Why: Settings showed packs and engines as two unrelated lists. https://www.refactoringui.com/previews/labels-are-a-last-resort
- Each card has ONE action that runs `mv_studio::voice_setup` (Python if needed, the engine, the voice files, a load check): "Set up Kokoro · {size}", "Finish setup" when files are there without the engine, or Remove voices when ready. Only the most useful one on the page is Primary (the narrator's engine, else a half-installed one, else Kokoro). Why: "I downloaded the models and it still says download the engine"; one primary button per view. https://www.nngroup.com/articles/visibility-system-status/ and https://m3.material.io/components/all-buttons
- Setup shows the kit `stepper` (Python, Engine, Voices, Check), an animated `progress_bar`, the live step text with download speed, and Cancel (or Pause for a Studio download) with a tooltip saying what happens. Why: long waits need moving, specific progress. https://www.nngroup.com/articles/progress-indicators/
- Setup failures stay on the card as an error `inline_alert` with Try again; success is a toast. Why: problems persist until fixed, success is brief. https://www.nngroup.com/articles/error-message-guidelines/
- Kokoro offers Standard / Lite as a small `segmented` control with sizes, and a Lite install offers "Get full quality". Why: let slower PCs choose the smaller pack. https://m3.material.io/components/segmented-buttons/overview
- Chatterbox card shows this PC's graphics card and memory (success tag at 4 GB or more, warning tag otherwise), the measured speed from the Voice Lab or the RTX 3080 figure, and a ~4 GB VRAM warning. Why: the engine needs a GPU and runs slower than real time. https://github.com/resemble-ai/chatterbox
- Remove voices is a DangerGhost button with a tooltip and a two-step `confirm_inline` naming the size ("Remove {size} of Kokoro voices?"). Why: costly deletes name the item and the consequence. https://www.nngroup.com/articles/confirmation-dialog/
- Voices: grouped by engine in a two-column grid, filter chips All / English / Hindi / Ready to use with counts. Why: filter a long voice list. https://m3.material.io/components/chips/guidelines
- Each voice tile: play icon button (spinner while rendering, Stop while playing, disabled with a reason), short name, EN/HI tag, and a status line that is either "Ready · warm female" or a small button that fixes it (Download · {size} for a Piper voice, Set up Chatterbox for a voice a stand-in speaks for); never "Install its engine above". Why: status must be accurate and actionable. https://www.nngroup.com/articles/visibility-system-status/
- The default narrator is the kit `icon_toggle` star on every tile: filled `Tokens::favorite()` gold with "Default" when on, an outline that previews gold on hover with "Make this the default narrator" when off; the default tile gets a gold edge. Why: the star stayed an outline and was green; filled marks selection. https://fluent2.microsoft.design/iconography
- No ready voices shows an `empty_view` with "Set up …" and "Show all voices". Why: empty states give the direct path to the task. https://www.nngroup.com/articles/empty-state-interface-design/
- Voice Lab: voice picker (expressive voices first), scene kind filter chips with glyphs (Action, Fight, Emotional, Reveal…), and three play buttons for intensity 2 / 5 / 8 (Gentle / Building / Intense) that render all three takes with one model load via `tts::voice_lab`, then show each take's parameters (exaggeration, cfg weight, temperature…) and speed, and a note when a stand-in spoke. Why: hear how a scene changes the delivery. https://elevenlabs.io/blog/v3-audiotags
- Voice Lab warns that Chatterbox uses about 4 GB of graphics memory and the first take loads the model (about 25 s); for Kokoro/Piper it says they follow pace, not emotion. Why: set expectations before a long wait. https://www.nngroup.com/articles/response-times-3-important-limits/
- Advanced (a kit `disclosure`): Python with a Found tag, or "Python not found" with a python.org external link and a copyable install command; studio-grade engines; Check again with a spinner. Why: plumbing behind progressive disclosure; commands must be copyable. https://www.nngroup.com/articles/progressive-disclosure/
- Settings and Studio's Setup sheet adopt each other's newer engine check (`setup::EnvSync`) and recheck after a voice change. Why: two pages disagreed on whether voices were installed. https://www.nngroup.com/articles/visibility-system-status/

## Storage

- Order is now: space used, download folders, cache, data folder, then one danger zone at the bottom. Why: everyday settings sat below seven Delete buttons. https://docs.github.com/en/repositories/creating-and-managing-repositories/deleting-a-repository
- "Space used" is the kit stacked `usage_bar` (Manga, Anime & movies, Studio, Models & voices, Chats, Cache, Other in hue tokens) with an icon legend, the total, a measuring spinner and free space on the drive; skeleton bar and legend while measuring. Why: show where the space goes at a glance. https://support.apple.com/en-us/102624 and https://www.nngroup.com/articles/skeleton-screens/
- Folder rows: path in a middle-truncated monospace chip with copy, icon buttons "Show in Explorer", "Change folder…" and "Use the default folder" (only when custom). Why: the path lost its meaningful end; one verb everywhere. https://www.nngroup.com/articles/icon-usability/
- Cache: Show in Explorer and "Clear cache" (broom), disabled with a reason when already empty. Why: disabled controls explain themselves. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Data folder: Move… (folder-plus), Go back to the default (only when custom), a panel that explains the choice (move my data / use the data there / replace / start empty) and a copy progress bar. Why: say what each option does before it runs. https://www.nngroup.com/articles/confirmation-dialog/
- Danger zone: danger-edged surface with a filled warning glyph; each part is a DangerGhost Delete (trash) with its size, disabled at 0 B, and a `confirm_inline` that names the size ("Delete {size}?") with Keep / Delete; Factory reset is the only Danger button and opens a `confirm_dialog` ("Erase everything" / "Keep my data", Enter does not confirm). Why: destructive actions grouped, named and confirmed. https://www.nngroup.com/articles/confirmation-dialog/ and https://developer.apple.com/design/human-interface-guidelines/alerts
- "Local AI models & voices" says it also removes the voice engines and the private Python and package cache that voice setup installed, and that voices need setting up again in Audio. Why: Delete models now removes `engines/`, `uv-python` and `uv-cache`. https://www.nngroup.com/articles/error-message-guidelines/
- Work waiting for a restart is a warning `inline_alert` listing it, with Cancel these changes and Restart now (arrow-clockwise). Why: system status with the next step. https://www.nngroup.com/articles/visibility-system-status/

## System

- Resource limit row has the gauge glyph, a slider with a live % value and a detail line in threads and memory; release saves and toasts. Why: icon plus words; immediate effect. https://www.nngroup.com/articles/toggle-switch-guidelines/
- CPU and memory rows use CPU / memory glyphs and the kit `usage_bar` (warning colour above the limit); four skeleton rows while the first reading is taken instead of "Measuring…". Why: skeletons show the coming layout. https://www.nngroup.com/articles/skeleton-screens/

## CLI tools

- Each CLI row has its provider logo, version text, an "Update" button with an arrow-up glyph that spins and blocks clicks while running, and Primary only when an update exists; the redundant "Update available" pill is gone. Why: one signal per state; no double clicks while busy. https://www.nngroup.com/articles/visibility-system-status/
- The last result is a status tag (Updated / Update failed) and the raw output sits behind a "Show log" `disclosure` in a 160px scrolling monospace box. Why: concise errors, details on request. https://www.nngroup.com/articles/error-message-guidelines/
- Update banners on AI pages are dismissible `inline_alert`s with a View action. Why: one alert component with a close button. https://fluent2.microsoft.design/components/web/react/core/messagebar/usage
- Check now has a refresh glyph and spins while checking. Why: a looped indicator for short waits. https://www.nngroup.com/articles/progress-indicators/

## About

- The real MangaVox mark (`brand/mangavox.svg`) on an accent-soft tile with a soft accent glow replaces the "MV" text. Why: use the app's own icon. https://learn.microsoft.com/en-us/windows/apps/design/style/iconography/app-icon-design
- Data folder and config file are `path_row`s with Show in Explorer icon buttons; Source code (GitHub logo) and Report a problem (bug) have "View source" / "Report" buttons with a trailing arrow-up-right. Why: icons with labels; external links say so. https://www.nngroup.com/articles/icon-usability/

## Model Library

- Sub-navigation is four primary pages in a `segmented` control with icons (In use, On this PC · n, Get models, Engines); the categories became filter chips with their icon and count under On this PC. Why: 11 wrapping text pills; "Downloaded" vs "Downloads" were confusing. https://lawsofux.com/hicks-law/
- Each job card has a hue icon tile, the picker, and a status pill with an icon and words (Uses / Saved / MangaVox picks / Cloud / Not on this PC / None) instead of a colour dot. Why: never colour alone. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Every voice route goes to Settings → Audio ("Manage voices", waveform glyph): the Voice job's get link, the Python engine action, the Voice empty state and the Get models banner; the scan text says "Download narration voices in Settings → Audio" instead of "Studio installs voices the first time you narrate". Why: one task, one home. https://lawsofux.com/jakobs-law/
- Kokoro Lite (`kokoro-v1.0.int8.onnx`) is recognised as "Kokoro v1.0 Lite voices" under the same model id, and Chatterbox weights make its voices pickable for the Voice job. Why: Lite users saw "Nothing on this PC" while their voices worked. https://www.nngroup.com/articles/visibility-system-status/
- Loading shows skeleton job cards and model rows; each category's empty state has its icon and a next step (Get a chat model, Manage voices, Get Whisper, Get ComfyUI). Why: a blank page with one line of text. https://www.nngroup.com/articles/empty-state-interface-design/
- Rescan is a ghost button with a leading refresh icon that spins while scanning; Start / Get X (trailing arrow-up-right) / Use my own… (key) / Use for chat (check) have icons; Show in Explorer is a tooltipped icon button. Why: icons lead; icon-only buttons need a label. https://www.nngroup.com/articles/icon-usability/
- Engine cards show the vendor mark (Ollama, LM Studio) or a hue glyph tile and a status pill with an icon. Why: recognisable engines; status in words and shape. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color

## Skipped audit items

- Theme switch/slider colours (main.rs:90, high): `main.rs` is the shell's; the kit `switch` already passes `Tokens::accent()` and `theme::init` maps the gpui-component theme.
- Voice library search, gender and accent filters (audio.rs:240, medium, part): the catalogue's style text is free-form ("warm female"), not structured gender/accent data; language and readiness chips are done.
- "Emotion-aware narration" switch and Expressiveness slider (audio.rs:230, medium, part): Chatterbox follows each scene automatically; the per-chapter Emotion strength slider belongs in Studio (`voices::set_emotion_strength`), not a global setting. The Voice Lab lets people hear it.
- Equaliser bars on the playing voice tile (audio.rs:432, medium, part): the play button's selected state and Stop glyph mark the playing tile; an animated equaliser needs a kit component.
- Settings search jumping to and highlighting a matching row (lib.rs:659, medium, part): the rail filters pages by label, description and keywords; row-level indexing needs every row registered in one place.
- Setup assistant row that reruns first-run onboarding (tabs.rs:326, low, part): there is no onboarding route yet (`Route` has no Welcome); add the row when it exists.
- Providers, Sources and Addons panels (providers.rs, sources.rs, addons_panel.rs): owned by ui-assistant, ui-manga and ui-media.
- mv-ui kit items (components.rs:95 and the shared-needs list): delivered by the kit.

## Review fixes (adversarial pass)

- General: name and email also autosave 1.2 s after typing stops (a half-typed email is skipped quietly; its error still shows on leave or Enter). Why: kit controls, the shell's sidebar included, do not take focus on click, so leaving Settings that way never blurred the field and the edit was silently lost. https://www.nngroup.com/articles/errors-forms-design-guidelines/
- Studio: "Default voice" and the speed preview use the live default narrator (`voices::default_voice`, the one Audio shows and Studio narrates with), not the prefs copy read when Settings opened, which said "Not chosen yet" while Audio showed a narrator. Why: two pages must not disagree about one setting. https://www.nngroup.com/articles/visibility-system-status/
- Studio: saving speed or resolution no longer rewrites `voice` in `studio/prefs.json`. Why: it wrote back the voice read at open, undoing a narrator picked in Studio since then (prefs.json wins over the pipeline config). https://www.nngroup.com/articles/user-control-and-freedom/
- Audio: returning to the page rereads the default narrator and packs. Why: Studio's Setup sheet can change both while Settings stays open. https://www.nngroup.com/articles/visibility-system-status/
- Audio: while the engine check runs, disabled play, Preview, speed-preview and Voice Lab buttons say "Checking the voice engines…" instead of "Set up Kokoro first". Why: a disabled control's reason must match the real state. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Audio: an engine whose download was paused or cancelled shows "Resume · 120 MB of 340 MB" (play glyph) and says it continues where it stopped; a part-downloaded Piper voice does the same. Why: the Pause tooltip promised a resume that the page no longer offered (the old pack card had "Resume (x of y)"). https://www.nngroup.com/articles/progress-indicators/
- Audio: only the Piper voice being downloaded shows "Setting up…"; other Piper voices keep their own Download button. Why: one Piper voice's download marked every Piper voice as installing. https://www.nngroup.com/articles/visibility-system-status/
- Audio: a default narrator from an engine MangaVox cannot set up (F5-TTS, XTTS, Parler) and with no stand-in gets a warning alert that says why and what to do, not just a "Can't speak yet" pill. Why: errors must explain and offer the way forward. https://www.nngroup.com/articles/error-message-guidelines/
- Model Library: CLI update banners now show above the library (it is an AI page; the branch that renders the library skipped them). Why: the banner promised "AI pages" and missed one. https://fluent2.microsoft.design/components/web/react/core/messagebar/usage
- Rail search: Enter with nothing typed stays on the current page (it jumped to General). Why: Enter acts on the matches, and an empty field has none. https://www.nngroup.com/articles/search-visible-and-simple/
- Not fixed here (kit or other crates): `inline_alert` gives every alert the same action id ("mv-alert-action") under a title-derived id, so two alerts with the same title in one parent would share state; Settings never renders two with one title.

## Review fixes (second adversarial pass)

- Audio and Studio: the page ticker runs one more time after the page goes quiet, so a sample that ends (or a Stop the player finishes after the click) repaints Play instead of leaving a stale Stop and selected state. Why: status must match the real state. https://www.nngroup.com/articles/visibility-system-status/
- Audio: a "Setup failed" engine card's error alert has a Dismiss button (it clears the failed job). Why: a failed extra, such as Kokoro's full pack over a working Lite one, hid the engine's Ready status and its actions for the rest of the session. https://fluent2.microsoft.design/components/web/react/core/messagebar/usage
- Audio: while the narrator's engine is being set up, the narrator alert says so and drops its "Set up …" action. Why: a second press did nothing, and the alert should report progress. https://www.nngroup.com/articles/visibility-system-status/
- Audio: a downloaded Piper voice tile has a Remove icon button (danger hover, tooltip with the size). It confirms in place with "Remove {size}?" and Keep / Remove. Why: the old pack cards could remove one Piper voice, and the engine card's Remove takes them all. https://www.nngroup.com/articles/confirmation-dialog/
- Audio: with both Kokoro packs on disk, the card offers "Remove Lite copy · {size}" (DangerGhost). Why: the Lite copy is never used once the full pack is there, and the old Lite pack card could remove it. https://www.nngroup.com/articles/user-control-and-freedom/
- Audio: removal sizes count a file that two packs share only once, and the size shown is what deleting actually frees. Why: "Remove … of Kokoro voices" counted `voices-v1.0.bin` twice. https://www.nngroup.com/articles/error-message-guidelines/
- Audio: only the newest sample request plays and clears its spinner. A Voice Lab take that finishes after you picked another voice or scene no longer plays. Why: pressing a second voice while the first rendered played the first voice and dropped the second's spinner. https://www.nngroup.com/articles/response-times-3-important-limits/
- General: the profile is not written until the config has been read. Why: leaving General, or the view closing, in the first moments after Settings opened could save the empty fields as "User" over the real name. https://www.nngroup.com/articles/user-control-and-freedom/
- General: a valid edit is saved when the Settings view is closed (the palette opening another Settings page replaces the view), and leaving General with an invalid email shows a warning toast. Why: both cases lost the edit silently, and the field error was no longer on screen. https://www.nngroup.com/articles/errors-forms-design-guidelines/
- Model Library: returning to the page rereads what each job uses (no disk rescan). Why: the Voice card kept showing the old narrator after the star in Audio changed it. https://www.nngroup.com/articles/visibility-system-status/
- Model Library: the job status glyph is filled only when something is chosen; "Not chosen yet" and "None" use the bold outline. Why: filled marks a chosen state. https://fluent2.microsoft.design/iconography
- Rail: the CLI tools update dot has a tooltip ("A CLI update is available"). Why: the dot was colour and shape only. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Not fixed here (other crates): the shell builds a new SettingsView when the palette or a link opens a different Settings page while Settings is showing. Page state such as the Voice Lab takes, the Kokoro Standard/Lite pick and an open confirmation resets. Running setups survive because they live in `voice_setup`. The fix belongs in `app/src/shell.rs`: route to the live view with `select_tab`.
- Left as is: when several CLIs have updates, each gets a Primary "Update" button. This breaks the one-primary-button rule, but only in that rare state, and it keeps each pending update visible.
