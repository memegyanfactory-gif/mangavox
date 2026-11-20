# Anime: a manga chapter made into an anime episode

Crates: `native/crates/mv-create/src/anime.rs` and `anime/` (data, the director's prompts,
`pacing.rs`, `recipes.rs`, `checks.rs`, `board.rs`, `director.rs`), `native/crates/ui-creative`
(`anime_jobs.rs`, `anime_view.rs`, the Make an anime page), `python/anime_comp.py` (panels, keying,
camera, finish, animatic, edit). Format: what we measured, what the pipeline does about it, how it is
checked.

## What was measured

Two episodes of a TV anime were measured cut by cut against the eleven manga chapters they adapt
(every frame at 320x180: change per frame, camera estimate per cut, colour, loudness; subtitles for
dialogue timing; 841 manga panels cut with `anime_comp.py panels`). Tools and per-cut data:
`.claude/anime-reference/` (local, not in git).

### Pacing
| | Episode 1 | Episode 2 |
|---|---|---|
| Story length (no opening/ending) | 22.1 min | 20.5 min |
| Cuts | 395 | 334 |
| Cuts per minute | 17.9 | 16.3 |
| Cut length: mean / median / p90 | 3.4 s / 2.0 s / 7.5 s | 3.7 s / 2.8 s / 7.4 s |
| 1-3 frame impact flashes | 248 | 57 |

By scene type (mean cut length): action 1.9-3.1 s, dread 3.7-4.5 s, dialogue 4.1-5.5 s (p90 up to
12 s), narration 5-8 s, flashback 4.8 s. A quarter of all cuts are under 1 s but take only 4% of the
time; cuts over 7 s are 12% of cuts and 40% of the time.

### How much really moves
Share of screen time, dialogue scenes: a still drawing under a slow camera 21-25%, frozen hold 6%,
mouth flaps only 7-8%, pose-to-pose (two or three pose changes, with or without a camera move)
45-50%, continuous animation about 15%. Action scenes: continuous animation 52-64%.
- 45-58% of screen time has a camera move. Over still drawings the move is slow: about 2% of the
  frame width per second (10-13% over a cut), with almost no zoom (0-1% per cut; pushes 0.3-0.9%/s).
- Animated cuts are drawn mostly on threes (43-51%), then twos (15-21%), ones 15%.
- Mouths are often not shown at all while people talk: backs, wide shots with tiny figures,
  over-the-shoulder shots, lines heard over the listener's face. One 9 s pan across the group's backs
  carries eight lines; a 16 s wide shot carries seven.

### Dialogue
- 52-64% of cuts carry a line; those cuts last 4.8-5.0 s, the others 1.5-1.7 s.
- A speaking cut holds 1.8-2.4 lines on average; 16-30% hold three or more.
- The first line starts about 0.5-0.6 s after the cut; the cut usually comes right as a line ends.
  16-17% of lines run across a cut.
- 19-25% of lines are inner voice, narration or off screen. About 2.5 subtitle words per second.

### Colour and sound
- One colour key per place and lighting state: daylight scenes are nearly neutral (saturation 0.31);
  the statue hall is deep blue (#1a284c, luma 0.16, saturation 0.74) with the characters tinted to
  match; when the altar's fire lights, the same hall turns orange against blue. Episode 2 is 77% blue.
- Flashbacks: saturation 0.14, vignette, soft glow. Memories cut into a dark scene are bright and warm.
- Action is about 10 dB louder than dialogue (-20 dB against -27 to -30 dB); 2-3% near silence.

### Manga to episode
- Episode 1 covers chapter 1 to the middle of chapter 4 (about 220 used panels); episode 2 the rest
  of chapter 4 to chapter 10 (about 535 panels).
- Dialogue and drama: about one cut per panel, 3.5-4.5 s per panel, lines kept nearly word for word.
  Action and horror: 0.55 cuts per panel, about 2 s per panel (several small panels fuse into one cut).
- 39% of episode 1 is new material (a spectacle cold open, world-building scenes with authority
  figures, a fight the manga skipped); episode 2 only 8% (two short scenes of the hero's family).
- Cut: the manga's flash-forward opening, comedy and chibi beats, a small side plot, crowd size
  (17 hunters became 13). Changed: who pushes the group onward, so the leader stays sympathetic.
- Narration boxes became a narration montage with dissolves, or dialogue in a new scene. A tall panel
  of a character arriving became three inserts (body, hair, hand) before the face. A panel with
  several balloons became one long take. Iconic panels (the fist, the hospital bed) became held inserts.

### Camera language seen
Establishing in three steps (aerial, street, the site from a steep tilted high angle with strong
diagonals); "dirty" over-the-shoulder with a large soft foreground head; three depth layers in group
shots; low-angle hero against sky for resolve; extreme close-ups of eyes for realisation and terror,
of a mouth or a fist for shame and anger; symmetrical centre framing for the ominous statue; tiny
figures under a subject too big for the frame; a high angle through a dark foreground silhouette for
isolation; top-down tableau for a death; split-screen reaction panels; a recurring cutaway to a
"clock" object (the flames, six times); action frames with a tilted horizon, strong foreshortening,
speed-line backgrounds, smears and white flashes; both episodes end held on one image (eyes, a flame).

## What the pipeline does

The steps run in the order a studio works. Steps 1 to 4 need only the AI that reads and writes;
nothing is painted until the animatic has been watched and the user presses **Draw it**.

1. **Read the pages** (built). `anime_comp.py panels` cuts each page at its gutters (recursive X-Y
   cut on flat bands; tall webtoon strips at their flat bands), keeping each panel's size. A vision
   model reads the panels six at a time (`anime::read_panels`): framing, who is there and in what
   outfit, where, every balloon in English (thought, narration, System windows marked), sound
   effects, mood; logos, credits and blanks are skipped.
2. **Cast from the pages** (built, `board::cast_from_pages`). The characters and places come from
   what the panels show, merged with what Studio already knows of the manga. Labels for one person
   ("Kim", "Mr. Kim") become one character; the panels then use the cast's names everywhere.
3. **Scenes** (built, `board::scenes`). The chapter is divided into scenes: place, light, one colour
   grade, a kind (dialogue, action, dread, narration, flashback, montage) and a plan of what is kept,
   cut and added (`expansion`: faithful, cinematic, expanded). Each scene's length comes from the
   measured pacing (`pacing.rs`: 4 s per dialogue panel, 2 s per action panel, never shorter than its
   lines). A scene of more than 26 story panels is boarded in parts.
4. **Storyboard and check** (built, `board::storyboard_scene`, `checks.rs`, `director.rs`). Scene by
   scene the director writes cuts from 40 shot recipes (`recipes.rs`, the camera language above:
   aerial establishing, steep tilted high angle on the site, dirty over-the-shoulder, pan across the
   group's backs, extreme close-up of the eyes, too big for the frame, the clock cutaway, the lunge at
   the lens, the held last image...). Each cut has size, angle, lens, a drawable composition, the
   camera move and how far it goes, length, how much moves (hold, camera, lip, pose, full), who is on
   which side of the frame and facing where, its lines and whether a mouth is seen, effects, colour
   grade and transition. What needs no judgment is put right (a cut too short for its lines, a mouth
   that cannot be seen); the episode director's check then finds what breaks film grammar or the
   measured pacing: jump cuts, crossing the line, eyelines that do not meet, a new place never shown
   wide, rushed or dragging scenes, too much or too little animation for the kind of scene, dropped
   dialogue, a weak ending. Faulted scenes go back to the director twice at most; a revision is kept
   only when it is better.
5. **Animatic** (built). Every cut is voiced (each character's own voice) and shown from the manga
   panel it adapts under its camera move, at the measured speeds (a slow drift of about 2% of the
   frame per second in talk; a tall panel travelled end to end under a tilt), with a slate; a cut the
   director added shows its description on a card. The cuts are joined with their transitions; a
   cut's voice sets its final length.
6. **Designs, backgrounds, effects** (built). Design and expression sheets for everyone the board
   shows (from their own manga face); one plate per place, view and light; effects painted on black.
7. **Cuts** (built). The characters drawn on a chroma screen over the plate's light (green, magenta
   for green-haired characters), animated by the video model when the cut is lip, pose or full,
   keyed and composited: plate, characters and effects through one camera, colour match, light wrap,
   contact shadow, drawings held on threes, then the shared finish (bloom, diffusion, grade, grain,
   chromatic aberration, vignette, dust, light rays, speed lines, flash).
8. **Episode** (built). The finished cuts edited to the frame: hard cuts joined without re-encoding,
   soft ones cross-faded, voices half a second into their cut, music ducked under speech.

Still to do: compose the whole frame first and derive the cel from it (so scale and perspective
match), check each drawn frame against the design sheets with a vision model, mouth flaps and
pose-to-pose in the compositor instead of a video model, focus blur by shot size and parallax from a
depth map, on-screen text (system windows, captions) drawn by the app, sound effects and music cues
per scene, and voices batched per scene (one voice-model start per scene, not per cut).

## How it is checked
- `cargo test -p mv-create` (engine: vocabulary, reading, cast, scenes, boards, checks, the director
  loop with a scripted AI), `cargo test -p ui-creative anime` (stages, merging),
  `python -m unittest tests.test_anime_comp` (panels, camera, keying, animatic views, edit timing).
- `cargo run -p mv-create --example anime_board -- "<title>" <chapter> "<pages folder>"` runs the
  director without the window on a downloaded chapter, prints the scenes, the check's notes and the
  board, and composites a silent animatic.
- The measuring tools (`.claude/anime-reference/`) can run on our own episode, to compare cut
  length by scene type, camera moves, how much moves, colour keys and loudness with the reference.

## Research
`.claude/anime-reference/research/` holds five verified-source reports (shot sizes, angles and
lenses; composition and staging; continuity and editing; movement, suspense and action; adaptation,
colour and sound) and the anime-craft report; `rulebook.json` and `rulebook.md` merge them into
rules, shot recipes and checks for the director.
