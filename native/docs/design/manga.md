# Manga area: design changes

Crate: `native/crates/ui-manga`. Source audit: `audit-manga.json`. Every change uses the mv-ui kit
(`COMPONENTS.md`): Phosphor icons (fill weight for on/selected, bold at 14px and below), `Tokens`
only (no raw colours left in the crate), `theme::{icon, radius}` scales, kit buttons, chips, menus,
empty/error states, skeletons, toasts and confirmations.

Format: what changed, why, reference.

## Shared building blocks (`ui.rs`)

- Cover card lifts 3px with a stronger border and shadow on hover, settles when pressed, is a Tab stop with the focus ring and a link role. Why: covers felt flat and keyboard focus was invisible. https://m3.material.io/components/cards/guidelines, https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- Cover card shows a filled gold favourite star (top right) and a reading-progress line along the bottom edge. Why: Library has a Favorites filter but covers never showed it; progress was invisible. https://lawsofux.com/aesthetic-usability-effect/
- Cover images shimmer while loading, fall back to a duotone broken-image glyph, and long clamped titles show in full in a tooltip. Why: blank boxes and cut titles. https://www.nngroup.com/articles/skeleton-screens/
- Cover badges use `on_accent` / `surface_3` instead of `Tokens::base` text and `rgb(0x3b4450)`; "In library" carries a filled bookmark and the download count a download glyph. Why: raw colours, and state shown by text colour only. https://fluent2.microsoft.design/iconography
- Grid and list skeletons use the kit's shimmering `skeleton`, shaped like the real card or row. Why: static grey boxes looked frozen. https://www.nngroup.com/articles/skeleton-screens/
- Underline tabs take an optional Phosphor icon (fill weight when selected), get the focus ring and the tab role. Why: text-only tabs scan slowly. https://www.nngroup.com/articles/icon-usability/
- `action_tile` (icon above label) shows an on state as the fill-weight icon in its colour, a tinted container and a semibold label, pops briefly on toggle (not under reduced motion) and can show a spinner while busy. Why: "active" was only a colour change on an outline glyph (fails WCAG 1.4.11). https://m3.material.io/components/icon-buttons/overview
- Search fields get a leading magnifier. Why: a bare field reads as any text input. https://www.nngroup.com/articles/icon-usability/
- New local scrolling dropdown (`MenuTrigger`, `Entry`, `fill_menu`) shares one item list between a dropdown and a right-click menu, with checked items, hints, danger rows and a max height. Why: the kit menu cannot scroll (chapter lists) or use a custom trigger (progress ring). https://m3.material.io/components/menus/guidelines
- Local helpers `spinner`, `state_view`, `error_view`, `note_line`, `select_chip`, `tool_button`, `icon_tip`, `action`, `tag`, `muted` are gone; pages use the kit equivalents. Why: two spinners, two empty states and three progress bars with different looks. https://www.nngroup.com/articles/skeleton-screens/
- `thumb()` row covers (History, Downloads) get the same loading and fallback states as cards. Why: rows showed an empty grey box. https://www.nngroup.com/articles/skeleton-screens/
- Every raw `rgb(0x..)`/`rgba(0x..)` in the crate became a token (`scrim`, `glass`, `glass_strong`, `glass_border`, `canvas`, `track`, `base().alpha(..)`). Why: presets could not reach these colours. https://fluent2.microsoft.design/color

## Library

- Page header is `page_title` with the Library glyph, subtitle and right-aligned actions. Why: the old header used margin hacks. https://www.nngroup.com/articles/visual-hierarchy-ux-definition/
- Sort is a menu: "Sort by" with the current mode checked, plus "Reverse order"; the button shows the current mode; both are remembered. Why: the button silently cycled through four modes. https://lawsofux.com/hicks-law/, https://m3.material.io/components/menus/guidelines
- New "Update" action checks the whole library (or the open category) for new chapters, three titles at a time, with a progress bar and a result toast ("12 new chapters in 4 titles", or a warning with Retry for titles whose source did not answer). Why: unread counts went stale. https://mihon.app/docs/faq/library
- Categories can now be created, renamed and deleted ("Manage categories" in the header menu and a pencil at the end of the category tabs). Why: `category::create` had no caller, so the tabs were unreachable. https://mihon.app/docs/guides/categories
- Filters are kit filter chips: a check when selected, an icon each (Favorites is a star), counts for Unread / Downloaded / Favorites, and clicking a selected chip returns to All. Why: the selected chip measured 1.24:1 with no check. https://m3.material.io/components/chips/guidelines
- Continue reading is a 30px filled play button at the cover's bottom right (the title keeps clear of it), revealed with the card on hover and shown when it has keyboard focus. Why: it only existed while the pointer hovered, and a per-hover re-render tracked it. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- Empty library: `empty_view` with a Browse sources primary button (leading compass). No matches: "Clear filters" resets chips, category and search. Why: dead ends. https://www.nngroup.com/articles/empty-state-interface-design/
- Load failure: `error_view` with Try again; "Could not open" errors are toasts instead of a note in the chip row. Why: notes never went away. https://m3.material.io/components/snackbar/overview

## Manga detail

- Favourite is a gold filled star tile ("Favorite" / "Favorited", `Tokens::favorite`), not an outline heart. Why: the user reported it; Mihon fills the active icon. https://github.com/mihonapp/mihon/blob/main/app/src/main/java/eu/kanade/presentation/manga/components/MangaInfoHeader.kt
- In library is a filled bookmark tile ("In library" / "Add to library") with tooltips that say what a click does. Why: bookmark-check vs bookmark-plus outlines looked the same. https://m3.material.io/components/icon-buttons/overview
- Favouriting a title that is not in the library also adds it, with a toast saying so. Why: two overlapping save concepts; a favourite outside the library was invisible. https://www.nngroup.com/articles/icon-usability/
- New Categories tile (when in library) opens a dialog with a checkbox per category and a field to create one; its label shows the category name or count. Adding to the library offers "Set categories" in the toast when categories exist. Why: categories could never be assigned. https://mihon.app/docs/guides/categories
- Refresh spins its own icon and reports the result in a toast ("3 new chapters" / "Up to date"), errors with Retry; the separate toolbar spinner is gone. Why: feedback away from where the user acted. https://www.nngroup.com/articles/visibility-system-status/
- Removing from the library shows a toast with Undo; all inline notes in the action row are toasts. Why: notes never dismissed and could not be undone. https://m3.material.io/components/snackbar/overview
- Chapter download indicator: arrow to download, flat ring while queued, filling ring while downloading, filled accent check when saved, red alert when it failed (tooltip has the error, click retries). The check and rings open a menu (Delete download / Cancel); none of them deletes on click. Why: the success icon doubled as delete. https://github.com/mihonapp/mihon/blob/main/app/src/main/java/eu/kanade/presentation/manga/components/ChapterDownloadIndicator.kt
- Delete download is optimistic with an Undo toast; files are removed only after the Undo window, and a late Undo downloads the chapter again. Why: undo beats surprise destruction. https://www.nngroup.com/articles/confirmation-dialog/
- Every chapter row has a "More actions" menu (revealed on hover) and the same menu on right-click: Mark as read / unread, Mark previous as read, Download / Cancel / Delete download. Why: catching up needed one hover and click per chapter. https://www.nngroup.com/articles/confirmation-dialog/
- Bulk marks (Mark all read/unread, Mark previous as read) show "Marked 37 chapters as read" with Undo that restores each chapter's read flag and saved page (`store::restore_read`). Why: "Mark all read" had no undo and unread reset the page. https://www.nngroup.com/articles/confirmation-dialog/
- Read rows keep their date/group line in `text_3` (4.5:1 or better) and get a small check before the title. Why: the old 2.58:1 meta text, and state shown by dimming alone. https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html, https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Chapter filters are filter chips; "Unread" became "Hide read" so the eye-slash icon matches the label. Why: icon and label disagreed. https://www.nngroup.com/articles/icon-usability/
- "Download all" is a Secondary button with a leading icon (the Resume button stays the one primary); "Unread (n)" gets a download icon; the downloading pill is a status tag with an icon Cancel; "All chapters downloaded" is a Done status tag. Why: two primaries, trailing icons via `flex_row_reverse`. https://m3.material.io/components/all-buttons
- Resume / Start is the kit's large primary button with a filled play icon. Why: hand-rolled pill with no focus or pressed state. https://m3.material.io/components/all-buttons
- "Chapters" heading gets a numbered-list icon; status, author, source and language lines get bold 14px icons. Why: missing icons from the audit. https://www.nngroup.com/articles/icon-usability/
- "You may also like" uses `section_header` with a sparkle, Previous / Next arrow buttons and a right-edge fade; a failure is an inline alert with Try again. Why: no cue that the row scrolls. https://www.nngroup.com/articles/designing-effective-carousels/
- The loading state mirrors the real page: 168x252 cover, title and meta bars, four action tiles, the description and six chapter rows; chapters being fetched show row skeletons. Why: content jumped when data arrived. https://www.nngroup.com/articles/skeleton-screens/
- Empty chapter states use `empty_view` with "Check again" / "Show all chapters". Why: no next step. https://www.nngroup.com/articles/empty-state-interface-design/
- The back pill sits on `Tokens::glass` (was `rgba(0x0000004d)`) and shows its Alt+Left shortcut. Why: raw colour, undiscoverable shortcut. https://developer.apple.com/design/human-interface-guidelines/keyboards
- "Show more" is a real button with a caret; genre chips say "Browse titles tagged X". Why: a clickable paragraph had no affordance. https://www.nngroup.com/articles/tooltip-guidelines/

## Reader

- Reading mode and page fit are kit segmented controls with icons (LTR, RTL, Strip; Screen, Width, Original) and shortcut tooltips. Why: wordy text-only segments at 1.24:1. https://m3.material.io/components/segmented-buttons/overview
- Paged zoom: Ctrl + wheel, double-click in the middle, `=` / `-` / `0`, and Zoom out / 100% / Zoom in buttons; the point under the pointer stays put; a drag pans; Escape unzooms; a page turn resets it. Why: no way to read small text or SFX. https://mihon.app/docs/guides/reader-settings
- Top bar adds a scrolling chapter list (current checked, read / page hints), Download this chapter (ring while downloading, check when saved), Keyboard shortcuts and Full screen (F11). Why: every exemplar reader has these one click away. https://github.com/mihonapp/mihon/blob/main/app/src/main/java/eu/kanade/presentation/reader/appbars/ReaderBottomBar.kt
- Page slider: 6px track in `Tokens::track` (3:1 or better), the thumb grows with an accent ring on hover and drag, and a "Page N" bubble follows it while dragging. Why: 1.39:1 track, blind scrubbing. https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html
- The run-on "Esc back · M mode · F fit" text became a shortcuts panel (`?`) with key caps, and every control's tooltip shows its key. Why: most shortcuts were undiscoverable. https://developer.apple.com/design/human-interface-guidelines/keyboards
- Previous / Next chapter buttons are kit icon buttons, disabled with a reason ("This is the first chapter") instead of a dimmed raw colour. Why: `rgba(0x7d859066)` and no explanation. https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Chapter transition card: Finished / Next lines with icons, a Downloaded or Reads online tag, a "1 chapter missing" warning when numbers skip (`logic::missing_between`), icon buttons with shortcuts. Why: plain text with no status. https://mihon.app/docs/guides/reader-settings
- Failed page and failed chapter use `error_view` (Try again, Back). Why: hand-rolled states. https://www.nngroup.com/articles/error-message-guidelines/
- Bars use `glass_strong` / `glass_border`, the page canvas `Tokens::canvas`, the mini back button `Tokens::glass`. Why: four raw colours. https://fluent2.microsoft.design/color
- Every bar control, page click and menu close hands keyboard focus back to the reader, because kit controls no longer move focus on a mouse press. Why: shortcuts must keep working after a click. https://www.w3.org/WAI/WCAG22/Understanding/keyboard.html

## Browse

- Header is `page_title` with the Browse glyph; source chips get kit hover, pressed and focus states and a tab role; with more than five sources they collapse into one picker button with a scrolling menu (language and 18+ as hints). Why: a wall of chips pushed results down. https://lawsofux.com/hicks-law/
- Popular / Latest / Search / Tags tabs have icons (fire, clock, magnifier, tag); the search tab is shortened to 24 characters and has a Clear search button. Why: text-only tabs, a search tab that could not be closed. https://www.nngroup.com/articles/icon-usability/
- "Finding tag…" and "Opening title…" are shimmer text; their errors are toasts, not text crammed into the tab row. Why: errors in the tab row never went away. https://m3.material.io/components/snackbar/overview
- The grid ends with a footer: skeletons while more may load, "You have reached the end of this list" when done, or an inline alert with Retry when loading more failed. Why: infinite scroll just stopped. https://www.nngroup.com/articles/infinite-scrolling-tips/
- No results: Clear search; no titles for tags: Clear tags; empty source: Try again. Why: dead ends. https://www.nngroup.com/articles/empty-state-interface-design/
- No enabled sources: `empty_view` "Add a source" with a primary button that opens Settings > Sources. Why: the old text-only button was a dead end. https://www.nngroup.com/articles/empty-state-interface-design/
- Tag panel: shimmering chip skeletons, an inline alert with Try again, filter chips with checks, group labels, a "Clear all" button with an icon, a tag icon on "Browse MangaDex tags". Why: missing icons and states. https://m3.material.io/components/chips/guidelines

## History

- Each row is one target (resume); details and removal are explicit icon buttons with tooltips (Resume, Title details, Remove from history). Why: the cover and the row did different things with no cue. https://www.nngroup.com/articles/tooltip-guidelines/
- Remove from history hides the row and offers Undo; the database delete waits for the Undo window. Why: removal was instant with no undo. https://www.nngroup.com/articles/confirmation-dialog/
- "Clear history" (broom icon) opens a confirmation dialog with the count and what stays ("Clear history" / "Keep history"). Why: a two-click text button was easy to trigger by accident. https://www.nngroup.com/articles/confirmation-dialog/
- Empty: "Open Library" action; no match: "Clear search". Loading: shimmering row skeletons. Why: dead ends and static boxes. https://www.nngroup.com/articles/empty-state-interface-design/

## Downloads

- Queue card: a progress ring and the kit progress bar (moving comet), Pause / Resume (the chapter in progress finishes first), Cancel all, and an "Up next" disclosure listing every queued chapter with its own remove button. Why: only the first job was visible and nothing could be cancelled singly. https://www.nngroup.com/articles/visibility-system-status/
- Deleting a title's downloads opens a confirmation dialog that names the chapter count, size and the Studio work that goes with it ("Delete downloads" / "Keep"). Why: an inline two-click trash removed irreplaceable Studio work. https://www.nngroup.com/articles/confirmation-dialog/
- Rows are one target (read); the chapter list and delete are explicit icon buttons; Resume has a leading play icon. Why: hidden second target, trailing icon hack. https://www.nngroup.com/articles/tooltip-guidelines/
- Empty: "Go to Library" with the Library glyph leading; sizes use the kit `fmt_bytes`. Why: wrong glyph, trailing icon. https://fluent2.microsoft.design/iconography
- Download notification: glass tokens, progress ring as its icon, a 28px close button, Pause, and icon buttons for Cancel all / Retry / Open Downloads; the slide-in respects reduced motion. Why: 24px close target and static glyph while busy. https://www.nngroup.com/articles/fitts-law/
- Queue model (`downloads.rs`): per-chapter error memory (shown on the chapter row), pause, single-chapter cancel, and a finished state when everything is cancelled while paused. Why: failed chapters silently reverted to "not downloaded". https://github.com/mihonapp/mihon/blob/main/app/src/main/java/eu/kanade/presentation/manga/components/ChapterDownloadIndicator.kt

## Settings > Sources

- Enable / disable is a kit switch with a tooltip. Why: a toggle disguised as a filter chip. https://www.nngroup.com/articles/toggle-switch-guidelines/
- Remove plugin is a danger trash icon that opens a confirmation dialog explaining what happens to titles from it. Why: instant uninstall. https://www.nngroup.com/articles/confirmation-dialog/
- Test is a ghost button with a pulse icon that spins while testing; the result is a status tag ("OK · 24 results · 312 ms" / "Not working") plus the error text. Why: a coloured sentence with no icon. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- Rows show the source avatar, and language, 18+ and Plugin / Built in as tags. Why: plain metadata text. https://www.nngroup.com/articles/visual-hierarchy-ux-definition/
- Install: link icon in the field, inline validation ("Paste a link that starts with https://"), Enter installs, the button is disabled with a reason until the link looks right and shows a spinner while installing; results are toasts. Why: "Installing..." text and no validation. https://www.nngroup.com/articles/error-message-guidelines/
- Chapter language uses filter chips with checks under a section header with the language glyph. Why: 1.24:1 selected chip. https://m3.material.io/components/chips/guidelines

## Audit items skipped, and why

- browse.rs:607 first-run onboarding (Add repository / Install extension / Skip): left to the onboarding agent as instructed; Browse keeps an `empty_view` "Add a source" that opens Settings > Sources.
- sources.rs:259 repositories and a browsable extension list: left to the repository-install agent as instructed; this pass only polished the existing single-link install.
- browse.rs:518 global "Search all sources" and Pinned / Last used groups: needs a multi-source search mode and result layout; the source wall now collapses into a scrolling picker above five sources. A searchable picker needs a filter field inside a menu, which the kit does not have.
- library.rs:263 "NEW" badge after an update: there is no stored per-chapter "new" flag; the unread badge and the result toast carry the news.
- library.rs:272 Display options (grid size, compact / comfortable, badge toggles): no settings back them yet.
- detail.rs:878 shift-click range selection with a floating selection bar: a large interaction model; covered for now by "Mark previous as read" and bulk Undo.
- detail.rs:308 mapping the vertical wheel to sideways scrolling in "You may also like": deliberately not done, it would hijack the page's own scrolling; arrows and an edge fade were added instead.
- downloads_page.rs:274 "Keep Studio work" checkbox: `confirm_dialog` has no content slot (mv-ui) and `Downloads::delete` removes pages and Studio work together.
- downloads_page.rs:131 queue reordering: needs drag and drop.
- reader.rs:651 chapter bookmark: the chapter table has no bookmark column (mv-core). Reader settings sheet: every option already sits in the bottom bar.
- reader.rs:721 page thumbnail while scrubbing: a page-number bubble only.
- reader.rs:833 one-time tap-zone overlay on first open: first-run work for the onboarding pass.
- Category reordering: mv-core has no `category::reorder`; create, rename and delete are done.
- library.rs:309 gold star on the Favorites chip: `filter_chip` draws its icon in `text_3`; the chip uses the star glyph without the gold.
- components.rs:78, components.rs:43, theme.rs:7 (shared chip, focus ring, tokens, CI colour check): mv-ui items, already delivered by the kit.
- app/src/shell.rs:42 Library nav icon: shell crate, not owned here (the Library page, empty states and CTAs here all use `glyphs::LIBRARY`).

## Review pass (adversarial check of the restyle)

- Cover card: the card box is now a fixed hover/click target and only an inner face lifts 3px on hover (busy spinner and Continue reading live in the face, `CardInfo::busy` / `CardInfo::action`). Why: moving the hovered element itself moved its hit area, so the pointer in the bottom 3px flickered between hovered and not. https://developer.mozilla.org/en-US/docs/Web/CSS/:hover
- Chapter rows: Mark read and More actions carry their own hidden opacity and show on row hover, on keyboard focus (`in_focus`) and while their menu is open. Why: the opacity sat on a wrapper, so Tab landed on invisible buttons and the More trigger vanished under its own open menu. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- Reader, paged Fit width / Original: the page wrapper is `flex_none` again. Why: GPUI sizes the scroll range from direct children, and a shrinkable wrapper with `min_h(100%)` collapsed to the window, so tall pages could not scroll. https://developer.mozilla.org/en-US/docs/Web/CSS/flex-shrink
- Reader bars stay up while the chapter list menu is open or keyboard focus is on a bar control. Why: the pointer inside the menu is outside the bars, so they hid after 2.6 s and took the menu (or the focused control, and with it every shortcut) away. https://www.nngroup.com/articles/visibility-system-status/
- Reader: Space on a Tab-focused bar control only activates that control. Why: GPUI clicks the control on key up while the reader also turned the page on key down. https://www.w3.org/WAI/ARIA/apg/patterns/button/
- Library "Update whole library" checks every title without switching the open category tab. Why: it silently reset the user's tab to All. https://www.nngroup.com/articles/ten-usability-heuristics/ (user control)
- "You may also like": the subtitle says "on <source>" only for another source (it always said it, because the page state is lent out while list rows render), the edge fade goes at the end of the row, and the arrows disable at either end. Why: wrong state shown and a fade over the last card. https://www.nngroup.com/articles/designing-effective-carousels/
- Detail: downloading a chapter whose delete is still in its Undo window keeps the files (same as Undo) instead of queuing a download that the pending delete would wipe. Why: "Download N more" counted that chapter as missing. https://www.nngroup.com/articles/slips/
- Delete-download toast uses the standard success icon. Why: a green filled trash mixed "done" with "destructive". https://m3.material.io/components/snackbar/guidelines
- Clippy clean-up (type aliases for the cover lookup and menu handler, derived `ChapterView::default`, `then_some`). Why: the crate must build with zero warnings. https://rust-lang.github.io/rust-clippy/master/

### Review: checked and left as is

- Second `.hover()` / `.tooltip()` on kit builders: none (segmented, chip, tab, checkbox, action tiles and triggers each get one).
- Lucide paths and raw colour literals: none left in the crate.
- Audit high/medium items: all claimed items are present in code; the skipped ones above are unchanged.
