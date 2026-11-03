import os
import tempfile
import unittest

from python.hook_maker import FLASH, PUNCH, TO_OUTRO, WHIPS, edit_line

LINE = "This monster was made to wipe out humanity. He'd rather have ice cream."


class HookEditTests(unittest.TestCase):
    def setUp(self):
        f = tempfile.NamedTemporaryFile(suffix=".jpg", delete=False)
        f.close()
        self.img = f.name
        self.shot = {"text": LINE, "image": self.img}

    def tearDown(self):
        os.unlink(self.img)

    def test_each_line_is_on_its_own_panel_for_its_whole_voice(self):
        clips, punched = edit_line(2, self.shot, 4.1, True)
        self.assertTrue(punched)
        self.assertEqual(len(clips), 1)
        self.assertEqual(clips[0]["text"], LINE)
        self.assertEqual(clips[0]["image"], self.img)
        self.assertEqual((clips[0]["offset"], clips[0]["seconds"]), (0.0, 4.1))
        self.assertEqual((clips[0]["motion"], clips[0]["amount"], clips[0]["transition"]), ("shake", PUNCH, FLASH))
        self.assertEqual(clips[0]["layout"], "single")

    def test_cut_in_panels_are_not_used(self):
        shot = dict(self.shot, more=[{"image": self.img}, {"image": self.img}])
        self.assertEqual(len(edit_line(1, shot, 6.0, False)[0]), 1)

    def test_a_calm_proof_whips_in(self):
        clips, punched = edit_line(1, self.shot, 1.5, False)
        self.assertFalse(punched)
        self.assertIn(clips[0]["transition"], WHIPS)
        self.assertNotIn("motion", clips[0])
        # The statement has nothing to come in from.
        self.assertNotIn("transition", edit_line(0, self.shot, 1.5, True)[0][0])

    def test_the_outro_fades_through_black_onto_the_cover(self):
        clips, _ = edit_line(3, dict(self.shot, layout="single"), 3.0, False)
        self.assertEqual(len(clips), 1)
        self.assertEqual(clips[0]["transition"], TO_OUTRO)
        self.assertNotIn("motion", clips[0])


if __name__ == "__main__":
    unittest.main()
