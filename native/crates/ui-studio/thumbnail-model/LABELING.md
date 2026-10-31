# Thumbnail labelling

Each sheet image (sheets/sheet_NN.png) shows 9 YouTube thumbnails of manhwa/manga recap videos in a 3x3 grid.
Above each thumbnail is a black strip with its number "#N" in yellow. Label EVERY thumbnail on your sheets.

Write one JSON object per thumbnail, one per line (JSON Lines), with exactly these keys:

- "n": the number shown (int)
- "style": "ai_illustration" (one composed, polished painted/AI scene) | "manhwa_panel" (a raw comic panel/screenshot) | "collage" (several cut-out pieces/panels pasted together)
- "chars": number of visible characters, including background people (int, cap at 9)
- "face_size": size of the biggest face: "none" | "small" (< 1/6 of frame height) | "medium" (1/6 to 1/3) | "large" (> 1/3, a close-up)
- "expression": the focal character's expression: "calm" | "smirk" (confident/cocky) | "angry" (intense/shouting) | "shocked" | "scared" | "sad" (crying) | "happy" (laughing/smiling) | "blush" (embarrassed/flustered) | "none"
- "onlookers": true if other characters are reacting to / staring at the main character
- "women": true if one or more female characters are prominent
- "suggestive": true if revealing outfits / fan-service are prominent
- "text": the exact words printed on the thumbnail (not the video title), joined with " / " between separate captions; "" if none
- "text_words": total number of words in "text" (int)
- "text_color": main text colour: "yellow" | "white" | "red" | "other" | "none"
- "text_outline": true if the text has a thick dark outline/stroke or shadow
- "text_kind": main text's role: "label" (names something: "MC", "NEWBIE") | "exclamation" ("WHAT??") | "dialogue" (a character's line) | "stat" (a number/rank: "LV 999", "100°C", "SSS") | "headline" (summarises the story: "Betrayed / Revenge") | "none"
- "arrow": true if an arrow points at something
- "circle": true if something is circled/highlighted with a ring
- "glow_aura": true for visible energy effects: aura, lightning, fire magic, glowing weapon
- "glowing_eyes": true if a character's eyes glow
- "weapon": true if a weapon is visible and important
- "monster": true if a monster/beast/dragon is visible
- "scale_contrast": true if a small figure faces something huge (giant, army, dragon)
- "panels": 1 for a single scene, 2 for a split frame, 3+ for multi-panel
- "before_after": true if the image contrasts weak vs strong, then vs now, poor vs rich
- "system_ui": true if a game/status window, stat card or UI is shown
- "kneeling": true if people kneel/bow to someone
- "blood": true if blood or injuries are visible
- "humor": true if the framing is comedic/meme-like
- "romance": true if romance is the focus (couple, blushing, intimacy)
- "wealth": true if money, luxury, food abundance or modern riches are shown
- "setting": "dark" (cave, night, void) | "castle" (fantasy palace/court) | "nature" | "city" (modern) | "school" | "battle" | "plain" (flat/abstract) | "interior" | "other"
- "busy_background": true if the background is detailed/cluttered rather than simple or blurred
- "subject_pos": main subject position: "center" | "left" | "right" | "spread" (no single subject)
- "mood": "dark" | "bright" | "mixed"
- "colors": the two dominant colours as words, e.g. ["blue","purple"]

Be literal and consistent. Do not skip any thumbnail. Output nothing but the JSON lines into your file.
