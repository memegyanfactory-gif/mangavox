"""The renderer's per-frame panel camera, pinned by tests/fixtures/frame_transforms.json.

The Video editor's preview (native/crates/ui-studio/src/vedit_frame.rs) checks its own camera
against the same file, so the preview and the rendered video move the panel the same way.
Regenerate the file after a deliberate change with:  py -3 tests/test_frame_transforms.py --write
"""
import json
import math
import os
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "python"))
sys.path.insert(0, ROOT)

from python.video_generator import clip_camera  # noqa: E402

FIXTURE = os.path.join(ROOT, "tests", "fixtures", "frame_transforms.json")

# (name, clip as the renderer gets it, frames)
CASES = [
    ("auto push", {"motion": "push", "push": 0.05}, 90),
    ("auto pan right, short, motion 150%", {"motion": "pan_right", "push": 0.05, "motion_k": 1.5}, 60),
    ("auto pull, ramp, short", {"motion": "pull", "push": 0.08, "ramp": True}, 45),
    ("auto pan down, ramp, long", {"motion": "pan_down", "push": 0.08, "ramp": True}, 150),
    ("still", {"motion": "still", "push": 0.05}, 40),
    ("zoom 12% into 30,40", {"motion": "edit", "push": 0.05, "move": {"kind": "zoom", "amount": 0.12, "from": None, "to": [0.3, 0.4]}}, 90),
    ("zoom out, look's amount", {"motion": "edit", "push": 0.06, "move": {"kind": "zoom_out", "amount": None, "from": None, "to": None}}, 120),
    ("pan up 20%", {"motion": "edit", "push": 0.05, "move": {"kind": "pan_up", "amount": 0.2, "from": None, "to": None}}, 75),
    ("pan left, default", {"motion": "edit", "push": 0.05, "move": {"kind": "pan_left", "amount": None, "from": None, "to": None}}, 75),
    ("ken burns, ramp", {"motion": "edit", "push": 0.08, "ramp": True, "move": {"kind": "ken_burns", "amount": 0.11, "from": [0.3, 0.35], "to": [0.7, 0.62]}}, 100),
    ("ken burns, default path", {"motion": "edit", "push": 0.05, "move": {"kind": "ken_burns", "amount": None, "from": None, "to": None}}, 80),
    ("punch 15%", {"motion": "edit", "push": 0.05, "move": {"kind": "punch", "amount": 0.15, "from": None, "to": [0.5, 0.42]}}, 60),
    ("shake 20% over a pan", {"motion": "pan_left", "push": 0.08, "impact": True, "punch": 0.2}, 50),
    ("auto action punch", {"motion": "push", "push": 0.05, "impact": True}, 70),
]


def camera_at(clip, frames, on):
    """(zoom, x, y) the renderer's expressions give at frame `on`: the visible rectangle is
    1/zoom of the panel, at x and y of the slack the zoom leaves."""
    zoom, window, ease = clip_camera(clip, frames)
    names = {"on": on, "PI": math.pi, "cos": math.cos, "pow": pow, "min": min, "max": max, "__builtins__": {}}
    ev = lambda s: eval(s.format(e=f"({ease})"), names)  # noqa: E731
    return [round(ev(zoom), 6), round(ev(window[0]), 6), round(ev(window[1]), 6)]


def samples(frames):
    return sorted({0, 3, frames // 2, frames - 1})


def build():
    out = []
    for name, clip, frames in CASES:
        out.append({
            "name": name, "clip": clip, "frames": frames,
            "expect": [[f, *camera_at(clip, frames, f)] for f in samples(frames)],
        })
    return {"about": "Panel camera (frame, zoom, x, y) per case: python/video_generator.clip_camera and ui-studio vedit_frame::camera must both give these.", "cases": out}


class FrameTransformTests(unittest.TestCase):
    def test_the_renderer_matches_the_shared_fixture(self):
        with open(FIXTURE, encoding="utf-8") as f:
            fixture = json.load(f)
        self.assertEqual([c["name"] for c in fixture["cases"]], [c[0] for c in CASES])
        for case in fixture["cases"]:
            for frame, *want in case["expect"]:
                got = camera_at(case["clip"], case["frames"], frame)
                for g, w in zip(got, want):
                    self.assertAlmostEqual(g, w, places=4, msg=f"{case['name']} at frame {frame}")

    def test_moves_go_where_they_say(self):
        # Pushing into the top-left aims the window there and ends 12% in.
        z, x, y = camera_at(CASES[5][1], 90, 89)
        self.assertAlmostEqual(z, 1.12, places=4)
        self.assertEqual((x, y), (0.3, 0.4))
        # Pan up starts at the bottom of the slack and ends at the top.
        self.assertEqual(camera_at(CASES[7][1], 75, 0)[2], 1.0)
        self.assertEqual(camera_at(CASES[7][1], 75, 74)[2], 0.0)
        # A punch is all the way in after PUNCH_SEC (8 frames), then drifts on.
        self.assertGreater(camera_at(CASES[11][1], 60, 8)[0], 1.15)


if __name__ == "__main__":
    if "--write" in sys.argv:
        os.makedirs(os.path.dirname(FIXTURE), exist_ok=True)
        data = build()
        with open(FIXTURE, "w", encoding="utf-8") as f:
            f.write('{\n "about": ' + json.dumps(data["about"]) + ',\n "cases": [\n  ')
            f.write(",\n  ".join(json.dumps(c) for c in data["cases"]))
            f.write("\n ]\n}\n")
        print(f"wrote {FIXTURE}")
    else:
        unittest.main()
