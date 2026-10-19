# mv-ui component kit

The shared controls every page uses. They take colours from `Tokens` (so every Appearance preset
applies), icons from Phosphor (`ph::*`), and share one set of hover, pressed and focus states.
See them all, live: `cargo run -p mv-ui --example kit_gallery`.

```rust
use gpui_kit::*;
use mv_ui::{btn, icon_button, ph, ButtonKind, Tokens};   // import what you use; everything is at the root
use mv_ui::toast::{self, Toast};
```

## Rules

- **No raw colours.** Use `Tokens::*()` (`tests/no_raw_colors.rs` fails on `rgb(0x..)` in mv-ui).
- **Icons are Phosphor.** Pass `ph::NAME`; weights follow state (bold at 14px and below, fill when selected/on, duotone for empty states). `IconSrc` also accepts an old `"icons/x.svg"` path.
- **Builders own their hover and tooltip.** GPUI panics in debug builds if `.hover()` or `.tooltip()` is set twice on one element. Do not call `.hover(..)` or GPUI's `.tooltip(closure)` on a kit builder; use the builder's own `.tooltip("text")`. Your other `Styled` calls (`.mt(..)`, `.w_full()`) are applied last and win.
- **Handlers:** click handlers are `Fn(&ClickEvent, &mut Window, &mut App)`, so `cx.listener(..)` works. Index/flag handlers take references (`&usize`, `&bool`) for the same reason.
- **Focus:** kit controls are Tab stops with a focus ring, and a mouse press on one does not move keyboard focus (so a click never pulls focus out of a text field). A view that needs focus after a click (reader, player shortcuts) calls `window.focus(&handle)` in the handler.
- **Ids:** builders render a `Stateful<Div>`; give each one an id unique among its siblings.
- **One primary button per view.** Destructive actions: `DangerGhost` in rows, `Danger` only on the confirming step. Prefer an Undo toast to a confirmation when the action can be reversed.

## What to use for what

| Need | Use | Example |
|---|---|---|
| Text button | `btn` | `btn("save", "Save", ButtonKind::Primary).icon(ph::CHECK).on_click(..)` |
| Button that cannot run yet | `.disabled(reason)` | `btn("export", "Export", ButtonKind::Secondary).disabled("Render the video first")` |
| Busy button | `.loading(bool)` | `btn("dl", "Download", ButtonKind::Primary).loading(self.busy)` |
| On/off text button ("In library") | `.selected(bool)` | `btn("lib", if saved { "In library" } else { "Add to library" }, ButtonKind::Secondary).icon(glyphs::SAVED).selected(saved)` |
| Link that leaves the app (as a button) | `.trailing_icon` | `btn("docs", "Docs", ButtonKind::Ghost).trailing_icon(ph::ARROW_UP_RIGHT)` |
| Sizes | `.small()` 28px, default 32px, `.large()` 40px | `btn(..).small()` |
| Old call sites | `button(id, label, kind)` | returns `Stateful<Div>`; same look, no builder methods |
| Icon-only button | `icon_button(id, ph::X, "tooltip")` | `icon_button("close", ph::X, "Close").kbd("escape").on_click(..)` |
| Icon toggle in a toolbar | `.selected(bool)` | `icon_button("grid", ph::SQUARES_FOUR, "Grid view").selected(grid)` |
| Destructive icon | `.danger()` | `icon_button("rm", ph::TRASH, "Delete").danger()` |
| Old icon buttons | `icon_button(id, "icons/x.svg", enabled)` | returns the original 30px `Stateful<Div>` |
| Favourite star (manga, anime, movies, voices, models) | `favorite_star` | `favorite_star("fav", on).on_click(cx.listener(..))` |
| Any on/off icon (bookmark, pin, default voice) | `icon_toggle` | `icon_toggle("bm", ph::BOOKMARK_SIMPLE, on, Tokens::accent(), "Remove bookmark", "Bookmark")` |
| "In use" / "Default" / "Done" mark | `selected_badge()` | filled accent disc with a check, never an outline |
| Filter that toggles | `filter_chip` | `filter_chip("ongoing", "Ongoing", on).count(128).on_click(..)` |
| Tab-like pill | `chip(id, label, active)` | unchanged |
| One of 2 to 5 options | `segmented` | `segmented("mode", [(None, "Paged"), (None, "Webtoon")], ix).on_change(cx.listener(\|t, ix: &usize, _, cx\| ..))` |
| Checkbox / tri-state parent | `checkbox(id, CheckState)` | `checkbox("all", CheckState::Mixed).child("All chapters").on_click(..)` |
| Setting that applies at once | `switch` | `switch("autoplay", on).label("Autoplay").on_click(cx.listener(\|t, on: &bool, _, cx\| ..))` |
| Selectable list row (chats, models, history) | `list_row(id, selected)` | `list_row(("chat", i), i == cur).child(..).on_click(..)`; selected stays stronger than hover |
| Sidebar / rail item | `nav_item` | `nav_item("lib", ph::BOOKS, "Library", active, collapsed).badge(3).on_click(..)` |
| Page heading | `page_title` | `page_title("Library").icon(ph::BOOKS).subtitle("128 series").action(btn(..))` |
| Section heading | `section_header` | `section_header("Continue reading", Some(ph::BOOK_OPEN)).count(12).see_all(..)` |
| Uppercase group label | `section_label("Voices")` | append `.child(..)` for a trailing element |
| Settings group and row | `setting_section`, `setting_row` | `setting_section("Playback", None).child(setting_row("Autoplay", "Start the next episode", switch(..)))` |
| Secondary text | `muted("12 chapters")` | `text_3`, small |
| Poster card | `cover_tile` | `cover_tile("m1", title).sub("Ch. 40").cover(src).progress(0.4).favorite(true).on_click(..)` |
| Old cover cards | `cover_card(..)` | same signature, now with hover lift, fallback image and 2-line title |
| Overflow ("...") menu | `overflow_menu` | `overflow_menu("more", vec![MenuItem::new("Rename", f).icon(ph::PENCIL_SIMPLE), MenuItem::separator(), MenuItem::new("Delete", g).icon(ph::TRASH).danger()])` |
| Sort / choice menu | `menu_button` | `menu_button("sort", ph::SORT_ASCENDING, "Sort", items)` with `MenuItem::checked(true)`, or `.label("Title")` for a text trigger |
| Rows in a custom popover | `menu_row` + `popup_surface()` | `menu_row("m-1", "Qwen 2.5").hint("7B · 4.7 GB").selected(cur).on_click(..)` |
| Shortcut chip | `kbd("ctrl-k")` | shows "Ctrl+K" |
| Tooltip on your own element | `tip` / `tip_kbd` | `.tooltip(tip("Copy link"))`, `.tooltip(tip_kbd("Search", "ctrl-k"))` |
| Your own clickable div | `interactive(id)` or `.interactive()` | adds hover, pressed, Tab stop, focus ring, Enter/Space; use `.pressable()` if you set your own `.hover()` |
| Brief feedback, Undo, Retry | toast | `toast::show(cx, Toast::success("Chapter deleted").action("Undo", \|w, cx\| ..))` |
| Problem that stays until fixed | `inline_alert` | `inline_alert(AlertKind::Warning, "Voice engine missing", "Studio needs Kokoro.").action("Install", ..)` |
| Page-wide problem | `banner` | `banner(AlertKind::Error, "Could not reach MangaDex.").action("Retry", ..)` |
| Nothing to show | `empty_view` | `empty_view(ph::BOOKS, "Your library is empty", "Add manga from Browse.").action(btn(..))` |
| Load failed | `error_view` | `error_view(err.to_string()).retry(cx.listener(\|t, _, _, cx\| t.reload(cx)))` |
| Loading placeholders | `skeleton_card`, `skeleton_row`, `skeleton_text(n)`, `skeleton_line(w)`, `skeleton_block(w, h)`, `skeleton(w, h, r)` | shimmer, static under reduced motion |
| Unknown wait, as text | `shimmer_text("Thinking…")` | |
| Unknown wait, as icon | `spinner(id, color, px)` | |
| Known progress | `progress_bar(id, p, active)`, `progress_ring(p)` | |
| Rating / quality | `meter(3.0, 5)` | |
| Score with a gold star | `rating_badge(8.4, 14.)` | "★ 8.4" (0-100 scores show as "86") |
| Storage split | `usage_bar(&[(bytes, Tokens::hue(Hue::Blue)), ..], total)` | stacked segments; add a legend |
| Job state | `status_tag(StatusKind::Running).label("Rendering 3/8")` | Queued, Running, Done, Failed, Cancelled, Info |
| Metadata label | `tag("1080p", Tone::Neutral)`, `icon_tag(ph::HARD_DRIVE, "Local", Tone::Accent)` | soft fill |
| Strong label | `badge("NEW", Tone::Accent)` | solid fill, use sparingly |
| Count | `.children(count_badge(n))` | `None` for 0, caps at 99+ |
| Multi-phase work | `stepper` | `stepper("phases", [(ph::SCAN, "Read"), (ph::MICROPHONE, "Voice")], current, &done, running, failed).on_select(..)` |
| Collapsible details | `disclosure` | `disclosure("log", "Show log").content(log_view)` (keeps its own open state) |
| Confirm in place | `confirm_inline` | `confirm_inline("Delete this chat?", "Delete chat", true).on_confirm(..).on_cancel(..)` |
| Confirm in a dialog | `confirm_dialog` | `confirm_dialog("Erase all data?", "This cannot be undone.", "Erase everything", true).cancel_label("Keep my data").on_confirm(\|w, cx\| ..).open(window, cx)` |
| Copy a value | `copy_button(id, value)` | check mark for 2 s after copying |
| Show a path | `path_chip(path)` | monospace, shortened in the middle, copy button |
| Link out | `external_link("Kokoro on Hugging Face", url)` | |
| Sizes | `fmt_bytes(n)` | "1.5 GB", "340 MB" |
| Fade something in | `appear(id, el)` | instant under reduced motion |

## Tokens and scales (`mv_ui::theme`)

Every `Tokens::*()` reads the live Appearance preset, so never cache a colour in a struct field.

- **Surfaces** (dark to light): `canvas`, `base`, `sidebar`, `surface`, `surface_2`, `surface_3` (= `popover`), `well` (recessed).
- **Lines**: `border`, `border_strong`. **State layers**: `hover` < `active` (selected) < `active_hover` < `pressed`; `focus_ring`; `track` (unfilled slider/switch, 3:1).
- **Text**: `text_1`, `text_2`, `text_3` (4.5:1 on every surface), `text_disabled`.
- **Accent**: `accent`, `accent_hover`, `accent_pressed`, `accent_soft`, `accent_border`, `accent_a(alpha)`, `on_accent` (text on an accent fill), `accent_2` (preset secondary).
- **Status** (status only): `success`, `info`, `warning`, `danger`, each with `_soft`; `warning_border`, `danger_border`.
- **Stars**: `favorite()` and `rating()` (one gold, #FACC15). Never use `warning` for a star.
- **Floating**: `glass` (over images), `glass_strong` (toasts, floating bars), `glass_border`, `overlay` (dialog backdrop), `scrim` (text on images), `overlay_gradient(angle)`.
- **Categories**: `Tokens::hue(Hue::Blue)` and `hue_soft(..)` for icon tiles, storage segments and chart series (Blue, Violet, Cyan, Green, Amber, Orange, Rose, Slate; `Hue::ALL`). Not for status.
- **Scales**: `theme::icon` XS 12 / SM 14 / MD 16 / LG 20 / XL 24 / HERO 32 / EMPTY 40; `theme::radius` XS 4 / SM 6 / MD 8 / LG 12 / XL 16 / PILL; `theme::space` 2 to 48 on a 4-pt grid; `theme::duration` FAST 120 / BASE 180 / SLOW 280 ms; `theme::easing::{standard, enter, exit}`.
- **Type**: `.ramp(ramp::BODY)` (Fluent 2 ramp: CAPTION_2 11/14 ... DISPLAY 68/92) sets size, line height and weight; the older `size::*` still works.
- **Theme API**: `theme::init(cx)` (startup, done in main.rs), `theme::set_palette(id, cx)` (Settings), `theme::palette_id()`, `theme::PRESETS`, `theme::contrast(fg, bg)`.
- **Motion**: check `motion::reduced()` before any animation of your own (Settings toggle or Windows "Animation effects" off); the kit's animations already do.

## Icons

- `ph::NAME` (Phosphor, 300 icons, four weights). `icon(ph::STAR.on(fav), colour, 14.)`, `ph_icon(i, Weight::Fill, ..)`, or `icon_ph(i, colour, px)` (bold at 14px and below, regular above).
- **Use `glyphs::*` first** so one meaning has one glyph everywhere: `glyphs::LIBRARY`, `HISTORY`, `DOWNLOADS`, `FAVORITES` (star), `SAVED` (bookmark), `BROWSE`/`DISCOVER`, `ANIME`, `MOVIES`, `STUDIO`, `ASSISTANT`, `SETTINGS`, Studio phases (`READ`, `STORY`, `NARRATE`, `VIDEO`) and actions (`ADD`, `EDIT`, `DELETE`, `MORE`, `REFRESH`, `RETRY`, `FILTER`, `SORT`, `EXTERNAL`...).
- Old `"icons/x.svg"` paths still load (Lucide). The 40 paths gpui-component widgets use (chevrons, check, close, search, star...) are served as Phosphor **bold**, so widgets match the app.
- New icon: add it between the `vendor-phosphor` markers in `icons.rs` and run `native/scripts/vendor-phosphor.ps1`.

## Toasts

- `toast::show(cx, Toast::info|success|warning|error(msg))`, optional `.action(label, f)`, `.icon(ph)`, `.duration(d)`, `.sticky()`.
- One at a time at the bottom centre of the page area. 4 s, or 6 s with an action or for warnings and errors; hovering pauses. A newer toast replaces the current one after 1.5 s; the same message twice restarts the timer.
- The shell already mounts `toast::host(cx)`. A standalone window must add `.child(mv_ui::toast::host(cx))` to a `relative()` container.

## Confirmations

- `confirm_dialog` opens nothing focused, and with `danger = true` Enter does not confirm. Label buttons by outcome ("Delete chat" / "Keep chat"), never "OK".
- Use `confirm_inline` for row-level deletes and keep a `confirming` flag on your view.

## Replacing local helpers

| Local helper | Use instead |
|---|---|
| ui-manga `select_chip` | `filter_chip` |
| ui-manga `state_view` / `error_view`, mv_ui `empty_state` / `error_state` | `empty_view` / `error_view` (old ones still compile) |
| ui-assistant `menu_row`, `popup_surface`, `fmt_bytes` | `mv_ui::menu_row`, `popup_surface`, `fmt_bytes` |
| Hand-rolled toggles and stars (`icons/star.svg` tinted accent) | `favorite_star`, `icon_toggle`, `switch` |
| `notice_bar`, the download toast shell, ad-hoc notes | `toast::show` or `inline_alert` |
| `small_btn`, `cta`, `.flex_row_reverse()` icon hacks | `btn(..).small().icon(..)` |
| Grey boxes while loading | `skeleton_*` |
| `.tooltip(\|w, cx\| Tooltip::new("..").build(w, cx))` | `.tooltip(tip(".."))` |
| `.mb(px(20.))` around a toolbar | `page_title(..).action(..)` |
| ui-settings `card` / `row` / `divided` | `setting_section` / `setting_row` (+ `.border_t_1()`) |
| Raw hues for icon tiles, `netflix()` / `crunchy()` | `Tokens::hue(..)` / `hue_soft(..)`, or the preset `accent` |
| Hand-rolled score stars | `rating_badge(score, px)` |

## Guidelines behind the choices

Each component's doc comment names its source. The main ones: Material 3 state layers, filter
chips, segmented buttons, icon buttons, snackbars and cards; Fluent 2 iconography (filled =
selected), tokens, MessageBar and Toast; Apple HIG alerts, loading, keyboards and motion; NN/g on
empty states, error messages, confirmation dialogs, icon usability, skeleton screens, toggles and
checkboxes; WCAG 2.2 (1.4.1 use of colour, 1.4.11 non-text contrast, 2.2.1 timing, 2.4.7 focus
visible, 2.5.8 target size); Carbon's progress indicator for the stepper.
