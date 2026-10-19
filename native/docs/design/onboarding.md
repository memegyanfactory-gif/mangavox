# Welcome setup and repositories design changes

One line per change: what, why, reference. Code: `native/crates/ui-welcome/src/{lib,plan,view,cards}.rs`, `native/crates/mv-core/src/onboarding.rs`, `native/crates/ui-manga/src/{sources,backend}.rs`, `native/app/src/{main,shell,routes}.rs`, `native/crates/ui-settings/src/tabs.rs` (About).

## When it shows

- Decided once per start in `main`, right after `paths::prepare()` and before WebView2, the database or any config write, and cached for the run (`onboarding::init`). Why: files the app creates later in the same start must not flip "fresh" to "existing". https://developer.apple.com/design/human-interface-guidelines/onboarding
- A data folder with any MangaVox data (database, settings, plugins, addons, repos.json, voice models, library files) is recorded as done (`existing: true`) and never sees the setup. Why: upgraders and the Electron-era shared folder must not be nagged. https://developer.apple.com/design/human-interface-guidelines/onboarding
- `MANGAVOX_WELCOME=1|0` and `--welcome` force or hide it; Settings › About › "Run welcome setup again" opens it any time. Why: testable without touching real data, and re-runnable like Jellyfin's setup wizard. https://jellyfin.org/docs/general/post-install/setup-wizard/
- Progress saved per confirmed screen in `config.json` → `onboarding` (`status`, `step`, `uses`, `steps`, `seeded`); a restart resumes at the same screen. Why: quitting mid-way must not lose choices or restart the flow. https://www.nngroup.com/articles/progressive-disclosure/

## Flow (6 screens at most)

- Screens: Welcome → What will you use MangaVox for? → Manga sources → Streaming addons → Narration voice → Done; a screen appears only when its use was picked (Studio also gets Manga sources, since recaps need chapters). Why: one decision per screen and no questions about features the user does not want. https://www.nngroup.com/articles/progressive-disclosure/
- Welcome screen: the MangaVox brand mark on an accent-soft tile with a soft glow, one line of copy, "Set up MangaVox" (primary, Enter) and "Start empty" (ghost), plus the data folder and its free space. Why: a single clear start, an equally visible way to skip, and where data lives (Plex/Obsidian ask this up front). https://developer.apple.com/design/human-interface-guidelines/onboarding
- Every middle screen has Back, "Skip this step" (Esc) and Continue (Enter, primary); the header always shows "Skip setup" and "Step N of M" with a dot trail. Why: a visible way out on every screen and a sense of length. https://www.nngroup.com/articles/wizards/
- "Start empty" and finishing with nothing picked leave the same state: every built-in source off (`disabledSources` = all 7 ids), no addons, no models, Studio off. Why: the user asked for a clean start when nothing is added (Mihon ships with no content). https://mihon.app/docs/guides/getting-started
- The clean start is written when "Set up MangaVox" is pressed, not at the end. Why: quitting part-way must also leave nothing switched on. https://mihon.app/docs/guides/getting-started
- "Skip setup" part-way keeps what was already set up and shows the summary. Why: skipping must never undo work the user just did. https://www.nngroup.com/articles/wizards/
- A repeated setup (from Settings) starts from the current settings, resets nothing, and "Close setup" leaves without changes. Why: re-running must be safe. https://developer.apple.com/design/human-interface-guidelines/onboarding
- Screens fade in (`appear`, instant under Reduce motion). Why: a short, calm transition; respects the motion setting. https://developer.apple.com/design/human-interface-guidelines/motion

## Choice cards

- Option cards: surface, 12px corners, 1px border; selected adds the accent border, accent-soft fill and the filled check badge; hover strengthens the border; Tab stop with focus ring. Why: one selected look across the app (same as the Audio voice tiles). https://m3.material.io/components/cards/guidelines
- "What will you use" cards are multi-select with a category-tinted icon tile that turns accent when picked; none are preselected. Why: no default the user did not choose. https://www.nngroup.com/articles/checkboxes-vs-radio-buttons/
- Manga sources: three cards (Built-in sites, Add a repository, Plugin link) pick which panel shows below; each card's line says what is set ("2 switched on", "3 extensions installed"). Why: three equal ways in, one visible at a time. https://www.nngroup.com/articles/progressive-disclosure/
- Built-in sites are switches, all off, with "Switch all on"; the 3 adult sources stay hidden behind an "I am 18 or older" checkbox and carry an 18+ tag. Why: explicit opt-in for adult content. https://www.nngroup.com/articles/toggle-switch-guidelines/
- Voices: one card per choice from `voices::PACKS` (Kokoro, Kokoro Lite, Piper English, Piper Hindi, Chatterbox) with quality and speed meters, languages and "voices + engine · total" sizes; Recommended tag (Lite on PCs with < 8 GB memory or < 2 GB free); Chatterbox tagged "Needs an NVIDIA GPU". Why: real sizes before any download (Steam and LM Studio show size and free space before installing). https://developer.apple.com/design/human-interface-guidelines/onboarding
- Nothing downloads until "Download and set up · <total size>" is pressed; the button is disabled with the shortfall when the drive is too full. Why: explicit consent with the size shown. https://developer.apple.com/design/human-interface-guidelines/onboarding
- The voice job (Python if missing, engine, voices, check) runs on the background runtime; the screen shows a progress bar with the step text and Cancel, Continue stays available, and a toast reports the end wherever the user is. Why: let people keep going while content downloads. https://developer.apple.com/design/human-interface-guidelines/onboarding
- Done screen: a summary row per area with a filled check (set up) or dashed circle (left for later) and a "Settings" link that finishes and opens that page; primary "Open Library / Anime / Studio" lands on the first chosen section. Why: say what happened and where to change it. https://www.nngroup.com/articles/wizards/

## Repositories and plugins (Settings › Sources and the setup)

- New `RepoBrowser`: paste a repository link (GitHub repo, index.min.json, or tachiyomi:// / mihon:// share link), Add verifies it lists extensions before saving to `repos.json`. Why: Mihon's "add repository" step; never save a link that does not work. https://mihon.app/docs/guides/getting-started
- Saved repositories are filter chips (owner/repo) with a remove button and an inline confirm that says installed extensions stay. Why: removal is not undoable, so confirm in place. https://www.nngroup.com/articles/confirmation-dialog/
- Extension list: search (name, package, site names), language menu sorted by count, "Show 18+" switch (off by default, "N 18+ hidden" count), 40 rows then "Show 40 more"; each row has the extension icon (letter fallback), language tag, 18+ tag, version and site count, and Install / Installed / Update (with the version change in its tooltip). Why: a 1,400-entry index needs search and filters; the state of each row must be visible at a glance. https://www.nngroup.com/articles/filters-vs-facets/
- Install picks the extension's site in the reading language (else its first) and uses the wall-aware plugin backend; an install whose first page came back empty shows a warning toast pointing to Test. Why: most Mihon sources are walled; a silent empty source would look broken. https://www.nngroup.com/articles/error-message-guidelines/
- `PluginLinkBox`: link field with Paste and Install, inline problems ("starts with https://"), and a result line that stays (success or the error) instead of a toast. Why: the result belongs next to the field it came from. https://www.nngroup.com/articles/errors-forms-design-guidelines/
- Trust note under both boxes ("third-party code, sandboxed: 128 MB, 2 minutes per request; only add ones you trust"). Why: say what runs and its limits. https://www.nngroup.com/articles/communicating-trustworthiness/

## App wiring

- `Route::Welcome` is a focus route: no sidebar, no dock, no search, no Back/Forward in the title bar, and app shortcuts are ignored; leaving it opens the target in its own section with a fresh history. Why: the setup must not be left half done by accident, and Back must not lead into it. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- `ui_media::warm_up()` (Anime/Movies prefetch) does not run while the setup is open, starts when the setup finishes with Anime/films chosen, and is skipped at later starts if they were left out. Why: no network for features the user did not ask for. https://developer.apple.com/design/human-interface-guidelines/onboarding
- Settings › About gets a "Setup" section with "Run welcome setup again". Why: every skipped item needs a way back. https://developer.apple.com/design/human-interface-guidelines/onboarding

## Skipped audit items (and why)

- AI provider screen (detected CLIs, API keys): left out to keep 6 screens; the Done screen links to Settings › AI providers for Studio users.
- Language choice on the "uses" screen: chapter language stays in Settings › Sources (default English); repository installs follow it.
- Moving the data folder from the welcome screen: shown read-only; moving needs the restart helper that lives in ui-settings (Settings › Storage).
- Title-bar chip for a running voice job and a "Finish voice download" banner at the next start: needs shell and mv-studio state the shell does not read yet; the job shows in Settings › Audio and ends with a toast.
- "I will install Python myself" choice: `voice_setup` already installs a private Python when none is found, so a second path was not needed.
- Shared pieces to promote to mv-ui: `option_card`, `icon_tile`, `summary_row` (ui-welcome/src/cards.rs) and `trust_note`, `result_line` (ui-manga/src/sources.rs).
