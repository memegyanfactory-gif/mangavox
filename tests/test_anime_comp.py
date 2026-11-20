"""Anime compositing (python/anime_comp.py): cutting pages into panels, the camera at the measured
speeds, keying, the animatic's panel views, and the edit's timing. The compositing tests run on
the CPU (never the graphics card) and are skipped without PyTorch; the encode test without FFmpeg."""
import contextlib
import io
import json
import os
import shutil
import subprocess
import tempfile
import unittest

import numpy as np
from PIL import Image

os.environ["MANGAVOX_ANIME_GPU"] = "0"

from python import anime_comp as ac  # noqa: E402

try:
    import torch  # noqa: F401

    HAVE_TORCH = True
except ImportError:
    HAVE_TORCH = False
HAVE_FFMPEG = shutil.which("ffmpeg") is not None and shutil.which("ffprobe") is not None


def page(w, h, boxes, bg=255):
    """A page: flat paper with panels of noisy grey at `boxes` (x0, y0, x1, y1)."""
    rng = np.random.default_rng(3)
    g = np.full((h, w), float(bg), dtype=np.float32)
    for k, (x0, y0, x1, y1) in enumerate(boxes):
        g[y0:y1, x0:x1] = 60 + 20 * k + rng.integers(0, 60, size=(y1 - y0, x1 - x0))
    return g


class PanelTests(unittest.TestCase):
    def test_a_page_is_cut_at_its_gutters_in_reading_order(self):
        boxes = [(20, 20, 290, 380), (310, 20, 580, 380), (20, 400, 580, 780)]
        g = page(600, 800, boxes)
        ltr = ac.find_panels(g, rtl=False)
        self.assertEqual(len(ltr), 3)
        # Top row left to right, then the wide panel under it.
        self.assertLess(ltr[0][0], ltr[1][0])
        self.assertGreater(ltr[2][1], ltr[0][3] - 5)
        for found, want in zip(ltr, boxes):
            self.assertTrue(all(abs(a - b) <= 3 for a, b in zip(found, want)), (found, want))
        rtl = ac.find_panels(g, rtl=True)
        self.assertGreater(rtl[0][0], rtl[1][0], "Japanese order reads the right panel first")

    def test_a_webtoon_strip_is_cut_at_its_flat_bands(self):
        g = page(700, 3000, [(0, 100, 700, 900), (40, 1100, 660, 1500), (0, 1800, 700, 2900)], bg=0)
        found = ac.webtoon_panels(g)
        self.assertEqual([(y0, y1) for _, y0, _, y1 in found], [(100, 900), (1100, 1500), (1800, 2900)])
        self.assertEqual((found[1][0], found[1][2]), (40, 660), "a narrow panel is trimmed to its art")

    def test_the_command_writes_panels_with_their_sizes(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "page.png")
            Image.fromarray(page(600, 800, [(20, 20, 580, 380), (20, 400, 580, 780)]).astype(np.uint8)).save(p)
            said = io.StringIO()
            with contextlib.redirect_stdout(said):
                # A page that cannot be read is skipped, not the chapter.
                ac.cmd_panels({"pages": [p, os.path.join(d, "missing.png")], "out_dir": os.path.join(d, "out"), "rtl": False})
            last = json.loads(said.getvalue().strip().splitlines()[-1])
            self.assertTrue(last["ok"], said.getvalue())
            self.assertEqual([(x["page"], x["w"] > 500, 340 < x["h"] < 400) for x in last["panels"]], [(0, True, True), (0, True, True)])
            self.assertTrue(all(os.path.isfile(x["file"]) for x in last["panels"]))


class CameraTests(unittest.TestCase):
    def test_moves_start_and_end_where_they_should(self):
        # A pan right moves the view from left of centre to right of it, easing in and out.
        self.assertEqual(ac.camera_at("pan_right", 0.0, 0.2, 0.0), (1.0, -0.1, 0.0))
        self.assertEqual(ac.camera_at("pan_right", 1.0, 0.2, 0.0), (1.0, 0.1, 0.0))
        self.assertEqual(ac.camera_at("pan_left", 1.0, 0.2, 0.0), (1.0, -0.1, 0.0))
        self.assertAlmostEqual(ac.camera_at("pan_right", 0.5, 0.2, 0.0)[1], 0.0)
        self.assertLess(abs(ac.camera_at("pan_right", 0.1, 0.2, 0.0)[1] - -0.1), 0.01, "it eases in")
        self.assertEqual(ac.camera_at("tilt_up", 0.0, 0.3, 0.0)[2], 0.15)
        self.assertEqual(ac.camera_at("tilt_down", 0.0, 0.3, 0.0)[2], -0.15)
        self.assertEqual((ac.camera_at("push_in", 0.0, 0.0, 0.04)[0], ac.camera_at("push_in", 1.0, 0.0, 0.04)[0]), (1.0, 1.04))
        self.assertEqual((ac.camera_at("pull_out", 0.0, 0.0, 0.04)[0], ac.camera_at("pull_out", 1.0, 0.0, 0.04)[0]), (1.04, 1.0))
        # A held cut is truly held; a track-in is mostly done early.
        self.assertEqual(ac.camera_at("fix", 0.7, 0.5, 0.5), (1.0, 0.0, 0.0))
        self.assertGreater(ac.camera_at("track_in", 0.3, 0.0, 0.2)[0], 1.13)

    def test_the_app_s_extents_win_over_the_defaults(self):
        self.assertEqual(ac.camera_extent({"move": "pan_left", "travel": 0.4, "zoom": 0.0}), (0.4, 0.0))
        self.assertEqual(ac.camera_extent({"move": "pan_left"}), (0.12, 0.0))
        self.assertEqual(ac.camera_extent({"move": "push_in", "amount": 2}), (0.0, 0.08))
        self.assertEqual(ac.camera_extent({"move": "fix"}), (0.0, 0.0))

    def test_a_layer_is_enlarged_only_as_much_as_the_move_needs(self):
        # A 16:9 plate under a pan of a fifth of the frame is enlarged by a fifth...
        self.assertAlmostEqual(ac.base_zoom("pan_right", 0.2, 1920, 1080, 1920, 1080), 1.2)
        # ...a plate painted a third wider is not enlarged at all...
        self.assertAlmostEqual(ac.base_zoom("pan_right", 0.2, 2560, 1080, 1920, 1080), 1.0)
        # ...nor is a tall plate under a tilt, though a pan over it still needs room.
        self.assertAlmostEqual(ac.base_zoom("tilt_up", 0.5, 1920, 2160, 1920, 1080), 1.0)
        self.assertAlmostEqual(ac.base_zoom("pan_left", 0.1, 1920, 2160, 1920, 1080), 1.1)
        self.assertAlmostEqual(ac.base_zoom("fix", 0.0, 1920, 1080, 1920, 1080), 1.0)
        self.assertAlmostEqual(ac.base_zoom("fix", 0.0, 1920, 1080, 1920, 1080, shake=1.0), 1.03)


class EditPlanTests(unittest.TestCase):
    def test_hard_cuts_butt_and_soft_ones_overlap(self):
        starts, reels, total = ac.edit_plan([(2.0, "cut"), (3.0, "cut"), (4.0, "dissolve"), (1.0, "cut"), (2.0, "black")], 24)
        self.assertEqual(starts[:2], [0.0, 2.0])
        self.assertAlmostEqual(starts[2], 5.0 - 0.5)
        self.assertAlmostEqual(starts[3], 8.5)
        self.assertAlmostEqual(starts[4], 9.5 - 0.5, msg="a fade is never longer than half the shorter cut")
        self.assertAlmostEqual(total, 11.0)
        self.assertEqual([(r["first"], r["count"]) for r in reels], [(0, 2), (2, 2), (4, 1)])
        self.assertEqual((reels[0]["xf"], reels[1]["xf"][0], reels[2]["xf"][0]), (None, "fade", "fadeblack"))
        # The first cut's own transition has nothing to fade from.
        starts, reels, total = ac.edit_plan([(2.0, "dissolve"), (2.0, "cut")], 24)
        self.assertEqual((starts, len(reels), total), ([0.0, 2.0], 1, 4.0))
        # Transitions last whole frames.
        _, reels, _ = ac.edit_plan([(1.0, "cut"), (0.1, "smash")], 24)
        self.assertAlmostEqual(reels[1]["xf"][1] * 24, round(reels[1]["xf"][1] * 24))


@unittest.skipUnless(HAVE_TORCH, "PyTorch is not installed")
class CompositeTests(unittest.TestCase):
    W, H = 320, 180

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="anime-comp-test-")
        self.addCleanup(shutil.rmtree, self.dir, True)

    def save(self, name, arr):
        p = os.path.join(self.dir, name)
        Image.fromarray(arr.astype(np.uint8)).save(p)
        return p

    def screen(self, colour, square):
        a = np.zeros((self.H, self.W, 3), dtype=np.uint8)
        a[:] = colour
        a[60:120, 130:190] = square
        return a

    def test_green_and_magenta_screens_key_out_and_the_character_stays(self):
        for colour, square in (((0, 177, 64), (200, 40, 40)), ((255, 0, 255), (30, 160, 60))):
            rgb, alpha = ac.chroma_key(ac.tens(self.screen(colour, square), ac.device()))
            a = alpha[0].numpy()
            self.assertLess(a[10:40, 10:60].mean(), 0.02, f"the {colour} screen is gone")
            self.assertGreater(a[75:105, 145:175].mean(), 0.98, "the character is solid")
            inside = rgb[:, 75:105, 145:175].mean(dim=(1, 2)).numpy() * 255
            self.assertTrue(np.allclose(inside, square, atol=6), (inside, square))

    def job(self, **more):
        job = {"out": os.path.join(self.dir, "o.png"), "width": self.W, "height": self.H, "fps": 24, "seconds": 1.0, "finish": {"bloom": 0, "diffusion": 0, "grain": 0, "ca": 0, "vignette": 0}}
        job.update(more)
        return job

    def test_a_character_is_seen_through_the_same_camera_as_the_plate(self):
        plate = np.zeros((self.H, self.W, 3), dtype=np.uint8)
        plate[:, :, 2] = np.linspace(40, 220, self.W)[None, :]  # a blue ramp, darker on the left
        cut = ac.Cut(self.job(plate={"path": self.save("plate.png", plate)}, chars=[{"path": self.save("char.png", self.screen((0, 177, 64), (220, 30, 30))), "shadow": False, "match": 0, "wrap": 0}], camera={"move": "pan_right", "travel": 0.25, "zoom": 0}))
        self.assertAlmostEqual(cut.z0, 1.25)
        with torch.no_grad():
            first, last = ac.to_u8(cut.render(0)), ac.to_u8(cut.render(cut.frames() - 1))
        red = lambda f: np.argwhere((f[:, :, 0] > 150) & (f[:, :, 1] < 90))[:, 1].mean()  # noqa: E731
        # The view moves right: the character and the plate slide left together, by the same amount.
        slide = red(first) - red(last)
        self.assertAlmostEqual(slide, 0.25 * self.W, delta=3)
        col = 40
        self.assertLess(int(first[5, col, 2]), int(last[5, col, 2]), "the plate's brighter right side comes into view")
        ramp_shift = (int(last[5, col, 2]) - int(first[5, col, 2])) / (180 / self.W) * cut.z0
        self.assertAlmostEqual(ramp_shift, slide, delta=6)
        self.assertEqual(first[90, 5, 1], 0, "no green is left anywhere")

    def test_the_animatic_shows_a_panel_whole_or_travels_it(self):
        tall = np.zeros((900, 300, 3), dtype=np.uint8)
        tall[:300] = (250, 0, 0)
        tall[300:600] = (0, 250, 0)
        tall[600:] = (0, 0, 250)
        path = self.save("tall.png", tall)
        whole = ac.Cut(self.job(base={"path": path, "fit": "contain"}, camera={"move": "fix"}))
        with torch.no_grad():
            f = ac.to_u8(whole.render(0))
        mid = f[:, self.W // 2]
        # All three bands are in the frame, top to bottom, in the middle; a dim blur fills the sides.
        self.assertTrue(mid[10, 0] > 200 and mid[self.H // 2, 1] > 200 and mid[self.H - 10, 2] > 200, mid[::30])
        self.assertLess(int(f[self.H // 2, 5].max()), 130)
        scan = ac.Cut(self.job(base={"path": path, "fit": "scan"}, camera={"move": "tilt_down", "travel": 0.1}))
        self.assertEqual(scan.z0, 1.0)
        self.assertAlmostEqual(scan.travel, 900 * (self.W / 300) / self.H - 1)
        with torch.no_grad():
            top, bottom = ac.to_u8(scan.render(0)), ac.to_u8(scan.render(scan.frames() - 1))
        self.assertGreater(top[self.H // 2, self.W // 2, 0], 200, "it starts on the top of the panel")
        self.assertGreater(bottom[self.H // 2, self.W // 2, 2], 200, "and ends on its bottom")
        up = ac.Cut(self.job(base={"path": path, "fit": "scan"}, camera={"move": "tilt_up"}))
        with torch.no_grad():
            self.assertGreater(ac.to_u8(up.render(0))[self.H // 2, self.W // 2, 2], 200, "a tilt up starts at the bottom")

    def test_a_card_and_a_slate_are_drawn(self):
        card = ac.Cut(self.job(card={"title": "Cut 3 · added by the director", "text": "The gate site from above, tiny figures for scale."}, slate="C003  3.5s  establishing / high  tilt down  camera"))
        with torch.no_grad():
            f = ac.to_u8(card.render(0))
        self.assertGreater((f.max(axis=2) > 200).sum(), 150, "there is text on the card")
        self.assertLess(int(f[5, 5].max()), 60, "on a dark ground")
        self.assertGreater((f[self.H - 30:self.H - 8, 8:240].max(axis=2) > 200).sum(), 20, "the slate sits in the lower left")
        self.assertEqual((f[self.H - 30:self.H - 8, 280:].max(axis=2) > 200).sum(), 0, "and only there")

    @unittest.skipUnless(HAVE_FFMPEG, "FFmpeg is not installed")
    def test_cuts_are_encoded_and_edited_to_the_frame(self):
        vids = []
        for k, colour in enumerate(((200, 60, 60), (60, 200, 60), (60, 60, 200))):
            still = np.zeros((self.H, self.W, 3), dtype=np.uint8)
            still[:] = colour
            out = os.path.join(self.dir, f"cut{k}.mp4")
            job = self.job(out=out, seconds=0.5, quality="rough", base={"path": self.save(f"s{k}.png", still), "fit": "contain"}, camera={"move": "push_in", "zoom": 0.05})
            with contextlib.redirect_stdout(io.StringIO()):
                ac.cmd_comp(job)
            self.assertTrue(os.path.isfile(out))
            vids.append(out)
        final = os.path.join(self.dir, "episode.mp4")
        said = io.StringIO()
        with contextlib.redirect_stdout(said):
            ac.cmd_edit({"out": final, "width": self.W, "height": self.H, "fps": 24, "quality": "rough", "cuts": [{"video": vids[0], "seconds": 0.5, "transition": "cut"}, {"video": vids[1], "seconds": 0.5, "transition": "cut"}, {"video": vids[2], "seconds": 1.0, "transition": "dissolve"}]})
        done = json.loads(said.getvalue().strip().splitlines()[-1])
        self.assertEqual((done["seconds"], done["starts"]), (1.75, [0.0, 0.5, 0.75]))
        r = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0", final], capture_output=True, text=True)
        # 12 + 12 + 24 frames, less the six the dissolve overlaps (a quarter second: half the shorter cut).
        self.assertEqual(int(r.stdout.strip()), 42, r.stderr)
        # The hard cut is exactly hard: frame 11 is the first colour, frame 12 the second.
        grab = lambda n: np.frombuffer(subprocess.run(["ffmpeg", "-v", "error", "-i", final, "-vf", f"select=eq(n\\,{n})", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout, np.uint8).reshape(self.H, self.W, 3)[self.H // 2, self.W // 2]  # noqa: E731
        self.assertGreater(int(grab(11)[0]), 150)
        self.assertGreater(int(grab(12)[1]), 150)
        self.assertLess(int(grab(12)[0]), 110)


if __name__ == "__main__":
    unittest.main()


class LayoutGroundTests(unittest.TestCase):
    """A layer lifted from a layout frame: its shadows go under it, or the layout is the plate."""

    def setUp(self):
        self.t = ac.torch_mod()
        self.dev = "cpu"
        h, w = 180, 320
        self.bg = self.t.full((3, h, w), 0.7)
        self.alpha = self.t.zeros((1, h, w))
        self.alpha[:, 60:150, 140:180] = 1.0  # a figure

    def test_the_layouts_shadow_is_carried_under_the_layer(self):
        lay = self.bg.clone()
        lay[:, 152:162, 100:220] = 0.35  # a cast shadow on the ground by the feet
        plate, shade = ac.layout_ground(self.bg, lay, self.alpha)
        self.assertTrue(self.t.equal(plate, self.bg))
        self.assertIsNotNone(shade)
        self.assertGreater(float(shade[0, 157, 125]), 0.2)
        self.assertLess(float(shade[0, 70, 130]), 0.01, "not around the head")
        self.assertLess(float(shade[0, 20, 20]), 0.01)

    def test_a_drifted_background_gives_way_to_the_layout(self):
        lay = self.bg.clone() * 0.5  # the whole place came back different
        plate, shade = ac.layout_ground(self.bg, lay, self.alpha)
        self.assertIsNone(shade)
        self.assertAlmostEqual(float(plate.mean()), 0.35, places=2)
