import os
import tempfile
import unittest

from python.hook_maker import FLASH, PUNCH, TO_OUTRO, WHIPS, edit_line, split_words

LINE = "One boy stands between the city and the end of the world."


class HookEditTests(unittest.TestCase):
    def setUp(self):
        f = tempfile.NamedTemporaryFile(suffix=".jpg", delete=False)
        f.close()
        self.img = f.name
        self.shot = {"text": LINE, "image": self.img, "more": [{"image": self.img}, {"image": self.img}]}

    def tearDown(self):
        os.unlink(self.img)

    def test_words_are_split_into_whole_runs(self):
        self.assertEqual(" ".join(split_words(LINE, 3)), LINE)
        self.assertEqual(len(split_words(LINE, 3)), 3)
        self.assertEqual(split_words("He was wrong.", 5), ["He", "was", "wrong."])
        self.assertEqual(split_words("Hi", 1), ["Hi"])

    def test_a_long_line_cuts_to_the_next_panels_and_keeps_its_timing(self):
        clips, punched = edit_line(2, self.shot, 4.1, True)
        self.assertTrue(punched)
        self.assertEqual(len(clips), 2)
        self.assertEqual(" ".join(c["text"] for c in clips), LINE)
        self.assertAlmostEqual(sum(c["seconds"] for c in clips), 4.1)
        self.assertAlmostEqual(clips[1]["offset"], clips[0]["seconds"])
        self.assertEqual((clips[0]["motion"], clips[0]["amount"], clips[0]["transition"]), ("shake", PUNCH, FLASH))
        self.assertTrue(clips[1]["cut"])
        self.assertTrue(all(c["layout"] == "single" for c in clips))

    def test_a_short_calm_line_whips_in_on_one_picture(self):
        clips, punched = edit_line(1, self.shot, 1.5, False)
        self.assertFalse(punched)
        self.assertEqual(len(clips), 1)
        self.assertIn(clips[0]["transition"], WHIPS)
        self.assertNotIn("motion", clips[0])
        # The first line has nothing to come in from.
        self.assertNotIn("transition", edit_line(0, self.shot, 1.5, True)[0][0])

    def test_the_outro_fades_through_black_on_one_picture(self):
        clips, _ = edit_line(6, dict(self.shot, layout="single"), 3.0, False)
        self.assertEqual(len(clips), 1)
        self.assertEqual(clips[0]["transition"], TO_OUTRO)

    def test_missing_cut_in_pictures_are_skipped(self):
        shot = dict(self.shot, more=[{"image": "missing.jpg"}])
        self.assertEqual(len(edit_line(1, shot, 6.0, False)[0]), 1)


if __name__ == "__main__":
    unittest.main()
