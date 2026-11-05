"""The ribbon transition (intro_transition): seeded from any frame and a mask of what turns into
light (the last cast card), it is exactly as long as asked at any frame rate, the same every time,
ends exactly on the recap's first frame (or in the dark without one), and its sound peaks as the
camera passes through the ribbons. Rendered small so it runs quickly."""
import colorsys
import hashlib
import os
import re
import sys
import unittest

import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "python"))
import intro_transition as tr  # noqa: E402

W, H = 192, 108
PALETTE = {"primary": "#df8d4a", "glow": "#ed9c5a", "secondary": "#e8ba4a", "accent": "#eb8633", "light": "#e6c8bc", "dark": "#131112"}
CARD = [(30, 30), (170, 26), (170, 86), (30, 90)]
FIGURE = (50, 12, 70, 95)


def card():
    """A small last card: a tilted backdrop with a figure standing in front of it, on black, and
    its mask (the card and the figure)."""
    im = Image.new("RGB", (W, H), (0, 0, 0))
    d = ImageDraw.Draw(im)
    d.polygon(CARD, fill=(120, 140, 200))
    d.rectangle(FIGURE, fill=(230, 180, 60))
    m = Image.new("L", (W, H), 0)
    dm = ImageDraw.Draw(m)
    dm.polygon(CARD, fill=255)
    dm.rectangle(FIGURE, fill=255)
    return np.asarray(im), np.asarray(m, np.float32) / 255


def reveal():
    yy, xx = np.mgrid[0:H, 0:W]
    return np.stack([(xx * 255 // W), (yy * 255 // H), np.full((H, W), 90)], -1).astype(np.uint8)


def dive(fps=30.0, recap=True):
    """The transition out of card(), onto reveal() (or onto nothing)."""
    src, mask = card()
    return tr.RibbonDive(W, H, src, mask, PALETTE, reveal() if recap else None, fps)


class TimingTests(unittest.TestCase):
    def test_timing_lines(self):
        # The app reads these as plain "NAME = number" lines at the start of a line.
        with open(tr.__file__, encoding="utf-8") as f:
            src = f.read()
        vals = {k: float(re.search(rf"^{k} = ([0-9.]+)", src, re.M).group(1)) for k in ("RIBBON_SEC", "REVEAL_SEC", "PASS_SEC")}
        self.assertEqual((vals["RIBBON_SEC"], vals["REVEAL_SEC"]), (1.2, 0.5))
        self.assertTrue(0.5 < vals["PASS_SEC"] < vals["RIBBON_SEC"], vals)
        self.assertAlmostEqual(vals["PASS_SEC"], tr.PASS_SEC)
        self.assertAlmostEqual(tr.PASS_U * tr.RIBBON_SEC, tr.PASS_SEC)
        self.assertAlmostEqual(tr.total_seconds(), 1.7)
        # The pass comes after the lines part and before the centre clears onto the recap.
        self.assertTrue(tr.SPLIT_U[1] < tr.PASS_U < tr.OPEN_U)

    def test_as_many_frames_as_asked_at_any_rate(self):
        for fps in (24.0, 25.0, 30000 / 1001, 30.0, 60.0):
            n = int(round(tr.total_seconds() * fps))
            d = dive(fps=fps)
            got = list(d.frames(n))
            self.assertEqual(len(got), n, fps)
            self.assertTrue(all(f.shape == (H, W, 3) and f.dtype == np.uint8 for f in got), fps)
            # It lands exactly on the recap's first frame.
            self.assertTrue(np.array_equal(got[-1], reveal()), fps)


class LookTests(unittest.TestCase):
    def test_same_every_time(self):
        h1 = [hashlib.sha1(f.tobytes()).hexdigest() for f in dive().frames(51)]
        h2 = [hashlib.sha1(f.tobytes()).hexdigest() for f in dive().frames(51)]
        self.assertEqual(h1, h2)

    def test_grows_out_of_the_card(self):
        src, mask = card()
        got = list(dive().frames(51))
        # The first frame is still the card (the light has only just started across it)...
        self.assertLess(np.abs(got[0].astype(int) - src.astype(int)).mean(), 12)
        # ...a third of the way in its columns are lines of light, reaching past the card...
        third = got[int(51 * 0.33)].astype(int)
        self.assertGreater(third[:20].mean(), src[:20].mean() + 10)
        # ...and in the middle of the dive it is neither the card nor the recap.
        mid = got[int(51 * 0.45)].astype(int)
        self.assertGreater(np.abs(mid - src.astype(int)).mean(), 10)
        self.assertGreater(np.abs(mid - reveal().astype(int)).mean(), 10)

    def test_without_a_recap_it_ends_in_the_dark(self):
        got = list(dive(recap=False).frames(51))
        self.assertLess(got[-1].mean(), 2.0)
        # ...and it fades there rather than cutting: the frames before it are dark too.
        self.assertLess(np.mean([f.mean() for f in got[-5:]]), 12.0)

    def test_seeded_from_any_frame_and_mask(self):
        rng = np.random.default_rng(3)
        blobs = Image.fromarray(rng.integers(0, 255, (9, 16, 3), dtype=np.uint8))
        src = np.asarray(blobs.resize((W, H), Image.BICUBIC))  # any picture (smooth, so a slight push-in barely changes it)
        yy, xx = np.mgrid[0:H, 0:W]
        mask = (((xx - 120) / 40.0) ** 2 + ((yy - 50) / 30.0) ** 2 < 1).astype(np.float32)
        d = tr.RibbonDive(W, H, src, mask, {}, None, 25.0)
        self.assertTrue(80 <= d.lx <= 160 and 20 <= d.ly <= 80)  # the light comes from where the mask is
        got = list(d.frames(43))
        self.assertEqual(len(got), 43)
        self.assertLess(np.abs(got[0].astype(int) - src.astype(int)).mean(), 25)
        # An empty mask, an odd-sized source, no palette and no frame at all still work.
        for s, m in ((src, np.zeros((H, W), np.float32)), (src[:50, :90], None), (None, None)):
            frames = list(tr.RibbonDive(W, H, s, m, None, None, 30.0).frames(5))
            self.assertEqual(len(frames), 5)
            self.assertTrue(all(f.shape == (H, W, 3) for f in frames))


class SoundTests(unittest.TestCase):
    def rms(self, x, hop):
        return np.sqrt(np.convolve(np.mean(x ** 2, axis=1), np.ones(hop) / hop, mode="same"))[::hop]

    def test_the_whoosh_peaks_as_the_camera_passes_through(self):
        d = tr.sound()
        self.assertTrue(np.isfinite(d).all())
        self.assertEqual(d.shape[1], 2)
        self.assertGreaterEqual(len(d) / tr.SR, tr.total_seconds())
        hop = int(0.02 * tr.SR)
        rms = self.rms(d, hop)
        t = np.arange(len(rms)) * 0.02
        dive_part = t < tr.OPEN_U * tr.RIBBON_SEC  # before the release
        peak = t[dive_part][np.argmax(rms[dive_part])]
        self.assertLess(abs(peak - tr.PASS_SEC), 0.1)
        # ...and the release is there as the recap lands, fading out after.
        land = tr.OPEN_U * tr.RIBBON_SEC
        self.assertGreater(rms[int((land + 0.4) / 0.02)], 3 * rms[-10])

    def test_the_shimmer_follows_the_key(self):
        minor, major = tr.sound(55.0, minor=True), tr.sound(65.41, minor=False)
        self.assertFalse(np.allclose(minor[: tr.SR // 2], major[: tr.SR // 2]))
        self.assertTrue(np.array_equal(tr.sound(55.0, True), minor))  # the same every time


class PaletteTests(unittest.TestCase):
    def test_palette_gradient(self):
        # A single red widens into its neighbours (crimson to orange), two hue families meet through
        # a pale middle, and no palette at all still gives colours.
        stops, hot = tr.ribbon_palette({"primary": "#c50204", "secondary": "#980406", "glow": "#ff383a", "light": "#ebdfd6"})
        hues = [(colorsys.rgb_to_hls(*c)[0] + 0.5) % 1 for c in stops]
        self.assertGreater(max(hues) - min(hues), 0.12)
        stops, hot = tr.ribbon_palette({"primary": "#18df22", "secondary": "#d96d96", "light": "#c2dab2"})
        self.assertGreater(min(stops[len(stops) // 2]), 0.6)              # the pale middle is the middle
        self.assertLess(min(stops[0]), 0.3)
        self.assertLess(min(stops[-1]), 0.3)
        stops, hot = tr.ribbon_palette({})
        self.assertGreater(len(stops), 2)
        self.assertGreater(min(hot), 0.8)


if __name__ == "__main__":
    unittest.main()
