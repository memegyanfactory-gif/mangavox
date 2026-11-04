"""
Sound effects (python/sfx.py): the synthesised sounds (checked by numbers, no listening), the
user's override folder, placement rules, levels against the narration, and the sync map carrying
the story's cues.
"""
import math
import os
import shutil
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import sfx  # noqa: E402
import sync_map  # noqa: E402

SR = sfx.SR


def bands(x):
    p = np.abs(np.fft.rfft(x)) ** 2
    f = np.fft.rfftfreq(len(x), 1.0 / SR)
    return [float(p[(f >= a) & (f < b)].sum() / p.sum()) for a, b in ((0, 250), (250, 2000), (2000, 6000), (6000, SR / 2))]


def peaks(x, frame=0.01, floor_db=-12.0):
    """Onsets (s) of the bursts in x: frames rising above its loudest frame + floor_db."""
    env = sfx.frame_db(x, SR, frame)
    on = env > env.max() + floor_db
    return ([0.0] if on[0] else []) + [i * frame for i in range(1, len(on)) if on[i] and not on[i - 1]]


class SoundsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sounds = {n: sfx.synthesize(n) for n in sfx.SOUNDS}

    def test_every_sound_is_clean(self):
        lengths = {"impact": 0.8, "whoosh": 0.9, "riser": 2.2, "boom": 2.6, "sword_clang": 1.6, "magic": 1.8,
                   "heartbeat": 1.0, "glass": 1.5, "crowd_gasp": 1.4, "thunder": 3.6, "footsteps_run": 1.6, "door_slam": 1.2}
        self.assertEqual(set(lengths), set(sfx.SOUNDS))
        for name, x in self.sounds.items():
            with self.subTest(name):
                self.assertTrue(np.all(np.isfinite(x)))
                self.assertAlmostEqual(len(x) / SR, lengths[name], delta=0.01)
                peak = float(np.max(np.abs(x)))
                self.assertAlmostEqual(20 * math.log10(peak), -1.0, delta=0.1)  # no clipping, -1 dBFS
                self.assertLess(abs(float(np.mean(x))), 0.002)  # no DC
                rms = 20 * math.log10(float(np.sqrt(np.mean(x.astype(np.float64) ** 2))))
                self.assertTrue(-24.0 < rms < -8.0, f"rms {rms:.1f}")
                crest = -1.0 - rms
                self.assertTrue(7.0 < crest < 23.0, f"crest {crest:.1f}")
                # Ends faded, starts from (near) silence.
                self.assertLess(float(np.max(np.abs(x[-48:]))), 0.02)
                self.assertLess(abs(float(x[0])), 0.01)

    def test_spectra_fit_the_sound(self):
        b = {n: bands(x) for n, x in self.sounds.items()}
        self.assertGreater(b["boom"][0], 0.9, "a boom is sub")
        self.assertGreater(b["heartbeat"][0], 0.9, "a heartbeat is low")
        self.assertGreater(b["thunder"][0], 0.8, "thunder rumbles")
        self.assertGreater(b["thunder"][3], 0.02, "... with a crack")
        self.assertGreater(b["glass"][2] + b["glass"][3], 0.9, "glass is bright")
        self.assertGreater(b["sword_clang"][1] + b["sword_clang"][2], 0.9, "a clang rings in the mids")
        self.assertGreater(b["magic"][1] + b["magic"][2], 0.9)
        self.assertGreater(b["whoosh"][1] + b["whoosh"][2], 0.8, "a whoosh is air")
        self.assertGreater(b["crowd_gasp"][1], 0.5, "a gasp is breath in the voice range")
        for n in ("impact", "door_slam", "footsteps_run"):
            self.assertTrue(0.6 < b[n][0] < 0.97, f"{n}: low but with some mid/high crunch {b[n]}")
            self.assertGreater(b[n][1] + b[n][2], 0.02, n)

    def test_envelopes(self):
        env = lambda n: sfx.frame_db(self.sounds[n], SR, 0.01)
        w = env("whoosh")
        self.assertTrue(0.3 <= np.argmax(w) * 0.01 <= 0.45, "a whoosh peaks a third of the way in")
        r = env("riser")
        self.assertGreater(np.argmax(r) * 0.01, 2.0, "a riser is loudest at its very end")
        self.assertLess(r[:50].max(), r.max() - 20.0, "and starts from almost nothing")
        for n in ("impact", "boom", "door_slam", "sword_clang", "glass", "thunder"):
            self.assertLess(np.argmax(env(n)) * 0.01, 0.1, f"{n} hits at once")
        self.assertGreater(env("boom")[150], env("boom").max() - 30.0, "a boom has a long tail")
        self.assertGreater(env("thunder")[200], env("thunder").max() - 30.0, "thunder rumbles on")
        self.assertLess(env("impact")[50], env("impact").max() - 30.0, "an impact is short")
        hb = peaks(self.sounds["heartbeat"])
        self.assertEqual(len(hb), 2, hb)
        self.assertAlmostEqual(hb[1] - hb[0], 0.27, delta=0.03)
        steps = peaks(self.sounds["footsteps_run"], 0.005, -10.0)
        self.assertGreaterEqual(len(steps), 5, steps)
        self.assertTrue(0.3 <= np.argmax(env("crowd_gasp")) * 0.01 <= 0.6, "a gasp swells in")

    @unittest.skipUnless(shutil.which("ffmpeg"), "needs ffmpeg")
    def test_loudness_matches_a_meter(self):
        import re
        import subprocess
        tmp = tempfile.mkdtemp(prefix="mv-sfx-meter-")
        try:
            for name in ("boom", "glass", "whoosh"):
                path = os.path.join(tmp, name + ".wav")
                sfx.write_wav(path, np.concatenate([np.zeros(SR // 10), self.sounds[name], np.zeros(SR)]))
                r = subprocess.run(["ffmpeg", "-hide_banner", "-nostats", "-loglevel", "verbose", "-i", path,
                                    "-af", "ebur128=framelog=verbose", "-f", "null", "-"], capture_output=True, text=True)
                meter = max(float(v) for v in re.findall(r"M:\s*(-?[\d.]+)", r.stderr))
                self.assertAlmostEqual(sfx.body_lufs(self.sounds[name]), meter, delta=0.3, msg=name)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_deterministic(self):
        self.assertTrue(np.array_equal(sfx.synthesize("glass"), self.sounds["glass"]))


class LibraryTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="mv-sfx-")
        self.old = os.environ.get("MANGAVOX_FX_CACHE")
        os.environ["MANGAVOX_FX_CACHE"] = os.path.join(self.tmp, "fx")

    def tearDown(self):
        if self.old is None:
            os.environ.pop("MANGAVOX_FX_CACHE", None)
        else:
            os.environ["MANGAVOX_FX_CACHE"] = self.old
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_built_in_sounds_are_cached(self):
        x = sfx.load_sound("door_slam")
        cached = os.path.join(self.tmp, "fx", "sfx", f"door_slam_v{sfx.SYNTH_VERSION}.wav")
        self.assertTrue(os.path.exists(cached))
        y = sfx.load_sound("door_slam")
        self.assertLess(float(np.max(np.abs(x - y))), 1e-3)

    @unittest.skipUnless(shutil.which("ffmpeg"), "needs ffmpeg")
    def test_user_sounds_win(self):
        mine = os.path.join(self.tmp, "mine")
        os.makedirs(mine)
        tone = np.concatenate([np.zeros(SR // 10), 0.5 * np.sin(2 * np.pi * 440 * np.arange(SR // 2) / SR)])
        sfx.write_wav(os.path.join(mine, "Impact_heavy.wav"), tone)
        sfx.write_wav(os.path.join(mine, "impactful.wav"), tone * 0.1)
        self.assertEqual([os.path.basename(p) for p in sfx.user_sounds("impact", mine)], ["Impact_heavy.wav"])
        x = sfx.load_sound("impact", mine)
        self.assertAlmostEqual(len(x) / SR, 0.5, delta=0.01, msg="leading silence trimmed")
        self.assertAlmostEqual(20 * math.log10(float(np.max(np.abs(x)))), -1.0, delta=0.1)
        # Other sounds stay built in.
        self.assertEqual(len(sfx.load_sound("whoosh", mine)), len(sfx.synthesize("whoosh")))

    @unittest.skipUnless(shutil.which("ffmpeg"), "needs ffmpeg")
    def test_library_only_cues_need_a_file_or_a_fallback(self):
        lib = os.path.join(self.tmp, "lib")
        os.makedirs(lib)
        # No file: a punch falls back to the built-in impact, a gunshot has no sound at all.
        self.assertTrue(sfx.playable("punch", lib))
        self.assertEqual(len(sfx.load_sound("punch", lib)), len(sfx.synthesize("impact")))
        self.assertFalse(sfx.playable("gunshot", lib))
        self.assertIsNone(sfx.load_sound("gunshot", lib))
        # A CC0 file in the library folder: played as it is.
        shot = np.concatenate([np.zeros(SR // 20), np.exp(-np.arange(SR // 4) / 2000.0) * np.sin(np.arange(SR // 4))])
        sfx.write_wav(os.path.join(lib, "gunshot_01.wav"), 0.8 * shot)
        self.assertTrue(sfx.playable("gunshot", lib))
        self.assertAlmostEqual(len(sfx.load_sound("gunshot", lib)) / SR, 0.25, delta=0.01)

    def test_long_recordings_are_cut_and_faded(self):
        rain = np.ones(int(30 * SR), dtype=np.float32) * 0.5
        x = sfx.fit_length(rain, "rain")
        self.assertEqual(len(x), int(sfx.AMBIENCE_SEC * SR))
        self.assertLess(abs(float(x[0])), 1e-6, "ambience fades in")
        self.assertLess(abs(float(x[-1])), 1e-3)
        self.assertAlmostEqual(float(x[len(x) // 2]), 0.5, places=5)
        thunder = sfx.fit_length(rain, "thunder")
        self.assertEqual(len(thunder), int(sfx.MAX_SEC["hit"] * SR))
        self.assertAlmostEqual(float(thunder[0]), 0.5, places=5, msg="a hit starts on its moment")
        self.assertLess(abs(float(thunder[-1])), 1e-3)
        short = np.ones(SR, dtype=np.float32)
        self.assertIs(sfx.fit_length(short, "rain"), short)

    def test_user_dir_lookup(self):
        out = os.path.join(self.tmp, "data", "studio", "projects", "m", "c")
        self.assertEqual(sfx.user_dir_for({}, out), os.path.join(self.tmp, "data", "studio", "sfx"))
        self.assertEqual(sfx.user_dir_for({"sfx_dir": "X"}, out), "X")


class NamesTest(unittest.TestCase):
    def test_story_vocabulary_matches(self):
        # The story writer may cue exactly the sounds this module knows (mv-recap types.rs SFX_NAMES).
        import re
        path = os.path.join(os.path.dirname(__file__), "..", "native", "crates", "mv-recap", "src", "types.rs")
        with open(path, encoding="utf-8") as f:
            body = re.search(r"pub const SFX_NAMES: &\[&str\] = &\[(.*?)\];", f.read(), re.S).group(1)
        self.assertEqual(set(re.findall(r'"([a-z_]+)"', body)), set(sfx.NAMES))

    def test_names_and_levels(self):
        self.assertEqual(sfx.normalize_name(" Sword clang"), "sword_clang")
        self.assertEqual(sfx.normalize_name("door-slam"), "door_slam")
        self.assertIsNone(sfx.normalize_name("laser"))
        self.assertIsNone(sfx.normalize_name(None))
        self.assertEqual(sfx.normalize_name("Bullet"), "gunshot")
        self.assertEqual(sfx.normalize_name("rain"), "rain")
        self.assertEqual(sfx.normalize_name("glass break"), "glass")
        self.assertEqual(set(sfx.CATEGORY), set(sfx.NAMES), "every cue has a level")
        self.assertTrue(set(sfx.FALLBACK.values()) <= set(sfx.SOUNDS))
        self.assertEqual(sfx.level_db("rain", 1), -28.0)
        self.assertEqual(sfx.level_db("gunshot", 2), -12.0)
        self.assertEqual([sfx.normalize_strength(v) for v in (0, "3", 9, None, "x", 2.4)], [1, 3, 3, 2, 2, 2])
        self.assertEqual(sfx.level_db("impact", 1), -15.0)
        self.assertEqual(sfx.level_db("impact", 3), -9.0)
        self.assertEqual(sfx.level_db("whoosh", 1), -23.0)
        self.assertEqual(sfx.level_db("whoosh", 3), -16.0)
        self.assertEqual(sfx.level_db("crowd_gasp", 1), -28.0)
        self.assertEqual(sfx.level_db("heartbeat", 3), -22.0)
        self.assertEqual(sfx.level_db("boom", 2, punchy=True), -12.0 + sfx.PUNCHY_DB)
        self.assertEqual([sfx.mode_of(v) for v in (True, None, False, "off", "Punchy", "subtle", 0)],
                         ["subtle", "subtle", "off", "off", "punchy", "subtle", "off"])


def clip(k, start, page=0, impact=False):
    return {"idx": k, "page": page, "impact": impact, "start": start}


class PlacementTest(unittest.TestCase):
    def test_writer_cues_are_thinned_out(self):
        # A cue on every 3 s beat for two minutes: far too many.
        n, beat = 40, 3.0
        items = [{"sfx": "impact", "sfx_strength": 2} for _ in range(n)]
        clips = [clip(k, k * beat) for k in range(n)]
        starts = [c["start"] for c in clips]
        cands = sfx.candidates(items, clips, starts, [None] * n)
        total = n * beat
        kept = sfx.select(cands, total)
        self.assertLessEqual(len(kept), math.ceil(total / sfx.SECONDS_PER_CUE) + 1)
        times = [c["at"] for c in kept]
        for a, b in zip(times, times[1:]):
            self.assertGreaterEqual(b - a, sfx.SPACING_EARLY if a < sfx.EARLY_SEC else sfx.SPACING)
        early = [t for t in times if t < sfx.EARLY_SEC]
        self.assertGreater(len(early), 30 / sfx.SPACING, "the opening may be a little denser")

    def test_writer_first_then_picture_moments(self):
        items = [{}, {"sfx": "boom", "sfx_strength": 3}, {}, {}]
        clips = [clip(0, 0.0), clip(1, 5.0, impact=True), clip(2, 6.0, impact=True), clip(3, 40.0, impact=True)]
        starts = [c["start"] for c in clips]
        trans = [None, ("fade", 0.5), ("whipleft", 0.22), ("fade", 0.5)]
        cands = sfx.candidates(items, clips, starts, trans)
        self.assertEqual(sorted((c["name"], c["ai"]) for c in cands),
                         [("boom", True), ("impact", False), ("impact", False), ("whoosh", False)])
        kept = sfx.select(cands, 60.0)
        self.assertEqual([(c["name"], c["at"]) for c in kept], [("boom", 5.0), ("impact", 40.0)])

    def test_intensity_nudges_and_gates(self):
        items = [{"sfx": "impact", "sfx_strength": 2}, {"sfx": "impact", "sfx_strength": 2}, {}, {}]
        clips = [clip(0, 0.0, page=0), clip(1, 10.0, page=1), clip(2, 20.0, page=2, impact=True), clip(3, 30.0, page=3, impact=True)]
        scenes = {0: {"intensity": 9}, 1: {"intensity": 2}, 2: {"intensity": 3}, 3: {"intensity": 7}}
        cands = sfx.candidates(items, clips, [c["start"] for c in clips], [None] * 4, scenes)
        self.assertEqual([(c["clip"], c["strength"]) for c in cands], [(0, 3), (1, 1), (3, 1)])

    def test_riser_pairs_with_its_hit(self):
        items = [{"sfx": "riser", "sfx_strength": 2}, {"sfx": "riser", "sfx_strength": 2}]
        # Two clips' cues on the same moment: a riser and (from the look) the punch it ends on.
        clips = [clip(0, 0.0), clip(1, 12.0, impact=True)]
        cands = sfx.candidates(items, clips, [0.0, 12.0], [None, None])
        cands.append({"name": "impact", "strength": 2, "at": 12.0, "clip": 1, "ai": True, "intensity": 0})
        kept = sfx.select(cands, 20.0)
        self.assertEqual(sorted(c["name"] for c in kept if c["clip"] == 1), ["impact", "riser"])

    def test_alignment(self):
        sounds = {n: sfx.synthesize(n) for n in ("riser", "whoosh", "impact")}
        sel = [
            {"name": "riser", "strength": 2, "at": 10.0, "clip": 1, "ai": True, "intensity": 0},
            {"name": "whoosh", "strength": 1, "at": 20.0, "clip": 2, "ai": False, "intensity": 0},
            {"name": "impact", "strength": 2, "at": 30.0, "clip": 3, "ai": True, "intensity": 0},
        ]
        ev = {e["name"]: e for e in sfx.place(sel, sounds)}
        self.assertAlmostEqual(ev["riser"]["start"] + len(sounds["riser"]) / SR, 10.0, delta=0.002, msg="a riser ends on its beat")
        peak = float(np.argmax(sfx.frame_db(sounds["whoosh"], SR, 0.01))) * 0.01
        self.assertAlmostEqual(ev["whoosh"]["start"] + peak, 20.0, delta=0.002, msg="a whoosh peaks on the cut")
        self.assertEqual(ev["impact"]["start"], 30.0)

    def test_hits_find_the_pause(self):
        # Speech everywhere except a pause from 9.8 to 10.05 s.
        t = np.arange(int(14 * SR)) / SR
        voice = 0.1 * np.sin(2 * np.pi * 200 * t) * ((t < 9.8) | (t >= 10.05))
        level = sfx.speech_db(voice)
        sounds = {"impact": sfx.synthesize("impact")}
        hit = lambda at, ai: {"name": "impact", "strength": 2, "at": at, "clip": 1, "ai": ai, "intensity": 0}
        ev = sfx.place([hit(10.0, True)], sounds, voice, level)
        self.assertAlmostEqual(ev[0]["start"], 9.82, delta=0.011, msg="moved into the pause")
        self.assertEqual(ev[0]["rel_db"], -12.0)
        # No pause near 5 s: the writer's cue plays quieter, the renderer's own is dropped.
        ev = sfx.place([hit(5.0, True)], sounds, voice, level)
        self.assertEqual((ev[0]["start"], ev[0]["rel_db"]), (5.0, -12.0 + sfx.MASKED_DB))
        self.assertEqual(sfx.place([hit(5.0, False)], sounds, voice, level), [])


class TrackTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="mv-sfx-track-")
        self.old = os.environ.get("MANGAVOX_FX_CACHE")
        os.environ["MANGAVOX_FX_CACHE"] = os.path.join(self.tmp, "fx")

    def tearDown(self):
        if self.old is None:
            os.environ.pop("MANGAVOX_FX_CACHE", None)
        else:
            os.environ["MANGAVOX_FX_CACHE"] = self.old
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_stem_levels_sit_under_the_narration(self):
        total = 40.0
        t = np.arange(int(total * SR)) / SR
        # Sentences of 3.6 s with 0.4 s pauses; beats start after each pause.
        speaking = (t % 4.0) < 3.6
        voice = (0.12 * np.sin(2 * np.pi * 180 * t) * (0.6 + 0.4 * np.abs(np.sin(2 * np.pi * 3 * t))) * speaking).astype(np.float32)
        nar = os.path.join(self.tmp, "narration.wav")
        sfx.write_wav(nar, voice)
        starts = [4.0 * k for k in range(10)]
        items = [{} for _ in starts]
        items[1] = {"sfx": "impact", "sfx_strength": 3}
        items[4] = {"sfx": "heartbeat", "sfx_strength": 1}
        items[7] = {"sfx": "whoosh", "sfx_strength": 2}
        items[8] = {"sfx": "laser"}
        clips = [clip(k, s) for k, s in enumerate(starts)]
        out = os.path.join(self.tmp, "sfx.wav")
        events = sfx.build_track(items, clips, starts, [None] * len(starts), total, nar, out)
        self.assertEqual([e["name"] for e in events], ["impact", "heartbeat", "whoosh"])
        stem = sfx.read_wav(out)[0]
        self.assertAlmostEqual(len(stem) / SR, total, delta=0.01)
        # Loudness against loudness: each sound's loudest momentary LUFS against the narration's
        # integrated LUFS (ffmpeg's meter).
        level = sfx.narration_lufs(nar) if shutil.which("ffmpeg") else sfx.speech_db(voice)
        for e, want in zip(events, (-9.0, -28.0, -19.5)):
            seg = stem[max(0, int(e["start"] * SR)):int((e["start"] + 3.0) * SR)]
            self.assertAlmostEqual(sfx.body_lufs(seg) - level, want, delta=0.6, msg=e["name"])
        self.assertLess(float(np.max(np.abs(stem))), 0.5, "well under full scale")
        # Off: nothing.
        self.assertEqual(sfx.build_track(items, clips, starts, [None] * 10, total, nar, out + "2", mode="off"), [])
        self.assertFalse(os.path.exists(out + "2"))


class SyncMapTest(unittest.TestCase):
    def test_cues_ride_on_the_beats(self):
        segments = [{"page": 0, "start_ms": 0, "end_ms": 6000, "audio_file": "page_0000.wav", "text": "x"}]
        beats = [
            {"page": 0, "panel": "a", "text": "He swings.", "sfx": "Sword Clang", "sfx_strength": 5},
            {"page": 0, "panel": "b", "text": "Laser.", "sfx": "laser", "sfx_strength": 2},
            {"page": 0, "panel": "c", "text": "Quiet."},
        ]
        out = sync_map._build_beat_segments(segments, beats)
        self.assertEqual([(b.get("sfx"), b.get("sfx_strength")) for b in out], [("sword_clang", 3), (None, None), (None, None)])


if __name__ == "__main__":
    unittest.main()
