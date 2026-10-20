# Music: scoring like a recap channel

Crates: `native/crates/mv-score` (engine: spot, compose, synth, mix, demos) and the Music step in
`native/crates/ui-studio/src/music.rs`. Format: what we learned (with sources), what the engine does
about it, and how it was checked.

The complaint was that the generated music did not sound like the music under manhwa / manga
recap videos on YouTube. The old engine wrote film-score cues (Action, Epic, Tense, Calm,
Romance...), mostly as orchestral sketches. Recap channels use a few recognisable genres instead,
switch them per arc, and lean on transition sound effects.

## Research

**Caveat.** No creator interview or how-to says exactly what recap channels use. The genre-per-scene
mapping below combines edit-culture writing, music-library pages and mixing guides. Treat it as an
informed inference, not documented practice.

### Genres per scene
- **Fights, cocky or badass moments: drift phonk / trap.**
  - Drift phonk spread from Russian drift TikToks into gym, edit and "sigma" content, built around the beat drop. https://www.esquireindia.co.in/amp/story/culture/books-and-music/how-russian-drift-culture-made-the-soundtrack-for-internet-looksmaxxers
  - Most of what streaming playlists call "phonk" is drift phonk. https://uppbeat.io/blog/music/what-is-phonk-music
  - Anime edits are cut to the beat, and fights are their highlight. https://sabukaru.online/articles/anime-music-videos-the-subcultural-phenomenon-that-is-taking-music-and-anime-to-another-level
- **Power-ups, climaxes, big reveals: epic / hybrid trailer music.** Staccato string ostinatos, braams, taiko, choir and brass. https://ende.app/en/song/13738-cinema-blockbuster-trailer-76 , https://www.pippit.ai/templates/movie-recap-music-background
- **Emotional scenes: piano leads, strings swell in later.** https://www.shockwave-sound.com/browse/mood/sad-sorrowful-mournful
- **Tension, mystery, horror: dark ambient.** Sub swells, evolving pads, sparse reverberant piano, reverse swells at transitions. https://4elementsmusic.com/album/4EM-097/Dark-Drama-Atmospheric-Menace-2
- **Comedy, slice of life: lo-fi.** Relaxed tempo, jazzy chords, vinyl crackle. https://www.epidemicsound.com/fr/music/genres/lo-fi-hip-hop/
- **Why generate locally:** official OSTs (for example Solo Leveling's) trigger Content ID claims that take the revenue and can escalate to strikes. https://vidiq.com/es/blog/post/can-you-monetize-anime-clips-recaps-gameplay-youtube/ , https://animecorner.me/milan-records-releases-solo-leveling-original-soundtrack/

### Tempo
- **Phonk:** sources disagree. One gives 90–120 BPM. https://blog.native-instruments.com/phonk-music/ Others give 130–160. https://beatstorapon.com/blog/how-to-make-a-phonk-beat/ , https://melodics.com/blog/producers-guide-to-phonk-music
  - These describe the same groove: 130–160 counted in full time with a half-time snare.
- **Trap:** 130–170 with a half-time feel (median 140). https://www.mixgraph.io/bpm-for/trap-hip-hop
- **Drill, lo-fi, ballad and ambient:** drill 140–145, lo-fi 70–90, ballads 60–80, ambient with no clear pulse. https://orphiq.com/resources/bpm-tempo-guide
- **Trailer music:** hybrid action 120–160, epic orchestral 100–150, emotional 70–110. https://www.tunee.ai/zh-HK/music-generator/trailer-music

### Production signatures
- **Phonk:** a pitched 808 cowbell melody in Phrygian or minor, a saturated 808 kept mono, velocity-varied rolling hats. https://blog.native-instruments.com/phonk-music/
  - Also 808 glides and slides, and the 808 following the cowbell line. https://beatstorapon.com/blog/how-to-make-a-phonk-beat/
- **Hybrid trailer:** synth bass and processed drums under strings, brass and choir, ticking pulses, and a riser into the drop. https://suno.com/playlist/c3fdb73f-36df-4e2c-8c87-9e2abbab6c4a

### When the music changes, and the SFX
- **Track changes:** change at a new arc or a dramatic spike. https://subscribr.ai/youtube-strategy/storytelling-video-editing-youtube
  - Silence before a reveal makes the hit land harder, and pulling music out leaves a key line exposed. https://invideo.io/faq/when-should-i-use-silence-instead-of-music-or-sound-effects/ , https://krotos.studio/blog/no-sound-in-sound-design
- **Recap editing specs:** cuts paced to the narration, music under the voice-over, separate stems. https://www.freelancer.de/projects/video-post-editing/manhwa-recap-video-editing
- **Whooshes:** the "workhorse" of transitions. **Riser into impact:** the build into a reveal. https://jackrighteous.com/en-gb/blogs/ai-art-visuals-creatives/cinematic-hits-and-transitions-suno-sounds-guide
- **Sub drop:** pairs with a downbeat or hit. https://sounddesign.irpr.agency/glossary/sub-drop/
- **Vine boom:** shock; heavy use reads as ironic. https://knowyourmeme.com/memes/vine-thud-boom-sound-effect
- **Heartbeat:** slow means dread, and a single thud works on a reveal. https://morphic.com/resources/sounds/tension-heartbeat-sound-effects
- **Record scratch:** a comedic full stop. https://morphic.com/resources/sounds/comedy-sound-effects
- **Manhwa-specific:** layered sword clashes and auras. https://www.freelancer.de/projects/audio-editing/enhance-lookism-video-sound-effects

### Loudness against the narration
- **Music level:** -18 to -25 dB under narration, -12 to -14 dB with no voice. Ducking ramps over 0.5–1 s, and cutting 3–6 dB at 800 Hz–3 kHz clears the voice. https://vidpros.com/fix-background-music-too-loud-video/
- **Ducking depth:** 15–20 dB under talking in one guide. https://sounddesign.irpr.agency/guides/how-to-mix-audio-for-youtube/
  - In Premiere, auto-duck at -12 dB is subtle and -18 dB is noticeable. https://www.freepik.com/blog/how-to-use-auto-ducking-in-premiere-pro-for-clean-professional-audio
- **YouTube normalization:** louder than -14 LUFS is turned down, quieter is never turned up. https://productionadvice.co.uk/stats-for-nerds/ , https://www.meterplugs.com/blog/2019/09/18/youtube-changes-loudness-reference-to-14-lufs.html

## What the engine does now (mv-score)

### Genre presets (`Style`)
Seven presets. Old cue names in `music.json` still load (serde aliases), and old `"sfx": true` reads
as Subtle.

| Preset | Used for | Tempo | Mode | Writing |
|---|---|---|---|---|
| Phonk Fight | fights, cocky / badass pages | 130–150 (half-time drums) | Phrygian | kick + distorted 808 that follows it and slides; clap + snare on beat 3; 8th/16th hats with triplet and 32nd rolls; open hats; a two-bar pitched-cowbell riff; dark root-fifth pad; a half-time music-box line at higher energy |
| Epic Orchestral | power-ups, climaxes, big reveals | 92–124 | harmonic minor | galloping string ostinato (GM strings, synth pulse an octave down at the peak); slow strings, then choir, then a supersaw layer as the energy rises; horn theme with brass doubling at the peak; taiko groove, timpani, toms and snare fills; 808 kick and backbeat at full energy |
| Dark Tension | danger, schemes, high-intensity dialogue | 84–100 | minor | 8th/16th synth pulse, drone, ticking rim, sparse low piano (sometimes the flat 2nd), 808 kick pulse, tremolo minor-second cluster at the top |
| Horror Drone | monsters, dread, gore | 60–70 (no pulse) | Phrygian | drone, a tritone pad, sub, tremolo minor seconds, a detuned music box, heartbeats as the dread rises, reverse swells |
| Sad Piano | grief, romance, flashbacks | 62–74 | minor (major when the pages are warm or hopeful) | broken-chord piano, the theme on top, strings entering after the first phrase, cello, violin doubling at the peak |
| Lo-fi Light | comedy, daily life | 76–90, swung 8ths | major / Dorian | Rhodes 7th and 9th chords with a strum; swung boom-bap with rim layering; round bass with approach notes; vinyl crackle; a marimba or vibes theme |
| Mystery | secrets, investigation, neutral narration | 84–98 | harmonic minor | pizzicato ostinato, celesta theme, dark strings, walking bass, shaker and rim |

- **Theme:** one chapter theme (question and answer phrase) runs through every genre, so the score sounds like one piece.
- **Energy layers:** layers enter with the energy curve (sparse below 3.5, groove to 6.5, full above).
- **Intros and builds:** after a soft change, the groove waits a bar or two. The last bar before an impact builds with a snare or tom roll, and the 808 drops out.

### Spotting (`spot.rs`)
- **Scene to preset:**
  - Fight pages are Phonk, or Epic for power-ups or intensity 9+.
  - Reveals are Epic at 7+, Tension at 5+, Mystery below.
  - Horror words map to Horror; ominous words to Tension; sad, love and nostalgic words to Sad; comedy and daily-life words to Lo-fi.
  - Neutral narration gets Mystery, or Tension at intensity 7+.
- **Cue grouping:** one bed per arc, not per page. A change of family needs 20 s, a change within a family 40 s; fights breaking out and well-spaced reveals cut in sooner. Cues are at most 90 s.
- **Transitions:**
  - Crossfade of 1.5 s.
  - Into a reveal: a riser and reverse cymbal, 0.9 s of silence with a single heartbeat, then a braam + boom + sub drop on the downbeat.
  - Into a fight: a hard cut onto an impact.
  - Into lo-fi after an intense cue: the music tape-stops over 0.7 s (a real slow-down of the mix bus) with a record-stop sound.
- **User control:**
  - The chapter genre (`MusicSettings.genre`) replaces every cue's preset; per-cue overrides win over it.
  - The energy setting shifts every page's energy by ±3, which changes tempo, layers and drums.

### Synth (`synth.rs`)
No samples; everything is generated.
- **New voices:** supersaw (7 detuned saws plus a sub octave), distorted 808 with glides (`Note.bend`), the TR-808 cowbell (two squares at a 1:1.48 ratio, band-passed), FM Rhodes, a formant "aah" choir, a pulse bass, and bowed tremolo.
- **New hits:** 808 kick, rim, braam, cinematic boom, glitch, record stop, sword clash, vinyl crackle, a multi-burst clap, and a riser with a gliding saw stack and accelerating tremolo.
- **With the orchestra SoundFont (GeneralUser GS) installed:** strings 48/49, tremolo strings 44, pizzicato 45, choir 52, horn 60, brass 61, timpani 47, piano 0, Rhodes 4, celesta 8, music box 10, vibes 11, marimba 12, basses 32/33/43, cello 42, violin 40.
- **Aliasing fixes:** the FM bell tames its index on high notes, the Karplus-Strong pluck uses smoothed noise, and the glitch and zap sounds are band-limited. Before these, the mystery demo measured +2.2 dBTP.

### Mix (`mix.rs`)
- **Per-clip balance:** every musical clip is loudness-matched to its role's place in its genre's mix. Phonk puts drums and 808 at -19 LUFS and pads at -28; Sad puts the piano at -20; and so on. Energy moves a cue ±3 dB.
- **Hits & risers** are balanced as a whole track.
- **SFX** are on their own bus and duck half as far as the music, so a whoosh still reads under a sentence.
- **Ducking:** the bus is set 7 LU under the narration (Normal) and ducks 8 dB while the voice speaks, which puts it about 15 LU under during speech. While ducked it also opens a 4 dB pocket at 2 kHz.
- **Limiter:** true-peak aware (4x interpolation), with a -6 dBFS stem ceiling.
- **Final mix:** the video stage still applies `loudnorm=I=-14` to the full mix. No Python change was needed.

### SFX layer (`compose::place_sfx`)
- **Subtle:**
  - The manga's lettering mapped to sounds, including clang to sword clash and beep to glitch.
  - A whoosh on every change of bed, and a heartbeat in the held breath before a reveal.
  - A glitch on "system window / status / quest" pages.
  - A slash, punch or clash on fight pages at intensity 8+ (at most one per 14 s).
  - Track gain is -4 dB.
- **Punchy:** fight hits from intensity 6 (one per 4 s), whooshes on page turns in intense stretches (one per 8 s), full gain.
- **Spacing:** never two effects within 1.2 s; the more important one wins.

## Music step UI (music.rs)

- **Music style** is one picker: "Auto from the story" or one of the 7 presets.
  - Each tile has an icon, a hue and a one-line use.
  - Each has a play button that renders a 20 s demo on first use (cached in the temp folder per genre and sound set), then plays or stops it.
  - Why: one obvious choice, with previews instead of names you have to imagine. https://www.nngroup.com/articles/recognition-and-recall/
- **Energy:** a 5-step segmented control (Chill · Calm · As the story · Intense · Hyped).
  - mv-ui has no slider, and gpui-kit's `SliderState` needs a window subscription owned by `page.rs`, which this change does not touch. Five labelled steps are easier to hit than a continuous slider.
- **Sound effects:** Off · Subtle · Punchy.
- **Unchanged:** Level, Duck under the voice, AI spotting and the orchestra row.
- **Why it can't compose, in place:**
  - "Narrate the chapter first", with **Go to Audio**.
  - Without the Read step (Auto only), an info alert explains everything will be scored as quiet narration, with **Go to Read**.
  - Why: say what is wrong and how to fix it. https://www.nngroup.com/articles/error-message-guidelines/
- **Timeline:** cue chips and the cue panel use the new presets ("Genre" chips). The cue panel tags "Record stop in" as well as hit, drop and riser.

## How it was checked

`cargo run -p mv-score --release --example genre_demos -- <dir> 36 [GeneralUser-GS.sf2]` renders a
36 s demo per preset (two cues: a build, then an impact, record stop or swell) mastered to -16 LUFS
with a -1 dBFS ceiling.
- **What it prints:** BS.1770 loudness, sample peak, clipped samples, energy per band and per octave, and the tempo found in the onsets.
- **Cross-check:** loudness and true peak were confirmed with `ffmpeg -af ebur128=peak=true`.

| Preset (synth / orchestra) | LUFS | True peak (dBTP) | Clipped | Octave tilt (dB/oct) | BPM written / heard |
|---|---|---|---|---|---|
| Dark Tension | -16.0 / -16.0 | -2.0 / -2.1 | 0 | -2.5 / -2.4 | 92 / 92 |
| Epic Orchestral | -16.0 / -16.0 | -2.7 / -1.2 | 0 | -2.3 / -2.0 | 108 / 107 (83 with orchestra strings) |
| Phonk Fight | -16.0 / -16.0 | -0.6 / -0.6 | 0 | -2.2 / -2.3 | 140 / 70 (half-time, as intended) |
| Sad Piano | -16.0 / -16.1 | -1.0 / -1.0 | 0 | -0.4 / -1.1 | 68 / 68 |
| Lo-fi Light | -16.1 / -16.0 | -0.6 / -1.0 | 0 | -1.9 / -2.6 | 83 / 83 |
| Horror Drone | -16.0 / -16.0 | -1.1 / -1.0 | 0 | -2.4 / -2.6 | 65 / no pulse (as intended) |
| Mystery | -16.1 / -16.2 | -0.6 / -0.6 | 0 | +0.1 / -1.2 | 91 / 91 |

- **Spectral balance:**
  - The tilts sit in the -0.4 to -2.6 dB/octave range of produced music.
  - Phonk is sub- and low-heavy (63 Hz octave loudest, mids 12 dB down), as phonk is.
  - Sad Piano has almost no sub (piano and strings).
  - Mystery is the brightest (celesta and pizzicato).
- **Story arc with narration** (`score_demo`, 12 pages): Mystery, then a riser into a Phonk fight with an impact, then a drop and braam into Epic for the reveal, then a crossfade to Mystery. The music sits 14.6 LU under the narration overall, with a stem peak of -12.4 dBFS.
- **Tests (`cargo test -p mv-score`, 25):** every preset renders clean at the asked loudness; phonk claps on beat 3 only, with hat rolls, 808 slides and the cowbell; epic brings in the choir and a braam; lo-fi crackles and enters with a record stop; the music is silent before a reveal and loud after; Off < Subtle < Punchy effect counts; old `music.json` names parse.
