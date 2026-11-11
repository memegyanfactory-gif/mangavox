"""The panel upscaler's planning (which panels, at what size, kept where) and the renderer
carrying on without it. No network, no model, no GPU."""
import os
import tempfile
import unittest
from unittest import mock

from PIL import Image

from python import upscale
from python import video_generator as vg


def picture(folder, name, size):
    path = os.path.join(folder, name)
    Image.new("RGB", size, (120, 90, 60)).save(path, "JPEG")
    return path


class PlanTests(unittest.TestCase):
    def test_only_panels_shown_larger_than_their_scan_are_upscaled(self):
        # A wide panel fills a 1440p frame at 2.88x; a tall webtoon panel is shown smaller.
        self.assertAlmostEqual(upscale.needs((800, 500), (2304, 1440)), 2.88)
        self.assertIsNone(upscale.needs((800, 2400), (480, 1440)))
        self.assertIsNone(upscale.needs((800, 1300), (886, 1440)), "1.1x is left to the resize")
        self.assertIsNone(upscale.needs((4, 4), (2560, 1440)), "nothing to redraw")

    def test_the_kept_size_has_headroom_and_never_passes_the_model(self):
        self.assertEqual(upscale.out_size((800, 500), 2.0), (1760, 1100))
        self.assertEqual(upscale.out_size((800, 500), 9.0), (3200, 2000))

    def test_plan_names_a_copy_by_the_file_and_its_size(self):
        with tempfile.TemporaryDirectory() as d:
            wide = picture(d, "p0001_00.jpg", (800, 500))
            tall = picture(d, "p0001_01.jpg", (800, 2400))
            jobs, where = upscale.plan([wide, tall, wide, os.path.join(d, "missing.jpg")], 2560, 1440, os.path.join(d, "up"))
            self.assertEqual([j["src"] for j in jobs], [wide], "the tall one is shown at its own size, the missing one skipped")
            self.assertEqual(jobs[0]["size"], [2534, 1584])
            self.assertEqual(where, {wide: jobs[0]["dst"]})
            # The same file at another video size, or the file changed, is another copy.
            again, _ = upscale.plan([wide], 1920, 1080, os.path.join(d, "up"))
            self.assertNotEqual(again[0]["dst"], jobs[0]["dst"])
            Image.new("RGB", (800, 500), (0, 0, 0)).save(wide, "JPEG", quality=30)
            os.utime(wide, (1, 1))
            changed, _ = upscale.plan([wide], 2560, 1440, os.path.join(d, "up"))
            self.assertNotEqual(changed[0]["dst"], jobs[0]["dst"])

    def test_it_can_be_turned_off(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MANGAVOX_UPSCALE", None)
            self.assertTrue(upscale.wanted({}))
            self.assertTrue(upscale.wanted(None))
            self.assertFalse(upscale.wanted({"upscale": False}))
        with mock.patch.dict(os.environ, {"MANGAVOX_UPSCALE": "0"}):
            self.assertFalse(upscale.wanted({"upscale": True}))


class RendererTests(unittest.TestCase):
    def test_without_the_pack_nothing_is_upscaled(self):
        with tempfile.TemporaryDirectory() as d:
            wide = picture(d, "p0001_00.jpg", (800, 500))
            with mock.patch.object(vg.layered_plan, "pack_python", return_value=(None, "not installed")):
                self.assertEqual(vg.upscale_panels([wide], 2560, 1440, d, {}, d), {})
            self.assertFalse(os.path.isdir(os.path.join(d, "studio", "upscaled")))

    def test_kept_copies_are_reused_and_stale_ones_removed(self):
        with tempfile.TemporaryDirectory() as d:
            wide = picture(d, "p0001_00.jpg", (800, 500))
            folder = os.path.join(d, "studio", "upscaled")
            jobs, _ = upscale.plan([wide], 2560, 1440, folder)
            os.makedirs(folder)
            Image.new("RGB", (64, 40)).save(jobs[0]["dst"], "JPEG")
            stale = picture(folder, "0123456789abcdef.jpg", (8, 8))
            # The copy is there already: the pack is never started.
            with mock.patch.object(vg.layered_plan, "pack_python", return_value=("python-that-is-not-there", None)), \
                    mock.patch.object(vg.subprocess, "run", side_effect=AssertionError("started the pack")):
                self.assertEqual(vg.upscale_panels([wide], 2560, 1440, d, {}, d), {wide: jobs[0]["dst"]})
            self.assertFalse(os.path.exists(stale))

    def test_a_pack_that_fails_leaves_the_panels_to_the_resize(self):
        with tempfile.TemporaryDirectory() as d:
            wide = picture(d, "p0001_00.jpg", (800, 500))
            with mock.patch.object(vg.layered_plan, "pack_python", return_value=("python-that-is-not-there", None)), \
                    mock.patch.object(vg.subprocess, "run", side_effect=OSError("cannot start")):
                self.assertEqual(vg.upscale_panels([wide], 2560, 1440, d, {}, d), {})


if __name__ == "__main__":
    unittest.main()
