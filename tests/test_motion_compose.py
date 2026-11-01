"""
Motion compose (python/motion_compose.py): easing and keyframes, the light spec check, the
transitions, and a real render of a tiny spec (image, text, shape and particle layers, a fade, a
sound effect) into a temp folder. No network, no TTS; FFmpeg's CPU encoder.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

os.environ["MANGAVOX_VIDEO_GPU"] = "0"  # libx264: the same output on every machine
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

from PIL import Image  # noqa: E402

import motion_compose as mc  # noqa: E402

REAL_EMIT = mc.emit

HAS_FFMPEG = shutil.which("ffmpeg") is not None


def probe_duration(path):
    if not shutil.which("ffprobe"):
        return None
    out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", path], capture_output=True, text=True).stdout
    return float(json.loads(out)["format"]["duration"])


def has_audio(path):
    if not shutil.which("ffprobe"):
        return None
    out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a", "-show_entries", "stream=codec_type", "-of", "json", path], capture_output=True, text=True).stdout
    return bool(json.loads(out).get("streams"))


class EasingAndKeyframes(unittest.TestCase):
    def test_eases_start_at_0_and_end_at_1(self):
        for name in ("linear", "in", "out", "in_out", "back", "bounce", "elastic", "ease_in_out", "back_out"):
            self.assertAlmostEqual(mc.ease(name, 0), 0, places=6, msg=name)
            self.assertAlmostEqual(mc.ease(name, 1), 1, places=6, msg=name)
        self.assertGreater(max(mc.ease("back", p / 20) for p in range(21)), 1.0, "back overshoots")
        self.assertEqual(mc.ease("step", 0.99), 0.0)

    def test_keyframes_interpolate_and_hold(self):
        layer = {"scale": 1.0, "keyframes": [{"t": 1, "scale": 2.0, "ease": "linear"}, {"t": 3, "scale": 4.0, "ease": "linear"}]}
        self.assertEqual(mc.prop_at(layer, "scale", 0), 2.0, "held before the first")
        self.assertAlmostEqual(mc.prop_at(layer, "scale", 2), 3.0)
        self.assertEqual(mc.prop_at(layer, "scale", 9), 4.0, "held after the last")
        self.assertEqual(mc.prop_at(layer, "opacity", 2), 1.0, "a property without keyframes keeps its value")


class Checking(unittest.TestCase):
    def test_good_and_bad_specs(self):
        self.assertEqual(mc.check({"scenes": [{"duration": 2, "layers": [{"type": "text", "text": "Hi"}]}]}), [])
        self.assertTrue(mc.check({"scenes": []})[0].startswith("spec: needs"))
        errs = mc.check({"scenes": [{"duration": "long", "transition": {"type": "dissolve"},
                                     "layers": [{"type": "img"}, {"type": "image", "src": "C:/nope/missing.png"}, {"type": "text", "text": " "}]}]})
        joined = "\n".join(errs)
        for want in ("scenes[0].duration", "transition.type", "layers[0].type", "layers[1].src: file not found", "layers[2]: a text layer"):
            self.assertIn(want, joined)

    def test_scene_starts_overlap_by_their_transition(self):
        s = mc.normalise({"scenes": [{"duration": 2}, {"duration": 3, "transition": {"type": "fade", "duration": 0.5}}, {"duration": 1, "transition": "cut"}]})
        self.assertEqual([x["start"] for x in s["scenes"]], [0.0, 1.5, 4.5])
        self.assertAlmostEqual(s["duration"], 5.5)
        self.assertIsNone(s["scenes"][2]["transition"])

    def test_transitions_blend(self):
        a, b = Image.new("RGB", (40, 20), (0, 0, 0)), Image.new("RGB", (40, 20), (255, 255, 255))
        for kind in mc.TRANSITIONS:
            for p in (0.0, 0.5, 1.0):
                out = mc.transition(a, b, kind, p)
                self.assertEqual(out.size, (40, 20), kind)
        self.assertEqual(mc.transition(a, b, "fade", 1.0).getpixel((5, 5)), (255, 255, 255))
        self.assertEqual(mc.transition(a, b, "fade", 0.0).getpixel((5, 5)), (0, 0, 0))
        mid = mc.transition(a, b, "wipe_right", 0.5)
        self.assertEqual((mid.getpixel((2, 5)), mid.getpixel((38, 5))), ((255, 255, 255), (0, 0, 0)))


class WorkFolder(unittest.TestCase):
    def test_only_a_folder_inside_work_or_preview_is_removable(self):
        root = tempfile.mkdtemp(prefix="mv-motion-test-")
        self.addCleanup(shutil.rmtree, root, True)
        motion = os.path.join(root, "motion")
        self.assertTrue(mc.removable_work_dir(os.path.join(motion, ".work", "job-1")))
        self.assertTrue(mc.removable_work_dir(os.path.join(motion, ".preview", "work-5")))
        for bad in (os.path.join(motion, ".work", ".."), os.path.join(motion, ".work", "."), os.path.join(motion, ".work"),
                    motion, os.path.join(motion, "other", "x"), "", None):
            self.assertFalse(mc.removable_work_dir(bad), bad)

    def test_a_dot_dot_work_dir_never_deletes_the_motion_folder(self):
        root = tempfile.mkdtemp(prefix="mv-motion-test-")
        self.addCleanup(shutil.rmtree, root, True)
        motion = os.path.join(root, "motion")
        os.makedirs(os.path.join(motion, ".work"))
        keep = os.path.join(motion, "earlier render.mp4")
        with open(keep, "wb") as f:
            f.write(b"x")
        mc.emit = lambda *_: None
        self.addCleanup(setattr, mc, "emit", REAL_EMIT)
        spec = {"width": 64, "height": 36, "scenes": [{"duration": 1, "layers": [{"type": "text", "text": "Hi"}]}]}
        frame = os.path.join(root, "f.png")
        self.assertEqual(mc.render({"mode": "frame", "spec": spec, "frame": frame, "work_dir": os.path.join(motion, ".work", "..")}), 0)
        self.assertTrue(os.path.isfile(keep))
        job_dir = os.path.join(motion, ".work", "job-1")
        self.assertEqual(mc.render({"mode": "frame", "spec": spec, "frame": frame, "work_dir": job_dir}), 0)
        self.assertFalse(os.path.exists(job_dir))
        self.assertTrue(os.path.isfile(keep))


@unittest.skipUnless(HAS_FFMPEG, "needs FFmpeg")
class Rendering(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="mv-motion-test-")
        self.png = os.path.join(self.dir, "panel.png")
        img = Image.new("RGB", (120, 80), (30, 60, 200))
        for x in range(40, 80):
            for y in range(20, 60):
                img.putpixel((x, y), (250, 200, 40))
        img.save(self.png)

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def spec(self):
        return {
            "width": 160, "height": 90, "fps": 10, "background": "#101018",
            "scenes": [
                {"duration": 1.0, "layers": [
                    {"type": "image", "src": {"path": self.png}, "fit": "contain", "effects": [{"type": "glow", "strength": 0.8, "color": "#ffffff"}, {"type": "edge_glow", "color": "#ff3355"}],
                     "keyframes": [{"t": 0, "scale": 0.8, "opacity": 0}, {"t": 0.5, "scale": 1.0, "opacity": 1, "ease": "back"}]},
                    {"type": "particles", "kind": "embers", "count": 12},
                ], "effects": [{"type": "vignette", "strength": 0.5}, {"type": "grade", "preset": "warm"}]},
                {"duration": 1.0, "transition": {"type": "fade", "duration": 0.3}, "background": {"color": "#000000", "image": {"path": self.png}, "blur": 8, "dim": 0.5},
                 "layers": [
                     {"type": "text", "text": "HELLO", "size": 0.2, "font": "black", "stroke": {"color": "#000000", "width": 0.1}, "typewriter": True, "end": 1.0},
                     {"type": "shape", "shape": "rect", "w": 0.5, "h": 0.05, "y": 0.85, "color": "#ff3355", "radius": 0.5, "effects": [{"type": "pulse"}]},
                 ], "effects": [{"type": "flash", "at": 0.1, "duration": 0.2}, {"type": "shake", "at": 0, "duration": 0.5, "strength": 0.5}, {"type": "letterbox", "size": 0.1}]},
            ],
            "audio": {"sfx": [{"name": "whoosh", "at": 0.6}], "narration": [], "music": None},
        }

    def test_a_tiny_spec_renders_to_video_and_preview(self):
        out = os.path.join(self.dir, "out", "tiny.mp4")
        frame = os.path.join(self.dir, "out", "tiny.png")
        job = {"mode": "full", "spec": self.spec(), "out": out, "frame": frame, "frame_at": 0.4, "scale": 1.0, "work_dir": os.path.join(self.dir, ".work", "job-1")}
        lines = []
        real_emit = mc.emit
        mc.emit = lambda obj: lines.append(obj)
        try:
            code = mc.render(job)
        finally:
            mc.emit = REAL_EMIT
        done = lines[-1]
        self.assertEqual(code, 0, lines)
        self.assertEqual(done["type"], "done")
        self.assertTrue(os.path.isfile(out) and os.path.getsize(out) > 1000)
        with Image.open(frame) as im:
            self.assertEqual(im.size, (160, 90))
        self.assertEqual(done["encoder"], "libx264")
        self.assertAlmostEqual(done["duration"], 1.7, places=3)
        d = probe_duration(out)
        if d is not None:
            self.assertAlmostEqual(d, 1.7, delta=0.25)
        if has_audio(out) is not None:
            self.assertTrue(has_audio(out), "the sound effect was mixed in")
        self.assertTrue(any(l["type"] == "progress" for l in lines))
        self.assertFalse(os.path.exists(job["work_dir"]), "the work folder is removed")

    def test_frames_differ_over_time_and_preview_modes(self):
        r = mc.Renderer(self.spec(), 1.0)
        f0, f1 = r.frame(0.0, 0), r.frame(0.5, 5)
        self.assertNotEqual(list(f0.getdata()), list(f1.getdata()), "the layer animates")
        frame = os.path.join(self.dir, "f.png")
        lines = []
        mc.emit = lambda obj: lines.append(obj)
        try:
            self.assertEqual(mc.render({"mode": "frame", "spec": self.spec(), "frame": frame, "frame_at": 1.2, "scale": 0.5}), 0)
            clip = os.path.join(self.dir, "c.mp4")
            self.assertEqual(mc.render({"mode": "clip", "spec": self.spec(), "out": clip, "frame": None, "start": 0.5, "seconds": 3, "scale": 0.5}), 0)
        finally:
            mc.emit = REAL_EMIT
        with Image.open(frame) as im:
            self.assertEqual(im.size, (80, 44))
        self.assertTrue(os.path.isfile(clip))
        d = probe_duration(clip)
        if d is not None:
            self.assertAlmostEqual(d, 1.2, delta=0.25, msg="the clip stops at the end of the video")

    def test_a_bad_spec_is_reported_not_rendered(self):
        lines = []
        mc.emit = lambda obj: lines.append(obj)
        try:
            code = mc.render({"mode": "full", "spec": {"scenes": [{"duration": 1, "layers": [{"type": "image", "src": "C:/missing.png"}]}]}, "out": os.path.join(self.dir, "x.mp4")})
        finally:
            mc.emit = REAL_EMIT
        self.assertEqual(code, 1)
        self.assertEqual(lines[-1]["type"], "error")
        self.assertIn("file not found", lines[-1]["errors"][0])


if __name__ == "__main__":
    unittest.main()
