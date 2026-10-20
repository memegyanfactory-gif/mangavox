# Assistant (chat) design changes

Crate: `native/crates/ui-assistant`. Source audit: `audit-chat.json`. Every line is one change: **what**, why, reference.

## Whole crate

- **Every icon is Phosphor (`ph::*` / `glyphs::*`), no Lucide `icons/*.svg` left**; fill for on/selected, bold at 14px and below. One family, weight follows state. https://fluent2.microsoft.design/iconography
- **No raw colour literals (was 18)**; all from `Tokens`, enforced by `tests/no_raw_colors.rs`, which also fails on any Lucide path. Fixed palette only. https://www.refactoringui.com/previews/building-your-color-palette
- **Local `menu_row`, `popup_surface`, `fmt_bytes`, `tag` and the hand-rolled buttons, chips, toggle, spinner and progress bar are replaced by kit components.** One look and one set of states everywhere. https://www.nngroup.com/articles/consistency-and-standards/
- **Sizes and radii come from `theme::icon` / `theme::radius`, and raw 13px text is now `size::SM`.** One type and shape scale. https://www.nngroup.com/articles/consistency-and-standards/
- **The last off-scale icon sizes are on the scale too: brand logos and family tiles 18 → 20 (`LG`), the usage logo 13 → 14 (`SM`), the Favourites provider mark 11 → 12 (`XS`), the empty-chat hero 28 → 24 (`XL`), the usage ring 16 (`MD`); the attachment remove target grows 18 → 20px with a 12px glyph.** Fluent 2 sizes icons on a fixed ramp. https://fluent2.microsoft.design/iconography
- **Status pills show an icon and a word (Ready, Needs key, Offline...) in a tonal fill.** Status never rests on colour alone. https://www.w3.org/WAI/WCAG22/Understanding/use-of-color
- **llama.cpp shows a CPU glyph instead of Ollama's llama.** Two identical marks could not be told apart. https://www.nngroup.com/articles/icon-usability/

## Dock: header and chat history

- **The history toggle is `sidebar-simple`, shown selected (fill and active background) while open; New chat is a compose glyph.** One glyph per meaning, and the toggle shows its state. https://www.nngroup.com/articles/icon-usability/
- **Every icon button has a tooltip: history, New chat (Ctrl+N), Rename, Delete chat, Attach, Send / Stop, Jump to latest, Regenerate, Copy.** Icon-only controls need a label. https://www.nngroup.com/articles/tooltip-guidelines/
- **The drawer covers the whole dock, with a "Chats" header, New chat, Close (Esc), a scrim that closes on click, and a 180ms slide-in (static under reduced motion).** Matches Claude and ChatGPT history panels. https://lawsofux.com/jakobs-law/
- **Chats are grouped as Today / Yesterday / Previous 7 days / Previous 30 days / Older, by local day.** Scannable history. https://lawsofux.com/jakobs-law/
- **The search field has a magnifying-glass prefix.** A recognisable search affordance. https://www.nngroup.com/articles/icon-usability/
- **Rows use `list_row`, so the selected chat stays highlighted under the pointer.** Hover is a layer on top of selection. https://m3.material.io/foundations/interaction/states/state-layers
- **Hover actions (Rename, Delete) sit on a fade of the row colour, so they no longer print over the title.** https://lawsofux.com/jakobs-law/
- **Delete is immediate with an Undo toast; it no longer arms silently on the first click.** Prefer undo to confirmation for reversible actions. https://www.nngroup.com/articles/confirmation-dialog/
- **Rename: Enter or click-away saves, Esc cancels and restores the old title.** User control. https://www.nngroup.com/articles/user-control-and-freedom/
- **Empty history shows an icon, a line of guidance and "Start a chat".** https://www.nngroup.com/articles/empty-state-interface-design/
- **A chat with a running reply shows a spinner in its row, and you can switch chats while it streams.** The click was a silent no-op before. https://www.nngroup.com/articles/visibility-system-status/

## Dock: first run and empty chat

- **No model yet: a setup card ("Connect a model" with Download a local model / Add an API key, or "Choose a model" plus one chip per ready provider) replaces "How can I help?".** The old flow ended in an error bubble. https://www.nngroup.com/articles/empty-state-interface-design/
- **Sending without a model opens the picker; no error is stored in the chat.** Avoid dead ends. https://www.nngroup.com/articles/user-control-and-freedom/
- **While providers are detected, a spinner and "Looking for AI on this PC…" replace a blank view.** https://www.nngroup.com/articles/progress-indicators/
- **Suggestions have an accent glyph each and an arrow that appears on hover.** Starter prompts show their outcome. https://www.shapeof.ai/patterns/suggestions
- **The placeholder is "Ask about your library, find anime, or attach a panel…", replacing a coding-agent string.** https://www.nngroup.com/articles/empty-state-interface-design/

## Dock: messages

- **Each answer has Copy, Regenerate (on the latest) and the model that answered; older answers show these on hover.** https://www.shapeof.ai/patterns/regenerate
- **Your messages have Copy and Edit on hover (Edit loads the last message back into the composer), and the bubble uses `surface_2`.** https://www.shapeof.ai/patterns/regenerate
- **Stick-to-bottom: scrolling up stops auto-follow, and a round "Jump to latest" button brings it back.** Streaming no longer yanks a reader down. https://github.com/stackblitz-labs/use-stick-to-bottom
- **"Thinking…" is shimmer text; the reasoning header has a brain icon and becomes "Thought for 12s".** Moving feedback during 2-30s waits. https://www.nngroup.com/articles/progress-indicators/
- **Collapsible headers light up icon and caret together, and remember your toggle (no XOR that reopened Activity when a reply finished).** https://www.nngroup.com/articles/user-control-and-freedom/
- **Tool steps are worded in the right tense: "Opening Library…" while running, "Opened Library" when done, "Couldn't open…" on error.** https://www.shapeof.ai/patterns/stream-of-thought
- **Tool steps show a spinner while running, a tool-specific icon when done, a green check, Declined as a tag, and a caret only when there is a result to open.** No false affordance. https://www.nngroup.com/articles/visibility-system-status/
- **Approval cards ask "Open Library?" (imperative) with a "Changes the app" / "Can't be undone" badge.** The old card used a past-tense label and a lowercase risk word. https://www.shapeof.ai/patterns/verification
- **Approval cards have Allow (✓, Enter; red when destructive), Deny (✕, Esc) and "Always allow in this chat"; raw arguments show only for unknown tools.** Friction matches risk. https://www.shapeof.ai/patterns/verification
- **An approval moves keyboard focus to the dock when nothing in the window has it, so Enter and Esc work at once (narrowed in the review pass below: it never takes focus from another field).** Kit buttons no longer take focus on click. https://www.w3.org/WAI/WCAG22/Understanding/keyboard
- **Errors are mapped to plain language ("Couldn't reach Ollama", "OpenAI didn't accept the API key", ...) with the raw text under Details and a Retry button with an icon.** https://www.nngroup.com/articles/error-message-guidelines/
- **Stopped shows an icon and a "Continue" button.** https://www.nngroup.com/articles/user-control-and-freedom/

## Dock: composer

- **Enter sends and Shift+Enter adds a new line; key-cap hints show under the box while it has focus ("Esc to stop" while a reply runs).** The space is reserved, so the list does not jump. https://developer.apple.com/design/human-interface-guidelines/keyboards
- **The border turns to `accent_border` while the text box has focus; colours come from tokens.** https://www.w3.org/WAI/WCAG22/Understanding/focus-visible.html
- **Esc closes the topmost thing first: a rename, then the drawer (the composer gets the keyboard back), then the approval on screen (Deny), then the running reply (Stop). Ctrl+N starts a new chat.** Esc used to stop a reply hidden behind the open drawer. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- **The send button is round accent with hover and pressed states, a Send (Enter) / Stop (Esc) tooltip, and a disabled reason ("Type a message", "OpenAI needs an API key").** https://www.smashingmagazine.com/2024/05/hidden-vs-disabled-ux/
- **The effort chip is a kit menu with a brain icon and "Reasoning effort" tooltip; its items have icons (lightning / gauge / brain) and a check. It is hidden for providers that ignore effort.** https://www.shapeof.ai/patterns/verification
- **The access chip has distinct icons (lock = Ask, shield-check = Auto, shield-slash = Full) inside the chip, and Full shows in warning colour.** Turned-off verification is flagged. https://www.shapeof.ai/patterns/verification
- **Attachments show as 52px thumbnails with a remove button, and can be dropped on the box or pasted (screenshots are saved); a toast explains skipped non-image files.** https://www.shapeof.ai/patterns/attachments
- **A provider that isn't ready shows an inline warning above the box with "Fix in Settings" or "Re-check".** The old warning was a bare yellow line. https://www.nngroup.com/articles/error-message-guidelines/

## Model picker

- **The trigger is 140px wide, has a "{provider} · {model id}" tooltip and uses `size::SM`; with no model it reads "Choose a model" in accent.** https://www.nngroup.com/articles/tooltip-guidelines/
- **Every rail logo has a provider-name tooltip and a selected state that stays under hover.** https://www.nngroup.com/articles/icon-usability/
- **The Favourites rail star is filled gold when favourites exist and an outline when there are none.** https://www.nngroup.com/articles/consistency-and-standards/
- **One favourite star (`favorite_star`, filled gold when on) replaces two mismatched star shapes; unstarred rows show it only on hover.** https://api.flutter.dev/flutter/material/IconButton-class.html
- **The current model has a trailing accent check, and rows use `list_row`.** https://m3.material.io/components/lists/guidelines
- **Up / Down / Enter move through the list and pick (a keystroke interceptor, because the search field's own actions run first); the pointer moves the highlight too.** Raycast / VS Code quick pick. https://lawsofux.com/hicks-law/
- **The Ctrl+N hint appears only on the highlighted or hovered row.** https://lawsofux.com/hicks-law/
- **Second line: context, parameters and size (the provider name only in Favourites), or the raw id when nothing else helps.** https://www.shapeof.ai/patterns/model-management
- **States: skeleton rows while loading, "No models found for X" with Refresh, "No favourites yet", "No models match", and an inline alert with "Set up" when the provider isn't ready.** https://www.nngroup.com/articles/skeleton-screens/
- **The header shows the provider name, status pill and a Refresh icon button.** https://www.nngroup.com/articles/visibility-system-status/
- **The custom model id sits behind "Add a model id", says "Adds to {provider}", and is disabled on Favourites.** https://www.nngroup.com/articles/progressive-disclosure/
- **The popover fades in.** https://m3.material.io/styles/motion/overview
- **Closing the popover (pick, Enter, Esc) gives the keyboard back to what had it, usually the composer, so you can type straight after choosing a model; Esc in the custom id field returns to search.** https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/

## Usage meter

- **The ring and bars use the accent below 75%, then warning and danger.** The off-palette blue is gone. https://www.refactoringui.com/previews/building-your-color-palette
- **The trigger has a tooltip ("Usage limits · 42% of the session limit used"), a focus ring, and fades in when data arrives.** https://www.nngroup.com/articles/tooltip-guidelines/
- **The card title has a gauge icon; the ring's track uses `Tokens::track()` for 3:1 contrast.** https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast
- **The limit bars in the card are the kit `usage_bar` (one segment in the usage colour) instead of a hand-rolled bar.** One bar component app-wide. https://www.nngroup.com/articles/consistency-and-standards/
- **The card takes the keyboard when it opens, Esc closes it, and focus goes back to where it was.** https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/

## Settings → AI providers

- **Cards collapse to a 56px row (logo, name, In use tag, status, switch, caret); only the provider in use opens by default.** https://www.nngroup.com/articles/progressive-disclosure/
- **The on/off control is the kit `switch` with a "Turn on/off {label}" tooltip, replacing a hand-rolled div.** https://fluent2.microsoft.design/components/web/react/core/switch/usage
- **Server URL and default model save on Enter or click-away, only when changed, with a toast; the three Save buttons are gone.** https://www.nngroup.com/articles/progressive-disclosure/
- **The API key has Save key (key icon, or Enter) and Remove (danger ghost), and "Get an API key" is an external link.** https://www.nngroup.com/articles/icon-usability/
- **One primary button: "Use in assistant" (sparkle) only on an expanded, ready card that isn't in use; it is disabled with a reason when not ready.** https://m3.material.io/components/all-buttons
- **The test result shows an icon and "Connected · 420 ms" (no "0 ms"), or a short failure with the full message in a tooltip; Test spins while running.** https://www.nngroup.com/articles/visibility-system-status/
- **Save and remove notes are toasts, so they no longer hide test results.** https://m3.material.io/components/snackbar/guidelines
- **Group headers have an icon (desktop / terminal / cloud) and "2 of 3 ready".** https://www.nngroup.com/articles/consistency-and-standards/
- **The install hint shows a copyable mono command and a "Download {app}" link for Ollama, LM Studio and llama.cpp.** https://www.nngroup.com/articles/error-message-guidelines/
- **Skeleton cards show while detecting, and an empty state when nothing is found.** https://www.nngroup.com/articles/skeleton-screens/

## Settings → Model hub

- **Tabs are a segmented control with icons (Picks / Search / Downloaded).** https://m3.material.io/components/segmented-buttons/overview
- **The seven filters are behind one "Filters · n" button that opens a popover of filter chips with icons and Clear.** https://lawsofux.com/hicks-law/
- **The filter popover takes the keyboard when it opens (Tab reaches the chips), Esc closes it, and focus goes back to where it was (the search field, usually).** https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- **The search field is pinned above the results.** https://lawsofux.com/hicks-law/
- **Rows use `list_row` with a family mark (OpenAI, DeepSeek or Mistral logo, else a model glyph in a per-family hue), replacing the uploader's initial.** https://github.com/lobehub/lobe-icons
- **Download and like counts use Phosphor icons instead of Unicode ↓ / ♥.** https://www.nngroup.com/articles/consistency-and-standards/
- **The verified badge is a filled seal with a "Built by a trusted quantizer" tooltip.** https://www.nngroup.com/articles/tooltip-guidelines/
- **Loading uses skeleton rows; "Couldn't reach Hugging Face" has Try again; search errors have Retry; "Nothing matches these filters" has Clear filters.** https://www.nngroup.com/articles/empty-state-interface-design/
- **Downloaded models: "Use in chat" (primary) once a model is in Ollama, an In use tag, and "Add to Ollama" with a spinner while importing.** https://www.nngroup.com/articles/user-control-and-freedom/
- **Without Ollama: a warning with "Get Ollama" plus Re-check, instead of a failing import.** https://www.nngroup.com/articles/user-control-and-freedom/
- **Delete file is a danger icon button with an inline confirm ("Delete this file (4.7 GB)? It can't be undone." Delete file / Keep) and a toast after.** https://www.nngroup.com/articles/confirmation-dialog/
- **Link download and the token moved under "Add from a link or token" below the models; Enter in the link field downloads.** https://www.nngroup.com/articles/progressive-disclosure/
- **Right pane: an empty state with the hardware summary, a skeleton while loading, an error with Retry, and an external Hugging Face link.** https://www.nngroup.com/articles/skeleton-screens/
- **Quant rows: a Download / Cancel / Downloaded tag with icons, a fit icon (check / gauge / hourglass / x) with its detail as a tooltip, a sparkle Recommended tag, the kit `progress_bar`, and a caret on "Show all".** https://www.nngroup.com/articles/progress-indicators/
- **The hub's grey note banner is gone: toasts by severity, plus a toast when a download finishes.** https://www.nngroup.com/articles/indicators-validations-notifications/

## Audit items skipped or changed

- **Capability icons (vision, reasoning, tools) on picker rows: skipped.** mv-ai reports no per-model capabilities, and tool support is per provider, so the icon would repeat on every row or guess wrong. The rows show context, parameters and size instead.
- **Disabling the paperclip for models without vision: skipped** for the same reason. Disabling on a guess would block models that can read images.
- **"Use with llama.cpp" for downloaded files: skipped.** llama-server serves only the model it was started with, so changing the model id does nothing.
- **Distinct llama.cpp, Hugging Face, Qwen, Gemma, Meta and GLM brand marks: not added.** The assets live in mv-ui (not this crate) and need downloading. A CPU glyph and per-family tinted tiles are used until then.
- **Selectable user-message text: skipped.** The kit has no plain selectable text, and Markdown would reformat what you typed. Copy is offered on hover instead.
- **Count badge on the Downloaded tab: skipped.** The 340px segmented control has no room for it. The tab shows "N on this PC · size" as its first line instead.
- **Disabling Send for every not-ready provider: changed.** Send is blocked only for Needs key, Not installed and Disabled. Offline and Error can be stale, so they get a Re-check action.
- **Hints in the effort and access menus: shortened into the item labels** ("Ask: approve every action"), because kit `MenuItem` has no second line.
- **Reserving the usage meter's slot with a 0% ring: replaced by a fade-in.** A 0% ring for providers with no limits (Ollama) would be noise.
- **The audit's turtle (Slow fit) and flask (Distilled) icons: substituted** with hourglass and feather from the vendored set. Adding new glyphs is an mv-ui change.
- **Usage limit bars on a 3:1 track: traded for the kit bar.** `usage_bar` draws its track in `Tokens::active()`; the ring keeps `Tokens::track()`. Asked mv-ui to switch `usage_bar` (and `progress_bar`) tracks to `track()` so every bar meets WCAG 1.4.11.
- **Esc and keyboard navigation inside the menus (effort, access): left to the kit.** They are gpui-component popup menus, which already handle Up / Down / Enter / Esc.

## Review pass (adversarial)

- **Clicking the model button while the picker is open now closes it.** The outside press that dismissed it used to reopen it on the same click, so the button could never close the popover. https://www.w3.org/WAI/ARIA/apg/patterns/menu-button/
- **An approval card takes the keyboard only when nothing else has focus.** It used to pull focus out of any field elsewhere in the app, so an Enter typed in a page search could approve an action nobody had read. https://www.w3.org/WAI/WCAG22/Understanding/on-focus
- **Enter never answers an approval hidden behind the open chat drawer,** from the dock or from an empty composer. https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum
- **A rename is kept when the drawer closes, another chat is clicked or a second rename starts; only Esc cancels it.** Kit controls don't move focus, so the old save-on-blur rarely fired and the typed name was thrown away. https://www.nngroup.com/articles/user-control-and-freedom/
- **Closing the drawer (Close, scrim or Esc) gives the keyboard back to the composer.** The scrim press no longer lets the dock root take focus. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- **Choosing a model from the setup card, a ready-provider chip or Send-with-no-model leaves the keyboard in the composer,** so the first message can be typed straight away. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- **Auto-follow comes back only after a scroll down to the bottom.** A small touchpad scroll up (still within 24px of the bottom) was undone on the next frame while a reply streamed. https://github.com/stackblitz-labs/use-stick-to-bottom
- **Edit on your last message offers Undo in a toast (until something new is sent in that chat),** because it removes the reply at once. https://www.nngroup.com/articles/user-control-and-freedom/
- **Providers: Test connection and Use in assistant first save a pasted key and any URL or default model typed without Enter.** The test used to run on the old values, because clicking a kit button doesn't blur the field. https://www.nngroup.com/articles/web-form-design/
- **Model hub: the trusted-quantizer seal follows the uploader.** A pick that fell back to an untrusted repo no longer claims it, and a trusted search result now shows it. https://www.nngroup.com/articles/trustworthy-design/
- **Model hub: the Ollama warning says what is actually wrong,** turned off ("Open AI providers"), not running (Re-check) or not installed ("Get Ollama"). The disabled "Add to Ollama" gives the matching reason. https://www.nngroup.com/articles/error-message-guidelines/

### Left as is (review pass)

- **The usage card and hub Filters still guard reopening with a 250 ms timer,** where the picker now uses the press position. A normal click works; a press held longer than 250 ms on the trigger reopens them. Worth folding into one `popup_surface` helper in mv-ui.
- **The "Full access" chip colours its label in warning, but its icon stays grey.** `menu_button` has no icon-colour option; that needs a `MenuButton::icon_color` in mv-ui.

## Review pass 2 (adversarial)

- **A chat you switch to while another chat's reply streams no longer shows Stop or stops that reply with Esc; Send is disabled with "Wait for the reply in “{chat}” to finish", and Enter or a suggestion says so in a toast instead of doing nothing; New chat and Regenerate name that chat in their disabled reason.** The Stop button and Esc acted on a reply that wasn't on screen. https://www.nngroup.com/articles/visibility-system-status/
- **Changing effort or access in one chat no longer overwrites the running reply's access with the visible chat's mode.** It silently undid "Always allow in this chat" for the chat that was streaming. https://www.shapeof.ai/patterns/verification
- **An approval asked in a chat you switched away from shows a warning toast with "Show", and that chat's history row shows a shield instead of a spinner.** The reply otherwise waited for an answer nobody could see. https://www.nngroup.com/articles/visibility-system-status/
- **Rename: Enter keeps the keyboard in the drawer.** It was left on the field that Enter removed, so Esc no longer closed the drawer. https://www.w3.org/WAI/WCAG22/Understanding/focus-order
- **"Jump to latest" goes away whenever the list is at its very end, however it got there (scrollbar drag, keyboard), not only after a wheel scroll down.** The button could sit over a list that was already at the bottom. https://github.com/stackblitz-labs/use-stick-to-bottom
- **The empty chat and setup card are centred with auto margins instead of `justify_center` in the scroll container.** In a short dock the tall setup card was cut off at the top, where scrolling can't reach (safe alignment). https://developer.mozilla.org/en-US/docs/Web/CSS/justify-content#safe
- **Picker: a pick or Esc gives the keyboard back (to the composer, usually) even when Tab had moved it onto a model row or rail logo.** Focus was left on a closed popover, so dock shortcuts stopped working. https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- **Picker: the Up/Down highlight shows on the current model's row too (one state layer stronger).** Landing on it looked like the highlight had vanished. https://m3.material.io/foundations/interaction/states/state-layers
- **AI providers: a collapsed card for a local app that isn't installed (Ollama, LM Studio, llama.cpp) offers "Get {app}" without opening it** (audit item 8; the copyable install command stays inside). https://www.nngroup.com/articles/progressive-disclosure/

### Left as is (review pass 2)

- **One reply at a time across chats.** The dock keeps a single run, so the other chat waits rather than streaming in parallel; parallel runs need per-thread run state.
- **A keyboard user who Tabs from the picker's search into the rail can't use Up/Down there** (they drive the search field's list only); Tab, Enter and Esc work.
