# Shell (native/app) design changes

One line per change: what, why, reference. Code: `native/app/src/{shell,routes,keys,palette,updates,prefs,main}.rs`.

## Title bar

- Raised from 34px to 40px and filled with controls: sidebar toggle, Back, Forward, breadcrumbs, a centred search field and the assistant toggle. Why: Windows title bars grow when they hold controls and put global search in the centre; 40px rather than 48px because the gpui-component caption buttons are a fixed 34px wide. https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design
- Search field "Search or jump to… Ctrl+K" (magnifying glass, key cap) opens the command palette; below 840px it becomes an icon button with the same tooltip. Why: one visible entry point to search and commands (Linear, Raycast); centred search in the Windows title bar. https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design
- Sidebar toggle shows the `sidebar-simple` glyph filled while the sidebar is open and as an outline while it is a rail, with "Collapse sidebar" / "Expand sidebar" (or "Show/Hide navigation" in a narrow window) and Ctrl+B in the tooltip. Why: the icon shows the state and the tooltip names what a click does. https://fluent2.microsoft.design/iconography
- Back and Forward are kit `icon_button`s with "Back to Library · Alt+Left" / "Forward to One Piece · Alt+Right" tooltips; when there is nowhere to go they are disabled and say so. Why: icon-only controls need a label; disabled controls should explain themselves. https://www.nngroup.com/articles/icon-usability/ and https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- Breadcrumbs are Section › Destination › Title ("Manga › Library › One Piece", "Manga › One Piece › Chapter 12", "Anime › Discover"); every level but the last is a link, the last truncates with a tooltip when long. Why: show where you are and let every level take you up. https://www.nngroup.com/articles/breadcrumbs/
- A breadcrumb link goes back to that page as it was left (scroll, filters), moving the pages in between onto Forward, instead of opening a fresh copy. Why: the way up should restore context, as Back does. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Manga titles and chapter names for the breadcrumb are looked up from the library database off the UI thread and cached per id (the routes carry only ids). Why: "Manga" and "Reader" told the user nothing. https://www.nngroup.com/articles/breadcrumbs/
- While a manga title or chapter name is still being looked up, its crumb is a text-shaped skeleton instead of the generic "Manga" / "Reader" (which then jumped to the real name); a failed lookup falls back to the generic word. Why: placeholders shaped like the content, never wrong content. https://www.nngroup.com/articles/skeleton-screens/
- In Settings (which has no sidebar) Back becomes a labelled "Back to Library" button with Esc in its tooltip, and Esc leaves Settings when no field has focus. Why: a clear, consistent way back from secondary pages. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Assistant toggle moved from the sidebar to the right of the title bar (sparkle glyph, selected while open, Ctrl+J, disabled with a reason on Studio, which has its own chat). Why: a supplementary pane is toggled, not navigated to, and lives on the trailing side. https://developer.apple.com/design/human-interface-guidelines/split-views
- Title bar content dims to 60% while the window is inactive. Why: Windows title bars show the active state. https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design
- The OS window title follows the page ("One Piece – MangaVox"). Why: the taskbar, Alt+Tab and screen readers name the current document. https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design

## Sidebar

- Every row is the kit `nav_item`: fill-weight accent icon, `active` container and a 3px accent bar on the current destination, hover that never weakens the selected state, icon and label brighten together, focus ring and Tab stop. Why: filled = selected; selected must stay stronger than hover; state applies to icon and label together. https://fluent2.microsoft.design/iconography, https://www.radix-ui.com/colors/docs/palette-composition/understanding-the-scale, https://m3.material.io/foundations/interaction/states/state-layers
- In the 60px rail every item, the section switcher and the update icon have tooltips (Settings shows Ctrl+,). Why: NavigationView compact mode puts a tooltip on every icon-only item. https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview
- The destination a page was opened from stays highlighted on title, chapter and player pages (Browse stays lit on a manga opened from Browse). Why: "you are here" cues on child pages. https://www.nngroup.com/articles/navigation-you-are-here/
- Section switcher is the kit `segmented` control (Manga, Anime, Movies with icons; fill weight on the current one) and, in the rail, three icon toggles in a recessed well with Ctrl+1/2/3 in their tooltips. Why: one choice of three is a segmented control; the rail keeps the same grouping. https://m3.material.io/components/segmented-buttons/overview
- Brand: the real MangaVox mark (`brand/mangavox.svg`, 24px, accent) and a 15px semibold wordmark replace the "M" tile; collapsed shows the mark only; clicking it goes to the section's home. Why: use the app's real icon in the window's title area; the logo leads home. https://learn.microsoft.com/en-us/windows/apps/design/style/iconography/app-icon-design
- Icons from `mv_ui::glyphs` so each meaning has one glyph: Library = books (no longer the same book as the Manga section), History = clock with a back arrow, Downloads = download tray, Home = house (no longer a copy of the section icon), Discover = compass, Favorites = star, Studio = magic wand, Settings = gear. Why: distinct, recognisable icons; one icon per meaning. https://www.nngroup.com/articles/icon-usability/ and https://developer.apple.com/design/human-interface-guidelines/sf-symbols
- Downloads shows the number of chapters queued (a dot in the rail). Why: visibility of system status. https://www.nngroup.com/articles/ten-usability-heuristics/
- Studio sits under a "Studio" group label (a hairline in the rail); Assistant is no longer a sidebar row, so the two "magic" icons are no longer side by side. Why: group by kind; destinations and panel toggles look different. https://www.nngroup.com/articles/icon-usability/
- The destination list scrolls when the window is short, with a 2px inset so focus rings are not clipped; Settings stays pinned at the bottom. Why: nothing becomes unreachable at the 480px minimum height; WCAG 2.4.7 focus visible. https://www.w3.org/WAI/WCAG22/Understanding/focus-visible
- Width 256px (was 244) so the three labelled segments fit at 12.5px. Why: labels beside icons are understood faster than icons alone. https://www.nngroup.com/articles/icon-usability/
- The collapsed choice is remembered in config.json (`shell.sidebarCollapsed`); Studio still forces the rail and restores your choice afterwards. Why: the pane keeps the state the user left it in. https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview

## Update in the sidebar footer

- The always-on "✓ MangaVox v8.0.5001" badge is gone; the footer shows something only when there is an update, a download, an install or a failure (the version lives in Settings → About). Why: minimalist design; status only when it matters. https://www.nngroup.com/articles/ten-usability-heuristics/
- Offer card: "Update available", what happens next, Later (ghost) and Download (primary, download icon); after Later a quiet "Update to vX" button brings it back. Why: say what will happen; let the user defer. https://developer.apple.com/design/human-interface-guidelines/alerts
- Download shows the kit animated `progress_bar` and percentage, install shows a spinner, failure shows the error with Close and Try again (retry icon). Why: progress indicators must move so the app does not look stuck; errors need a remedy. https://www.nngroup.com/articles/progress-indicators/ and https://www.nngroup.com/articles/error-message-guidelines/
- In the rail, the update icon opens a confirmation ("Update to MangaVox vX?", "Download and restart" / "Not now") instead of starting a ~200 MB download and restart on one click; while downloading it shows a progress ring with the percentage in its tooltip. Why: confirm actions with serious consequences (a restart loses state). https://www.nngroup.com/articles/confirmation-dialog/

## Navigation and routing

- Forward history: Back moves the page onto a forward stack, Forward (Alt+Right, mouse forward button, title-bar arrow) brings it back as it was; any new navigation clears it. Why: Windows apps support back and forward with keyboard and mouse buttons. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Switching sections parks the page, Back and Forward of the section; leaving from Settings, Studio or the player parks the page underneath instead of losing the section's history. Why: each section keeps its own place, like tabs in Netflix and Crunchyroll. https://www.nngroup.com/articles/consistency-and-standards/
- Moving between Settings pages from the palette replaces the page instead of stacking Back steps. Why: Back should leave Settings, not walk through its tabs. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Anime and Movies each keep their own Discover page (the keep-alive key includes the section), so a kind or mood chosen in one no longer changes the other. Why: the same control in the same place behaves in context. https://www.nngroup.com/articles/consistency-and-standards/
- The fallback page for a route without a page is the kit `empty_view` (traffic-cone duotone icon, plain message, "Go back" button) instead of "This page is being built." Why: empty states explain and offer a way out. https://www.nngroup.com/articles/empty-state-interface-design/

## Command palette (Ctrl+K)

- Search over every destination (current section first), Studio, Settings and each Settings page, the shell's actions (collapse sidebar, open assistant, back, forward, switch section, full screen, check for updates) and, while typing, the manga library; recently opened titles come first; the current page is checked. Why: one place to reach any page or action from the keyboard. https://www.nngroup.com/articles/ui-accelerators/
- Built on the gpui-component `Command` list (arrow keys, Enter, Escape clears the query and then closes), in a 620px popover with the dialog backdrop, a footer of key hints and a kit empty state. Why: familiar Linear/Raycast pattern; Escape must always get you out. https://developer.apple.com/design/human-interface-guidelines/keyboards
- Groups never change places while you type (only the library group at the end fills in), so the highlighted row cannot jump to a different command between keystrokes. Why: predictable results are a precondition for keyboard use. https://www.nngroup.com/articles/ui-accelerators/
- Rows show their shortcut (Ctrl+B, Ctrl+J, Alt+Left...) from the bound actions. Why: show shortcuts where the command lives so users learn them. https://developer.apple.com/design/human-interface-guidelines/keyboards
- Closing the palette gives focus back to what had it (the reader or player keep their keys). Why: the kit no longer moves focus on click, so views must not lose it silently; WCAG 2.4.3 focus order. https://www.w3.org/WAI/WCAG22/Understanding/focus-order
- "Check for updates" reports the result as a toast ("MangaVox vX is up to date", an error, or "vX is available" with an Update action). Why: every action gets feedback. https://www.nngroup.com/articles/ten-usability-heuristics/

## Shortcuts

- Ctrl+K palette, Ctrl+B sidebar, Ctrl+J assistant, Ctrl+, Settings, Ctrl+1/2/3 Manga/Anime/Movies, Alt+Left/Right back/forward, F11 full screen, Esc closes the palette, the floating sidebar or assistant, then Settings. Why: standard accelerators (Windows, Linear, Raycast, VS Code). https://learn.microsoft.com/en-us/windows/apps/design/input/keyboard-accelerators
- Shortcuts run from a keystroke observer, so they work whether or not anything has focus (kit controls no longer take focus on click); a key a page already used (a menu, a text field, the reader's Esc) or a key pressed while a dialog is open is left alone. Why: shortcuts must be predictable and never fight the focused control. https://developer.apple.com/design/human-interface-guidelines/keyboards
- Ctrl+, while Settings is open keeps the Settings page you are on instead of reopening Settings on its first tab. Why: the settings shortcut brings Settings forward; it must not throw away your place. https://developer.apple.com/design/human-interface-guidelines/settings

## Layout

- Below 1008px the sidebar becomes the 60px rail without overwriting your choice; the toggle then opens the full sidebar over the page with the backdrop (Esc, a click outside or a navigation closes it). Why: NavigationView switches to compact mode below 1008px and opens the pane as an overlay. https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview
- Assistant dock moved to the trailing edge; drag its left edge to resize (320 to 560px, remembered in `shell.dockWidth`); the page never gets narrower than 480px next to it, and below 900px (or when 480px would not fit) it floats over the page as a sheet with the backdrop and closes with Esc or a click outside. Why: HIG split views put supplementary panes on the trailing side; the content must keep a usable width. https://developer.apple.com/design/human-interface-guidelines/split-views
- The toast host is mounted once, at the bottom centre of the page area (also under the reader and player). Why: one consistent place for brief feedback. https://fluent2.microsoft.design/components/web/react/core/toast/usage
- Root font is Segoe UI Variable Text when installed (Windows 11), Segoe UI otherwise. Why: the Windows 11 UI font with optical sizing for small text. https://fluent2.microsoft.design/typography

## Hygiene

- `app/tests/no_raw_colors.rs` fails the build on raw colour literals (`rgb(0x..)`, `hsla(..)`, `black()`/`white()`) or Lucide `icons/*.svg` paths anywhere in the shell, as mv-ui and ui-player already do. Why: one token set and one icon family reach every pixel, so Appearance presets and icon weights stay coherent. https://fluent2.microsoft.design/design-tokens

## Skipped audit items

- Route::Welcome first-run onboarding (high): a later agent adds it, as instructed.
- Settings › tab label in the breadcrumb (medium, part): the Settings view keeps its tab privately, so the shell can only know the tab it opened with and the crumb would go stale; needs a getter or event from ui-settings.
- Scoped `Route::Discover { kind }` / `MediaHistory { kind }` / `Favorites { kind }` (medium, part): `Route` lives in mv-ui and Discover's kind is private to ui-media; done here: one Discover per section. Movies' Discover still opens on Anime until ui-media reads the section.
- `title: String` on `Route::MangaDetail` / `Reader` (medium, part): `Route` is in mv-ui; the shell looks the names up from the database instead.
- Dock close button in the dock header (medium, part): the header is ui-assistant's; the title-bar toggle, Ctrl+J and Esc (floating) close it.
- "Back to …" row at the top of the Settings rail (medium, part): the rail is ui-settings'; the shell's labelled Back button and Esc cover the exit.
- Tray menu icons (low): tray-icon needs rasterised RGBA icons, which means an SVG rasteriser dependency for two menu rows; Windows tray menus (Explorer, Teams, Spotify) are commonly text-only.
- Discover as a telescope (audit icon list): the kit's `glyphs::DISCOVER` is the compass (same meaning as Browse); one glyph per meaning wins over a new icon.
- Studio as a clapperboard (audit icon list): `glyphs::STUDIO` stays the magic wand; the clash with Assistant's sparkle is gone because Assistant moved to the title bar.
- Panel-right open/close glyphs for the assistant toggle: no such pair is vendored in Phosphor; the toggle uses the assistant's own glyph with the selected (filled) state.
- Theme presets, reduce motion, component theme, type ramp, tokens, kit primitives (audit items in mv-ui): already in the new kit; `main.rs` calls `mv_ui::theme::init` after `Theme::change`, which applies the gpui-component theme.
- Loading indicator for the palette's "Your library" group: it is one local SQLite read that finishes in milliseconds, so an indicator would only flash; the group fills in when ready and never moves the highlight. NN/g response-time limits (no indicator under about 1 s): https://www.nngroup.com/articles/response-times-3-important-limits/

## Adversarial review fixes

- F11 in the reader toggled full screen twice (the reader binds F11 as a plain key, so the shell's observer saw it unhandled and toggled again); the shell now leaves F11 to the reader and the player (which keeps its own full-screen state on F). Why: one key, one action. https://learn.microsoft.com/en-us/windows/apps/design/input/keyboard-accelerators
- Leaving the player through the shell (Ctrl+, Ctrl+1/2/3, a breadcrumb) now exits full screen first, as Back already did. Why: full screen belongs to the video, not to the page you go to. https://developer.apple.com/design/human-interface-guidelines/going-full-screen
- No command palette on the player: the search field is hidden there and Ctrl+K is ignored, because the video is drawn over the app window and the palette would open out of sight holding the keyboard. Why: focus must never be hidden under other content (WCAG 2.4.11). https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html
- Breadcrumb reads "Settings › Audio" and follows the page the Settings view shows (`ui_settings::current_tab` plus an observer), and the window title says "Audio – Settings – MangaVox". Why: breadcrumbs and titles name the current page. https://www.nngroup.com/articles/breadcrumbs/
- Asking for the Settings page already on screen does nothing, asking for another page opens it even when the route that opened Settings named it (the palette's "Audio" after moving to Themes inside Settings used to do nothing). Why: a command must visibly do what it says. https://www.nngroup.com/articles/ten-usability-heuristics/
- The palette marks the Settings page on screen, and its Settings rows take icon, name and search words from ui-settings (Mode was a toggle and AI providers a sparkle in the palette but an app window and a circuit head in the rail). Why: one glyph per meaning; "you are here". https://developer.apple.com/design/human-interface-guidelines/sf-symbols and https://www.nngroup.com/articles/navigation-you-are-here/
- Palette rows appear only where they work: no "Open assistant" in Studio or on full-screen pages, no sidebar toggle where there is no sidebar; the full-screen row shows corners-in while in full screen. Why: commands that cannot run are hidden from command menus (VS Code when-clauses). https://code.visualstudio.com/api/references/when-clause-contexts
- Tab no longer moves focus out of the open palette to the controls behind its backdrop (gpui-component focus trap on the panel). Why: a modal keeps keyboard focus inside it. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- Escape in Settings: a text field on screen lets go of focus on the first Escape and the next one leaves; focus left on a page no longer shown, or on a control reached with Tab, no longer blocks the way out (the check reads the key's context instead of "anything focused"). Why: Escape must always get you out, without losing typing. https://developer.apple.com/design/human-interface-guidelines/keyboards
- A destination of another section (the assistant's "open my library", Studio's "Open Library", "Browse anime" in Movies history) now opens in its own section, so the sidebar, highlight and breadcrumbs match it; from Studio or Settings, Back still returns there. Why: consistent "you are here" cues; Back retraces the user's path. https://www.nngroup.com/articles/navigation-you-are-here/ and https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Breadcrumb names cut off by the space get their full text as a tooltip: links over about 24 characters, the current page over about 28 (it now also takes the pointer, which the title bar's drag area otherwise kept from it, so the tooltip could never show on Windows). Why: truncated text needs a way to read it in full. https://learn.microsoft.com/en-us/windows/apps/design/controls/tooltips
- "Check for updates" while an update downloads or installs now says so in a toast instead of doing nothing. Why: every action gets feedback. https://www.nngroup.com/articles/ten-usability-heuristics/
- Update failures no longer read "Update failed: Update failed: …" in the footer card and rail tooltip. Why: error messages say the problem once, plainly. https://www.nngroup.com/articles/error-message-guidelines/
- Layout writes to config.json are ordered and always write the newest layout, so Ctrl+B twice in quick succession cannot leave the older state on disk. Why: the pane must reopen as it was left. https://learn.microsoft.com/en-us/windows/apps/design/controls/navigationview

## Skipped in the review

- Focus trap for the floating assistant and the floating sidebar: the trap needs `track_focus` on the wrapper, which would take focus from the assistant's composer on clicks in empty parts; the assistant crate should own its trap.
- Toasts while the floating assistant or floating sidebar is open are drawn under their backdrop: the host sits in the page area (bottom centre of the page) by kit design.
- Switching section from Studio (Ctrl+1/2/3, the switcher) does not keep Studio in Back: sections behave like tabs.
- In narrow windows the palette's sidebar row says "Expand sidebar" while the title bar says "Show navigation": the palette context does not tell narrow from collapsed; cosmetic.

## Second adversarial review

- Opening another section's destination from Studio or Settings kept Studio / Settings in Back only when the section did not open on that page already; when the destination was the section's home (or the page it was left on) Back was lost. Back now always leads to Studio / Settings. Why: Back retraces the user's path. https://learn.microsoft.com/en-us/windows/apps/design/basics/navigation-history-and-backwards-navigation
- Clicking the logo on the home page swapped the page for an older copy of itself from history (and filled Forward); it now does nothing beyond closing the floating sidebar. Why: a control for where you already are must not change the page. https://www.nngroup.com/articles/navigation-you-are-here/
- Turning Studio mode off left Studio in Back and Forward (in every section), so Back could open the hidden Studio page; those entries are now dropped. Why: a hidden feature must not come back through history. https://www.nngroup.com/articles/ten-usability-heuristics/
- An update started from the rail confirmation or the "Update" toast that then failed showed nothing in the expanded sidebar (the offer was closed), while the rail showed a failure icon even for a failed manual check the toast already reported. Confirming now opens the offer, and the rail shows a failure only while the offer is open, so both sidebars agree and every failed download keeps its Try again. Why: errors need a visible remedy, shown consistently. https://www.nngroup.com/articles/error-message-guidelines/
- The palette's sidebar row now uses the title bar's wording ("Show navigation" / "Hide navigation" in a narrow window, "Expand" / "Collapse sidebar" otherwise). Why: one command, one name. https://www.nngroup.com/articles/consistency-and-standards/
- The palette marks the page you are on with its icon in the fill weight as well as the check, like the sidebar's current destination. Why: filled icons mark the selected state. https://fluent2.microsoft.design/iconography
- The search field's magnifying glass now brightens with its text on hover (it stayed dim). Why: a state applies to the whole control, icon and label together. https://m3.material.io/foundations/interaction/states/state-layers
- The assistant's resize edge stays lit while it is dragged (the pointer soon leaves the 6px edge, which turned the line off mid-drag). Why: direct manipulation needs continuous feedback. https://www.nngroup.com/articles/direct-manipulation/

## Skipped in the second review

- Mouse back/forward buttons while the palette, a dialog or a floating panel is open: GPUI only runs the root's mouse handler when its hitbox is hovered, and those layers occlude it, so the buttons already do nothing under a modal; no change needed.
- `segmented` (mv-ui) moves its selection on Left/Right without checking modifiers, so Alt+Left on a focused section switcher changes section and stops the key before the shell's Back sees it; needs a modifier check in mv-ui.
- `Config` (mv-core) locks per instance and every writer in the process uses the same `config.json.<pid>.tmp` name, so the shell's background layout write can race another config write; needs a process-wide lock or unique temp names in mv-core.
- The centred search field is centred in the space left of the caption buttons (about 50px left of the window centre), because the caption buttons sit outside the title bar content; visual QA to judge.
