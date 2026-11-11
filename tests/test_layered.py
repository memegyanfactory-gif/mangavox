"""Layered motion (the optional 2.5D pack): prompts from read.json, the mask gate, the
lettering detector, motion chosen from the scene, the layer cache, key-moment selection, and
the renderer carrying on as usual without the pack. No network, no models."""
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import wave

import numpy as np

from python import layered_plan as lp
from python import video_generator as vg


def panel(bbox=(0.0, 0.0, 1.0, 1.0), faces=()):
    return {"bbox": list(bbox), "characters": [{"name": f"c{i}", "face": list(f)} for i, f in enumerate(faces)]}


class PromptTests(unittest.TestCase):
    def test_prompts_come_from_read_json_faces_in_crop_pixels(self):
        # A panel in the right half of the page; one face in its upper middle.
        p = panel((0.5, 0.0, 1.0, 0.5), [(0.7, 0.1, 0.8, 0.2)])
        prompts = lp.char_prompts(p, 1000, 1000)
        self.assertEqual(len(prompts), 1)
        q = prompts[0]
        x0, y0, x1, y1 = q["face"]
        # The crop is the bbox plus 1 %: the face lands about 40-60 % across, 20-40 % down.
        self.assertAlmostEqual(x0, 1000 * (0.7 - 0.495) / 0.505, delta=1)
        self.assertTrue(x0 < q["point"][0] < x1 and y0 < q["point"][1] < y1)
        bx0, by0, bx1, by1 = q["box"]
        self.assertTrue(bx0 < x0 and bx1 > x1 and by0 < y0 and by1 > y1, "the body box holds the face")
        self.assertGreater(by1 - y1, 2 * (y1 - y0), "and runs down over the body")
        self.assertTrue(0 <= bx0 and bx1 <= 1000 and 0 <= by0 and by1 <= 1000, "inside the panel")

    def test_no_face_no_layer_and_duplicates_merge(self):
        self.assertEqual(lp.char_prompts(panel(faces=()), 500, 500), [])
        self.assertEqual(lp.char_prompts({"bbox": [0, 0, 1, 1], "characters": [{"name": "x", "face": None}]}, 500, 500), [])
        twice = panel(faces=[(0.2, 0.2, 0.4, 0.4), (0.21, 0.2, 0.41, 0.4)])
        self.assertEqual(len(lp.char_prompts(twice, 500, 500)), 1)
        apart = panel(faces=[(0.1, 0.1, 0.2, 0.2), (0.7, 0.1, 0.8, 0.2)])
        self.assertEqual(len(lp.char_prompts(apart, 500, 500)), 2)


class GateTests(unittest.TestCase):
    def blob(self, h=200, w=200, box=(60, 40, 140, 190)):
        m = np.zeros((h, w), bool)
        m[box[1]:box[3], box[0]:box[2]] = True
        return m

    def test_a_clean_character_matte_passes(self):
        m = self.blob()
        ok, why = lp.mask_gate(lp.mask_stats(m, face=(80, 45, 120, 85), score=0.9))
        self.assertTrue(ok, why)

    def test_bad_mattes_are_dropped(self):
        tiny = self.blob(box=(90, 90, 95, 95))
        self.assertEqual(lp.mask_gate(lp.mask_stats(tiny))[1], "tiny")
        bg = np.ones((200, 200), bool)
        bg[50:150, 50:150] = False
        self.assertFalse(lp.mask_gate(lp.mask_stats(bg))[0], "background hugging the border")
        frag = np.zeros((200, 200), bool)
        for i in range(10):
            frag[10 + 18 * i:22 + 18 * i, 10 + 18 * i:30 + 18 * i] = True
        self.assertEqual(lp.mask_gate(lp.mask_stats(frag))[1], "fragmented")
        m = self.blob()
        self.assertEqual(lp.mask_gate(lp.mask_stats(m, face=(150, 0, 200, 30)))[1], "misses the face")
        self.assertEqual(lp.mask_gate(lp.mask_stats(m, score=0.2))[1], "low score")

    def test_labels_holes_and_thin_objects(self):
        m = np.zeros((50, 50), bool)
        m[5:15, 5:15] = True
        m[30:40, 30:45] = True
        lab, n = lp.label(m)
        self.assertEqual(n, 2)
        ring = np.zeros((30, 30), bool)
        ring[5:25, 5:25] = True
        ring[10:20, 10:20] = False
        self.assertTrue(lp.fill_holes(ring)[15, 15])
        whip = np.zeros((300, 300), bool)
        for x in range(20, 280):
            y = int(150 + 20 * math.sin(x / 40))
            whip[y - 3:y + 3, x] = True
        st = lp.thin_stats(whip)
        self.assertTrue(lp.thin_object_ok(st), st)
        self.assertFalse(lp.thin_object_ok(lp.thin_stats(self.blob(300, 300, (100, 100, 200, 200)))))


class LetteringTests(unittest.TestCase):
    def test_a_speech_bubble_is_found_and_hatching_is_not(self):
        g = np.full((300, 400), 0.55, np.float32)
        # Hatching (thin parallel lines) on the left: not text.
        for x in range(10, 150, 6):
            g[40:260, x:x + 1] = 0.05
        # A white bubble with a dark outline and a line of letters inside.
        yy, xx = np.mgrid[0:300, 0:400]
        inside = ((xx - 280) / 90.0) ** 2 + ((yy - 150) / 60.0) ** 2 <= 1
        edge = (((xx - 280) / 93.0) ** 2 + ((yy - 150) / 63.0) ** 2 <= 1) & ~inside
        g[inside] = 0.97
        g[edge] = 0.05
        for i in range(6):
            x0 = 215 + i * 22
            g[140:160, x0:x0 + 3] = 0.05
            g[140:160, x0 + 10:x0 + 13] = 0.05
            g[149:152, x0:x0 + 13] = 0.05
        m = lp.detect_lettering(g)
        self.assertTrue(m[150, 280], "the bubble is lettering")
        self.assertFalse(m[150, 60], "hatching is not")


class MotionTests(unittest.TestCase):
    layers = [{"name": "hero", "depth": 0.9, "area": 0.3, "face": [1, 1, 5, 5]}, {"name": "foe", "depth": 0.6, "area": 0.1, "face": [1, 1, 3, 3]}]

    def test_a_ground_smash_gets_the_big_hit(self):
        scene = {"kind": "fight", "intensity": 9, "tags": ["ground smash", "rubble", "crushing blow"]}
        plan = lp.motion_plan(scene, {"layers": self.layers, "impact": (0.4, 0.7)})
        self.assertEqual(plan["shock"], {"centre": (0.4, 0.7)})
        self.assertTrue(plan["drift"])
        self.assertEqual(plan["breathe"]["layer"], "hero", "breathing on the main character")
        self.assertIsNone(plan["sway"])
        self.assertIsNone(plan["streaks"])

    def test_quiet_scenes_and_mood_words_do_not_shake(self):
        calm = lp.motion_plan({"kind": "calm", "intensity": 3, "tags": ["tea", "garden"]}, {"layers": self.layers})
        self.assertEqual(lp.plan_effects(calm), ["parallax", "breathe"])
        self.assertLess(calm["breathe"]["amp"], 0.006)
        # An "explosive" mood with no hit in its tags is not an explosion.
        moody = lp.motion_plan({"kind": "fight", "intensity": 9, "mood": "explosive", "tags": ["arena", "crowd"]}, {"layers": self.layers})
        self.assertIsNone(moody["shock"])
        self.assertIsNone(moody["drift"])

    def test_sway_and_streaks_need_what_the_prep_found(self):
        a = {"layers": self.layers, "thin": {"root": [0.3, 0.9], "tip": [1.0, 0.3]}, "speed": (1.0, 0.0)}
        plan = lp.motion_plan({"kind": "fight", "intensity": 8, "tags": ["whip"]}, a)
        self.assertEqual(plan["sway"]["root"], [0.3, 0.9])
        self.assertEqual(plan["streaks"]["vec"], (1.0, 0.0))
        self.assertLessEqual(plan["parallax"], 0.011, "a few percent of the panel at most")
        none = lp.motion_plan({"kind": "fight", "intensity": 8}, {"layers": []})
        self.assertIsNone(none["breathe"])
        self.assertEqual(none["zoom"], 0.0)


class CacheTests(unittest.TestCase):
    def test_manifest_round_trip_and_invalidation(self):
        d = tempfile.mkdtemp()
        try:
            for f in ("plate.png", "text.png", "clean.png", "depth.png", "char_0.png"):
                open(os.path.join(d, f), "wb").close()
            data = {"plate": "plate.png", "text": "text.png", "clean": "clean.png", "depth": "depth.png", "layers": [{"name": "a", "file": "char_0.png", "depth": 0.8}], "analysis": {"speed": None}}
            lp.write_manifest(d, data)
            back = lp.read_manifest(d)
            self.assertEqual(back["layers"], data["layers"])
            self.assertEqual(back["version"], lp.PREP_VERSION)
            os.remove(os.path.join(d, "char_0.png"))
            self.assertIsNone(lp.read_manifest(d), "a missing layer file means prep again")
            open(os.path.join(d, "char_0.png"), "wb").close()
            os.remove(os.path.join(d, "depth.png"))
            self.assertIsNone(lp.read_manifest(d), "the animator needs the depth map (and the clean plate)")
            open(os.path.join(d, "depth.png"), "wb").close()
            lp.write_manifest(d, data)
            raw = json.load(open(os.path.join(d, "manifest.json"), encoding="utf-8"))
            raw["version"] = lp.PREP_VERSION - 1
            json.dump(raw, open(os.path.join(d, "manifest.json"), "w", encoding="utf-8"))
            self.assertIsNone(lp.read_manifest(d), "an older prep is redone")
        finally:
            shutil.rmtree(d, ignore_errors=True)

    def test_panel_key_follows_pixels_and_faces(self):
        d = tempfile.mkdtemp()
        try:
            src = os.path.join(d, "p0000_00.jpg")
            open(src, "wb").write(b"abc")
            a = lp.panel_key(src, panel(faces=[(0.1, 0.1, 0.2, 0.2)]))
            self.assertEqual(a, lp.panel_key(src, panel(faces=[(0.1, 0.1, 0.2, 0.2)])))
            self.assertNotEqual(a, lp.panel_key(src, panel(faces=[(0.3, 0.1, 0.4, 0.2)])))
            open(src, "wb").write(b"abd")
            self.assertNotEqual(a, lp.panel_key(src, panel(faces=[(0.1, 0.1, 0.2, 0.2)])))
            self.assertNotEqual(lp.anim_key(a, [1]), lp.anim_key(a, [2]))
        finally:
            shutil.rmtree(d, ignore_errors=True)


class KeyMomentTests(unittest.TestCase):
    def test_key_mode_picks_the_best_moments_sparsely(self):
        moods = [{"intensity": i, "kind": "fight" if i >= 7 else "dialogue", "tags": []} for i in (3, 8, 9, 4, 7.5, 9.5, 2, 3, 8, 3, 3, 3)]
        scores = [lp.key_candidate(m) for m in moods]
        self.assertIsNone(scores[0])
        picked = lp.select_layered("key", [True] * 12, scores)
        self.assertLessEqual(len(picked), 12 // lp.KEY_SHARE)
        self.assertIn(5, picked, "the most intense beat")
        self.assertTrue(all(b - a >= lp.KEY_GAP for a, b in zip(picked, picked[1:])))
        self.assertEqual(lp.select_layered("key", [True, False, True], [9, 9, None]), [0])
        self.assertEqual(lp.select_layered("key", [True] * 3, [None] * 3), [])
        self.assertEqual(lp.select_layered("all", [True, False, True], [None] * 3), [0, 2])
        self.assertEqual(lp.select_layered("off", [True] * 3, [9] * 3), [])
        self.assertIsNotNone(lp.key_candidate({"intensity": 5, "kind": "reveal", "tags": []}), "a reveal")
        self.assertIsNotNone(lp.key_candidate({"intensity": 6, "kind": "dialogue", "tags": ["surprise attack"]}), "a hit")


class PackTests(unittest.TestCase):
    def test_pack_is_found_only_when_installed(self):
        root = tempfile.mkdtemp()
        old = os.environ.pop("MANGAVOX_VFX_PYTHON", None)
        try:
            self.assertIsNone(lp.pack_python({}, root=root)[0])
            py = lp.venv_python(lp.pack_dir(root))
            os.makedirs(os.path.dirname(py))
            open(py, "wb").close()
            self.assertIsNone(lp.pack_python({}, root=root)[0], "no install marker yet")
            json.dump({"ok": True}, open(os.path.join(lp.pack_dir(root), "mangavox-vfx.json"), "w"))
            self.assertEqual(lp.pack_python({}, root=root)[0], py)
            self.assertIsNone(lp.pack_python({"vfx_python": os.path.join(root, "nope.exe")}, root=root)[0])
        finally:
            if old is not None:
                os.environ["MANGAVOX_VFX_PYTHON"] = old
            shutil.rmtree(root, ignore_errors=True)

    def test_look_carries_the_mode(self):
        self.assertEqual(vg.resolve_look({"preset": "dynamic"})["layered"], "off")
        self.assertEqual(vg.resolve_look({"preset": "dynamic", "layered": "key"})["layered"], "key")
        self.assertEqual(vg.resolve_look({"preset": "clean", "layered": "bogus"})["layered"], "off")

    def test_selection_is_empty_without_the_pack(self):
        clips = [{"idx": 0, "images": {"fg_size": (100, 100)}, "duo": None, "source": "x/p0000_00.jpg", "mood": {"intensity": 9, "kind": "fight", "tags": []}}]
        look = {"layered": "all"}
        self.assertEqual(vg.select_layered_clips(look, {"vfx_python": os.path.join(tempfile.gettempdir(), "no-such-python.exe")}, clips, [{}], {}), set())
        self.assertEqual(vg.select_layered_clips({"layered": "off"}, {}, clips, [{}], {}), set())


def _tone(path, seconds):
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(24000)
        n = int(24000 * seconds)
        w.writeframes(b"".join(int(3000 * math.sin(i / 20)).to_bytes(2, "little", signed=True) for i in range(n)))


@unittest.skipUnless(shutil.which("ffmpeg"), "needs ffmpeg")
class RenderWithoutPackTests(unittest.TestCase):
    def test_a_broken_pack_renders_as_usual(self):
        """layered=all with a "pack" that cannot run (this Python, no torch): the video is made."""
        from PIL import Image, ImageDraw
        # The renderer finds its sibling modules (captions, caption_engine) on the path, as in the app.
        here = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "python")
        if here not in sys.path:
            sys.path.insert(0, here)
        d = tempfile.mkdtemp()
        try:
            panels_dir = os.path.join(d, "studio", "panels")
            os.makedirs(panels_dir)
            os.makedirs(os.path.join(d, "audio"))
            img = Image.new("RGB", (300, 400), "white")
            ImageDraw.Draw(img).ellipse((100, 60, 200, 160), fill="black")
            src = os.path.join(panels_dir, "p0000_00.jpg")
            img.save(src)
            _tone(os.path.join(d, "audio", "page_0000.wav"), 1.6)
            read = {"pages": [{"index": 0, "summary": "a hit", "scene": {"kind": "fight", "mood": "brutal", "intensity": 9, "tags": ["impact"]},
                               "panels": [{"bbox": [0, 0, 1, 1], "characters": [{"name": "a", "face": [0.33, 0.15, 0.67, 0.4]}]}]}]}
            json.dump(read, open(os.path.join(d, "studio", "read.json"), "w"))
            sync = {"beat_segments": [{"page": 0, "panel": "p0000_00.jpg", "panel_image": src, "text": "It lands!", "audio_file": "page_0000.wav",
                                       "audio_offset_ms": 0, "duration_ms": 1500, "start_ms": 0, "end_ms": 1500}]}
            config = {"resolution": "720p", "look": {"preset": "clean", "layered": "all"}, "vfx_python": sys.executable,
                      "subtitles": {"enabled": False}, "sfx": "off"}
            out = vg.generate_video([src], sync, d, config)
            self.assertTrue(os.path.isfile(out) and os.path.getsize(out) > 1000)
            r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", out], capture_output=True, text=True)
            self.assertAlmostEqual(float(r.stdout.strip()), 1.5, delta=0.25)
        finally:
            shutil.rmtree(d, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()


class WholeFigureTests(unittest.TestCase):
    """The second SAM2 pass: a matte grown to the limbs its face-sized box cut off."""

    def test_a_grown_matte_is_taken_only_when_it_is_the_same_character(self):
        first = np.zeros((100, 100), bool)
        first[20:60, 30:60] = True
        hand = first.copy()
        hand[40:50, 10:30] = True                       # the reaching arm, outside the first box
        self.assertTrue(lp.grown_ok(first, hand))
        chair = first.copy()
        chair[10:90, 5:95] = True                       # the figure plus the chair and the floor
        self.assertFalse(lp.grown_ok(first, chair), "far more than the character")
        other = np.zeros((100, 100), bool)
        other[20:60, 55:95] = True                      # mostly somebody else
        self.assertFalse(lp.grown_ok(first, other), "loses the first matte")
        self.assertFalse(lp.grown_ok(np.zeros((10, 10), bool), hand[:10, :10]), "nothing to grow from")

    def test_stray_dots_are_dropped_and_real_pieces_kept(self):
        m = np.zeros((100, 100), bool)
        m[10:60, 10:60] = True                          # the figure
        m[70:80, 70:80] = True                          # a hand behind an armrest: 4 % of it
        m[90, 90] = m[5, 80] = True                     # dots
        out = lp.drop_specks(m)
        self.assertTrue(out[20, 20] and out[75, 75])
        self.assertFalse(out[90, 90] or out[5, 80])
        self.assertFalse(lp.drop_specks(np.zeros((4, 4), bool)).any())

    def test_layers_of_an_older_pack_version_are_removed(self):
        with tempfile.TemporaryDirectory() as d:
            root = lp.layers_root(d)
            new, old, half = (os.path.join(root, n) for n in ("new", "old", "half"))
            for f in (new, old, half):
                os.makedirs(f)
            lp.write_manifest(new, {"layers": []})
            with open(os.path.join(old, "manifest.json"), "w", encoding="utf-8") as f:
                json.dump({"version": lp.PREP_VERSION - 1, "layers": []}, f)
            self.assertEqual(lp.prune_stale(d), 1)
            self.assertTrue(os.path.isdir(new) and os.path.isdir(half), "the current and the half-made stay")
            self.assertFalse(os.path.exists(old))
            self.assertEqual(lp.prune_stale(os.path.join(d, "nowhere")), 0)
