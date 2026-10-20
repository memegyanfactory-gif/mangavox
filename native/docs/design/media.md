# Movies and Anime (ui-media) design changes

One line per change: what, why, reference. Code: `native/crates/ui-media/src/*.rs`. Audit: `audit-media.json` (items whose file is under ui-media).

## Shared look (look.rs, common.rs)

- Every rating is `mv_ui::rating_badge` (filled gold star + value) through `look::score`, with a muted "IMDb" source label on Movies; the outline orange star, the bare number and the raw `#f5c518` "IMDb 7.8" text are gone. Why: one rating style; filled glyphs mark emphasis/state. https://developer.apple.com/design/human-interface-guidelines/sf-symbols
- Card hover groups are keyed by the card's unique id (`group_of(prefix, (row, index))`), never by title, so the same show in two rows no longer lights up twice. Why: a state layer applies only to the element under the pointer. https://m3.material.io/foundations/interaction/states/applying-states
- Row cards sit in their own id scope (`row-{id}-cards`), so a card keyed `("airing", 0)` no longer shares hover, focus-ring and tooltip state with the row's arrow and page dash that used the same key. Why: one focus ring per focused element. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible
- Hover promises match the click: `Hover::Play` (filled play disc) only where the click plays; poster cards say "Details" with an info glyph because they open the detail page; plain cards show just the outline. Why: Jakob's law, a play glyph means play. https://lawsofux.com/jakobs-law/
- Primary hero action is one `look::hero_play` (kit Primary, large, filled `ph::PLAY`), secondary `hero_more` ("More info"); the square uppercase Anime buttons and rounded Movies buttons became one system. Why: one source of truth for components and weight on the main action. https://fluent2.microsoft.design/iconography
- My List button `look::list_button`: "Add to My List" (outline bookmark) / "In My List" (tonal accent container, filled bookmark), spinner while saving, tooltip says what a click does. Why: Material 3 toggle buttons show a selected container and selected icon. https://material-web.dev/components/icon-button/
- `look::resume_note`: a thin progress bar and "14m left" under the hero's play button when resuming. Why: show where the user is and what happens next. https://www.nngroup.com/articles/visibility-system-status/
- Artwork (`pic`, `pic_small`) shimmers while loading and shows a duotone broken-image glyph on `surface_2` when it fails; the backdrop stays transparent while loading so slides crossfade. Why: broken art must not look like art that is still loading. https://developer.apple.com/design/human-interface-guidelines/loading
- All loading placeholders are kit shimmer skeletons in the real shapes (rows, poster grids, hero title/synopsis, episode cards and rows) on `theme::radius` (XS/SM/MD). Why: skeleton screens reduce perceived wait. https://www.nngroup.com/articles/skeleton-screens/
- Billboard crossfade: the new slide fades in over the old one (`duration::SLOW`, enter easing), instant under reduced motion. Why: no flash to black between slides; motion respects the setting. https://developer.apple.com/design/human-interface-guidelines/motion
- Carousel `look::rotate` skips while the hero is hovered, paused, an overlay or grid is open, or motion is reduced; a glass pause/play button sits by the dots. Why: auto-advancing content over 5 s needs a way to pause. https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide
- Hero dots and row page dashes are 4px (3px) bars inside 24px hit areas, brighter on hover, with "Slide 3 of 6" / "Page 2 of 4" tooltips. Why: Fitts's law, small visuals inside larger targets. https://lawsofux.com/fittss-law/
- Rows get a "See all" ghost button with a caret that opens the whole catalog as a grid with "Load more" (and Back to Home restoring the scroll position). Why: capped lists without a way forward are dead ends. https://www.nngroup.com/articles/infinite-scrolling-tips/
- Row arrows are Tab stops that show themselves on focus (not only on hover), with tooltips; every card is a Tab stop with the kit focus ring and an accessible label. Why: WCAG 2.4.7 focus visible. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible
- Top 10 rank numerals are outlined (eight `text_3` offsets under a page-coloured fill) instead of flat `#4b5159`; the card has the white hover outline and a title tooltip. Why: 3:1 for meaningful graphics, as Netflix's outlined numerals. https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast
- Metadata tags over art (`meta_tag`, `quality_tag`) are kit tags on glass with a glass border. Why: non-text contrast on any artwork. https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast
- `look::glass_button`: round glass icon button for use over artwork, tooltip required, bold glyph at 28px and below. Why: icon-only controls need labels; heavier strokes stay legible small. https://www.nngroup.com/articles/icon-usability/
- Source picker overlay closes on scrim click, Escape and a header X ("Close", Esc chip), and takes focus when it opens so Escape reaches it. Why: scrim and Escape are the default modal dismissals. https://m2.material.io/components/dialogs
- Search fields: leading magnifier, clear X, placeholders "Search anime" / "Search movies and series", live results after a 300 ms pause (Enter still searches at once). Why: Material 3 search bar anatomy; type-ahead. https://m3.material.io/components/search/guidelines
- Colours are `Tokens` only (page accent = the preset accent; `netflix()`/`crunchy()` removed), type from `ramp`, sizes from `theme::icon`, radii from `theme::radius`, motion from `theme::duration`; `tests/no_raw_colors.rs` now guards the crate (raw colours, Lucide paths, flag CDN). Why: one source of truth for colour, type, shape and motion. https://m3.material.io/foundations/design-tokens/overview

## Movies home (movies_home.rs)

- Continue Watching resumes playback on click (movies, and series episodes whose season is known), with a glass info button (top-right) for the detail page; the subline reads "E5 · 12m left". Why: users expect Continue Watching to resume (Netflix, Apple TV, Plex). https://lawsofux.com/jakobs-law/
- Hero Play says "Resume" when there is a resume point and continues a series at the saved season/episode. Why: visibility of system status. https://www.nngroup.com/articles/visibility-system-status/
- A failed or empty search are different states: no matches is `empty_view` ("No matches for "q"", spelling hint), a failure is `error_view` with Retry. Why: describe the actual problem. https://www.nngroup.com/articles/error-message-guidelines/
- Search results and See all grids are responsive landscape cards; "Back to Home" (arrow) replaces the separate Clear chip that duplicated the input's X. Why: one control per action (heuristic 8). https://www.nngroup.com/articles/ten-usability-heuristics/
- All rows failing shows one `error_view` ("Movies could not be loaded", Retry) with the raw error behind a Details disclosure. Why: plain language first, technical detail on request. https://www.nngroup.com/articles/error-message-guidelines/
- "My List" row with See all to the My List page. Why: same term and destination everywhere (heuristic 4). https://www.nngroup.com/articles/ten-usability-heuristics/

## Anime home (anime_home.rs)

- Continue Watching plays the saved episode at once (overlay spinner while links are found) with a glass info button for details; the hero says "Resume E7" / "Start watching E1". Why: resume costs one click, as in Crunchyroll. https://lawsofux.com/jakobs-law/
- Rows have See all grids that page through the whole Jikan feed with Load more and an inline error with Try again. Why: explicit path to the full collection. https://www.nngroup.com/articles/infinite-scrolling-tips/
- Search, empty and error states as on Movies (magnifier, debounce, no-matches vs failure, Back to Home). Why: as above. https://m3.material.io/components/search/guidelines
- "E12 SOON" corner badge on posters with a known next episode is a kit accent badge. Why: tokens and one badge style. https://m3.material.io/foundations/design-tokens/overview

## Anime detail (anime_detail.rs)

- The hero CTA follows history: "Resume E7" with progress and time left, "Continue E8" after a finished one, "Start watching E1" otherwise ("Watch now" / "Watch again" for movies). Why: Netflix and Crunchyroll heroes switch to Resume/Continue. https://www.nngroup.com/articles/visibility-system-status/
- Clicking an episode thumbnail plays it (the hover shows a filled play disc); a glass list button ("Choose source") opens its links under the row, and the inline panel has its own close X. Why: a play glyph on an episode tile means play. https://lawsofux.com/jakobs-law/
- Episode cards: "Watched" glass tag plus dimmed art and an accent bar for finished episodes, a progress line on the one in progress. Why: show progress where the user picks. https://www.nngroup.com/articles/visibility-system-status/
- Synopsis toggle is a ghost button with a caret ("More details" / "Show less") that expands the synopsis in place in the hero (scrolls inside a 200px cap). Why: feedback must be visible next to the action. https://www.nngroup.com/articles/visibility-system-status/
- Facts row (Studio, Next episode, Year, Voices) has bold 12px glyphs; "Episodes" header has an icon and count. Why: icons help scanning labelled groups. https://www.nngroup.com/articles/icon-usability/
- Episode sort menu (Oldest first / Newest first; Newest needs the whole list so it is disabled until then). Why: Crunchyroll's sort toggle for long shows. https://www.nngroup.com/articles/infinite-scrolling-tips/
- New: episode range picker ("Episodes 101–200") for shows over 100 episodes, jumping straight to a Jikan page (cached pages open at once); Load more still appends; the hero keeps working from the resume point when a later range is shown. Why: Crunchyroll-style ranges, so episode 900 is one click away instead of nine. https://www.nngroup.com/articles/infinite-scrolling-tips/
- "Load more episodes" is a Secondary button with a caret and a spinner; a failed page keeps what is shown and says so in an error toast. Why: say what failed, keep the user's place. https://www.nngroup.com/articles/error-message-guidelines/
- Quality tags in the hero come from the links that were found ("4K", "1080p", "HDR"), never assumed. Why: don't display claims the system can't back. https://www.nngroup.com/articles/ten-usability-heuristics/
- My List toggle with "Added to My List" / "Removed from My List" toast and Undo (works after leaving the page). Why: Material 3 snackbar, one lightweight Undo. https://m3.material.io/components/snackbar/guidelines
- Playing a listed title moves it from Planned to Watching; new: finishing it (a movie watched to the end, or a finished-airing series with every episode watched) moves it to Completed once, with a toast and Undo. Why: My List statuses fill themselves in; automatic changes stay reversible. https://m3.material.io/components/snackbar/guidelines
- Load failure in the hero is a compact `error_view` with Retry instead of raw text. Why: constructive recovery. https://www.nngroup.com/articles/error-message-guidelines/

## Movie and series detail (movie_detail.rs)

- Hero CTA: "Resume" / "Resume S2 E4" with progress, "Continue S2 E5", "Play S1 E1", "Watch again". Why: as on anime. https://www.nngroup.com/articles/visibility-system-status/
- The invented "HD" tag is gone; quality tags come from the found links. Why: match reality. https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast
- Episode rows play on click; a "Choose source" icon button opens the links right under the row (no jump to the top); the selected row has a 3px accent bar and the active fill, distinct from hover. Why: selected must differ from hover and persist. https://m3.material.io/foundations/interaction/states/applying-states
- Watched marks, dimmed thumbnails and a progress line on the episode in progress (one-season shows; the history table has no season). Why: progress where the user picks. https://www.nngroup.com/articles/visibility-system-status/
- Seasons: a segmented control for 2 to 5 seasons, a menu for more ("Specials" for season 0). Why: Material 3 segmented buttons for few options; Netflix's season menu for many. https://m3.material.io/components/segmented-buttons/guidelines
- About block labels (Cast, Genres, Director) have glyphs; "Episodes" and "More Like This" headers have icons; More Like This uses poster cards with hover synopsis. Why: scanning; consistent card family. https://www.nngroup.com/articles/icon-usability/
- My List toggle and Undo toast as on anime; new: a movie watched to the end, or an ended one-season series with all episodes watched, moves to Completed once (toast with Undo). Why: as above. https://m3.material.io/components/snackbar/guidelines

## Sources (streams.rs)

- Searching state: kit spinner, three shimmer rows and a Cancel button; dropping the lookup task cancels it, so a stale lookup never opens the player. Why: show progress, allow cancel. https://developer.apple.com/design/human-interface-guidelines/loading
- Failures in words the user can act on: no addons (Open addons), no sources (Manage addons, Try again), no IMDb/Kitsu mapping, and unknown errors behind a Details disclosure with Try again. Why: NN/g error message guidelines. https://www.nngroup.com/articles/error-message-guidelines/
- Source rows: whole row plays (filled play glyph that lights on hover), "Best match" on the tonal accent container with a filled seal, quality tag, size, source and languages. Why: affordance matches the click; selection unambiguous. https://m3.material.io/foundations/interaction/states/applying-states
- No remote flag images: languages are named ("English, Japanese +2") on a translate-glyph tag with the full list in a tooltip; flagcdn.com is never contacted. Why: offline-safe and no third-party request per row; language is not a country. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Seeder health has three shapes and words (users glyph; "slow"; warning glyph "few") plus a tooltip, not colour alone. Why: WCAG 1.4.1 use of colour. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Filter chips (All, per addon, per language) are kit filter chips with counts and leading glyphs. Why: Material 3 filter chips. https://m3.material.io/components/chips/guidelines
- New: each row shows its addon's own logo (16px, manifest icon matched by name prefix) before the addon name when one is installed. Why: recognition over recall; the addon is recognisable at a glance. https://www.nngroup.com/articles/recognition-and-recall/
- The best link is prefetched (torrent warmed) so Play starts almost at once; a series' season/episode is remembered for Resume. Why: keep the wait short. https://developer.apple.com/design/human-interface-guidelines/loading

## History (history.rs)

- Page title "History" with the history glyph, count subtitle and a Secondary "Clear history" button (trash) that is disabled with a reason when empty. Why: kit page header; disabled controls explain why. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Clear asks first ("Clear 42 titles from your anime history?", "Clear history" / "Keep history"). Why: confirm only high-consequence, irreversible actions. https://www.nngroup.com/articles/confirmation-dialog/
- Per-title remove: a glass X on the card's corner (hover, Tab, or Delete on a focused card), then "Removed X from history" with Undo; the delete commits after 10 s. Why: Material 3 snackbar Undo for reversible actions. https://m3.material.io/components/snackbar/guidelines
- Grouped by Today / Yesterday / This week / Earlier with section headers and counts. Why: Netflix Viewing activity and YouTube History group by date. https://www.nngroup.com/articles/ten-usability-heuristics/
- Kind filter is a segmented control with glyphs (All, Anime, Movies, Series). Why: one of a few options. https://m3.material.io/components/segmented-buttons/guidelines
- Cards are kit `cover_tile`s with a progress bar and "E3 · 12m left" / "Watched". Why: progress as a bar, not "45%" text. https://m3.material.io/foundations/interaction/states/overview
- Empty state with icon and next steps (Browse anime / Browse movies); errors with Retry. Why: empty states teach and give a path. https://www.nngroup.com/articles/empty-state-interface-design/

## My List (history.rs, Favorites)

- One name and glyph: "My List" with the bookmark everywhere in this crate (page, buttons, toasts, empty state). Why: consistency and standards. https://www.nngroup.com/articles/ten-usability-heuristics/
- Status chips appear only for statuses that have titles (with counts and glyphs), so no filter leads to an empty page. Why: controls that can never produce results mislead. https://www.nngroup.com/articles/ten-usability-heuristics/
- Each card has a status tag (glyph + word + tone) and a "More actions" menu to set Watching / Planned / Completed / On hold / Dropped or remove; remove offers Undo. Why: a write path for every status; status never by colour alone. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Empty state with bookmark icon, "Choose Add to My List on any anime, movie or series" and Browse buttons. Why: NN/g empty states. https://www.nngroup.com/articles/empty-state-interface-design/

## Discover (discover.rs)

- Kind is a segmented control with glyphs; moods are filter chips with leading glyphs (mountains, lightning, smiley, mask, heart, star, brain, heartbeat, ghost, detective) and a check when selected. Why: Material 3 filter chips. https://m3.material.io/components/chips/guidelines
- Results are the shared poster cards (hover synopsis, rating badge, "Details") instead of small generic cover cards, so Discover matches the Movies and Anime pages. Why: consistency. https://www.nngroup.com/articles/ten-usability-heuristics/
- Skeleton poster grid while loading; empty and error states with icon and Retry. Why: as above. https://www.nngroup.com/articles/skeleton-screens/

## Addons panel (addons_panel.rs)

- Enable/disable is a kit switch with a tooltip, not a button whose label was the state. Why: on/off settings with immediate effect use a switch. https://www.nngroup.com/articles/toggle-switch-guidelines/
- Rows show the addon logo (fallback glyph), name, version tag, "Off" tag, a 2-line description, capability tags with glyphs (Streams, Catalogs, Subtitles) and the source URL (middle-ellipsis, tooltip). Why: recognition over recall. https://www.nngroup.com/articles/recognition-and-recall/
- Remove is a danger icon button ("Remove addon") that asks inline ("Remove Torrentio?" Remove / Keep), then a toast. Why: confirm in place for row-level deletes. https://www.nngroup.com/articles/confirmation-dialog/
- Install: link-glyph input with clear X, Primary "Install" with a spinner, disabled with a reason while the field is empty; errors stay under the field as a dismissible inline alert; success toast. Why: disabled-with-reason; problems that stay until fixed are inline. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Empty state explains what an addon is with a "Paste a manifest URL" action that focuses the field; skeleton rows while loading. Why: empty states give a direct path. https://www.nngroup.com/articles/empty-state-interface-design/

## Skipped audit items, and why

- Arrow-key navigation inside `look::row` (components.rs:43 fix, last part): only the visible page of a row is rendered, so arrow keys would have to page and move focus to a card that does not exist yet; Tab already reaches every visible card and the row arrows (which show on focus). Revisit with a virtualised row.
- My List "completed when the last episode ends" in the player (history.rs:113 fix, last part): the player is ui-player's; the detail pages now do it when they next load the saved progress, which covers the common path (finish, come back to the page).
- Movie detail sources in a sticky right-hand sheet (movie_detail.rs:319, "better" option): links now open right under the episode row, which removes the scroll jump without a second layout.
- Watched marks for multi-season series (movie_detail.rs:319): the history table stores the episode number without a season, so marks are shown for one-season shows only. Needs a season column in mv-core.
- Bundled flag SVGs (streams.rs:379 fix): replaced by language names on a translate glyph, as the brief asked (no remote flag images); no flag set needed.
- Addon remove with Undo instead of confirm: undoing an uninstall would need a network reinstall that can fail, so the inline confirm (as the audit suggests) stays.
- `PlayRequest.artwork` (ui-player view.rs:1049): mv-ui / ui-player own it; the player now fetches anime art by MAL id itself.

## Adversarial review (fixes)

- Source overlay scrim now occludes the page: before, cards under it kept their hover and the click that dismissed the scrim also opened or played the card beneath. Why: a modal's backdrop must block the page behind it. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- Closing the source overlay (X, Cancel, Escape, scrim) hands focus back to what had it before it opened. Why: the dialog pattern returns focus to the invoking element. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- Home top bars block hovers and clicks to the rows scrolled under them (the wheel still scrolls the page); a click on the opaque bar used to open the card beneath. Why: what looks on top must be on top. https://m3.material.io/components/top-app-bar/guidelines
- Carousel hover is read from the pointer against the hero's painted bounds instead of an `on_hover` flag: the homes are kept alive, and leaving one while the pointer was on the hero (every hero Play / More info click) left the carousel paused for good. Why: auto-advance must resume when the pause condition ends. https://www.w3.org/WAI/WCAG22/Understanding/pause-stop-hide
- Continue Watching (and the hero's Resume) rereads history whenever the player saved progress (`ui_player::watch_epoch`); the Movies "My List" row rereads when My List changed (`common::list_epoch`). On a kept-alive home a stale list made a Continue Watching click play an old episode. Why: show the current state of the system. https://www.nngroup.com/articles/visibility-system-status/
- Movies hero says "Resume" only when Play really resumes (a series needs its season); otherwise "Play" (it starts at S1 E1). Why: labels must match the action. https://lawsofux.com/jakobs-law/
- Movies Continue Watching subline reads "S2 E5 · 12m left" when the season is known. Why: as Netflix's resume line (audit item). https://www.nngroup.com/articles/visibility-system-status/
- Playing from an episode card or row shows the "Finding links" progress (and any failure with Try again) right under that row; the hero's Play reports in the hero. Before, a click far down the list reported into the hero, off screen. Why: feedback next to the action. https://www.nngroup.com/articles/visibility-system-status/
- Anime episode "Choose source" button has an on state (accent disc, fill glyph, toggled role) while its links are open (`look::glass_toggle`). Why: Material 3 toggle icon buttons fill when selected. https://m3.material.io/components/icon-buttons/guidelines
- Expanded anime synopsis scrolls in 120px (was 200px) and the title keeps one line while open: the bottom-anchored hero copy pushed a two-line title out of the 480px banner. Why: expanding content must not hide the heading. https://www.nngroup.com/articles/visibility-system-status/
- Row cards keep their 2px focus ring: the row's clip cut it off above and below every card (4px inner padding, cancelled by a negative margin). Why: WCAG 2.4.7 focus visible. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible
- Movie detail: Specials (season 0) no longer get the regular season's Watched marks, and a one-season show with Specials resumes (the season fallback ignored such shows). Why: progress marks must be true. https://www.nngroup.com/articles/visibility-system-status/
- My List status change that fails to save shows an error toast and reloads the list (it went to Loading and stayed there). Why: say what failed, keep the page usable. https://www.nngroup.com/articles/error-message-guidelines/
- History Undo after the kind filter changed no longer drops a movie into the Anime list. Why: the list must match its filter. https://www.nngroup.com/articles/ten-usability-heuristics/
- Source failures: "Open addons" closes the home overlay first (the homes are kept alive, so it was still open on return), and is Secondary on detail pages, where Play is the one primary. Why: one primary action per view. https://m3.material.io/components/buttons/guidelines
- Detail heroes: while the sources panel is docked on the right, the copy is capped at the width left beside it (`look::beside_panel`, 32px gap) instead of half the page, which ran under the 430px panel at the default window size. Why: content must not sit under another layer. https://m3.material.io/foundations/layout/understanding-layout/spacing
- The auto-Completed marker is touched when found, so the data cache's 30-day pruning does not forget it. Why: an automatic change happens once. https://m3.material.io/components/snackbar/guidelines

### Skipped in this review, and why

- Focus trap inside the source overlay (Tab can still reach the page behind it; the mouse cannot): GPUI has no scoped tab cycle for a subtree, and the overlay holds three controls at most. Revisit with a kit dialog primitive.
- Anime home hero "Continue E8" after a finished episode: the home only reads unfinished history, and an airing show caught up has no next episode yet; the detail page already shows Continue / Watch again correctly.
- A lookup started from a home and left running while the user navigates via the sidebar can still open the player when it finishes (kept-alive page, as before this work). Needs a "page hidden" signal from the shell.

## Second adversarial review (fixes)

- Undo after "Removed from My List" puts the title back with the status it had (Watching, Completed...); it came back as Planned. `set_watchlist` returns the entry it replaced, adding an already listed title keeps its status, and both detail pages share `common::announce_list_toggle`. Why: Undo must restore the previous state, not an approximation. https://m3.material.io/components/snackbar/guidelines
- Adding a show the user finished long ago no longer flips it to Completed with a second toast a moment after "Added to My List": progress read while the title was unlisted counts as checked (`common::completion_due`), so only a later finish moves it. Why: automatic changes should follow the user's action, not contradict it. https://www.nngroup.com/articles/ten-usability-heuristics/
- History removals, Clear and Undo bump a history epoch (`common::progress_stamp`); the homes' Continue Watching and the detail pages' Resume / Watched marks reread on it. A cleared title stayed in Continue Watching on the kept-alive homes (and a click resumed it). Why: show the current state of the system. https://www.nngroup.com/articles/visibility-system-status/
- Detail pages reread "In My List" when My List changed (they are kept for Back): a title removed on My List still said "In My List" on return. Why: visibility of system status. https://www.nngroup.com/articles/visibility-system-status/
- My List rereads on return the same way (a title removed or auto-completed on its detail page kept its old card), keeping cards in their on-screen order so a status change does not move the card under the pointer. Why: avoid unexpected layout shifts. https://web.dev/articles/cls
- History rereads after the player saved progress while it sat in the back stack, without bringing back entries still inside their Undo window. Why: as above. https://www.nngroup.com/articles/visibility-system-status/
- Movie detail opens the episode list on the season the hero continues in ("Resume S2 E4" sat over Season 1's list). Why: Netflix opens a series on the current season; the list should agree with the hero. https://www.nngroup.com/articles/ten-usability-heuristics/
- Anime detail with a later episode range on screen no longer offers "Continue E221" after the last of 220 episodes; it checks the show's total. Why: never offer an action the system cannot back. https://www.nngroup.com/articles/ten-usability-heuristics/
- Movies tabs (Home, Movies, Series) clear the search field and start the tab at its top: the field kept a query whose results were gone. Why: the input should match what is shown. https://m3.material.io/components/search/guidelines
- Source-row addon logos match whole words only, and only the link's own addon name: a link named "Stream" (the placeholder) or "HD" took the logo of "StreamFusion" / "HDHub". Why: recognition only helps when the mark is right. https://www.nngroup.com/articles/recognition-and-recall/
- `look::pic_small` takes the fallback glyph size, so a failed 16px source-row logo shows a 12px glyph instead of a clipped 20px one. Why: the broken-image state must read as one. https://developer.apple.com/design/human-interface-guidelines/loading

### Skipped in this review, and why

- Top 10 row paging uses one card width for every rank, but rank 10's two-digit numeral is about 60px wider, so at some window widths the 10th card can be clipped at the row's right edge. Needs the numeral SVGs (or a measured width) to fix without guessing; listed for the visual QA pass.
- Continue Watching's "See all" opens History on All rather than filtered to the page's kind: MediaHistory has no route parameter for the kind (mv-ui `Route`, not this crate).
