"""
How scene emotion becomes Chatterbox settings (narration_tts.chatterbox_params) and how it is
smoothed across lines: a calm narrator by default, emotion only for real moments, never extreme.
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "python"))

import narration_tts as nt  # noqa: E402


class ChatterboxParamsTest(unittest.TestCase):
    def test_ordinary_lines_are_plain_narration(self):
        for kind in nt.KINDS:
            for i in (0, 2, 3, 4):
                p = nt.chatterbox_params(kind, i)
                self.assertLessEqual(p["exaggeration"], 0.5, (kind, i))
                self.assertEqual(p["cfg_weight"], 0.5, (kind, i))
                self.assertLessEqual(p["temperature"], 0.8, (kind, i))

    def test_emotion_rises_with_intensity_and_is_capped(self):
        prev = 0.0
        for i in range(0, 11):
            e = nt.chatterbox_params("fight", i)["exaggeration"]
            self.assertGreaterEqual(e, prev)
            prev = e
        for kind in nt.KINDS:
            p = nt.chatterbox_params(kind, 10)
            self.assertLessEqual(p["exaggeration"], 0.8, kind)
            self.assertGreaterEqual(p["cfg_weight"], 0.3, kind)
            self.assertLessEqual(p["temperature"], 0.85, kind)

    def test_calm_scenes_never_lift(self):
        for kind in ("calm", "flashback"):
            self.assertEqual(nt.chatterbox_params(kind, 2)["exaggeration"], nt.chatterbox_params(kind, 10)["exaggeration"])

    def test_more_exaggeration_lowers_cfg(self):
        quiet, loud = nt.chatterbox_params("action", 4), nt.chatterbox_params("action", 9)
        self.assertGreater(loud["exaggeration"], quiet["exaggeration"])
        self.assertLess(loud["cfg_weight"], quiet["cfg_weight"])

    def test_strength(self):
        self.assertEqual(nt.chatterbox_params("fight", 10, strength=0)["exaggeration"], 0.5)
        self.assertLessEqual(nt.chatterbox_params("fight", 10, strength=2)["exaggeration"], 0.95)
        self.assertGreater(nt.chatterbox_params("fight", 10, strength=2)["exaggeration"],
                           nt.chatterbox_params("fight", 10, strength=1)["exaggeration"])

    def test_thoughts_are_softer(self):
        self.assertLess(nt.chatterbox_params("emotional", 8, "thought")["exaggeration"],
                        nt.chatterbox_params("emotional", 8)["exaggeration"])


class SmoothTest(unittest.TestCase):
    def test_no_wild_swings(self):
        out = nt.smooth([2, 2, 10, 2, 2, 9, 9, 9, 1])
        for a, b in zip(out, out[1:]):
            self.assertLessEqual(abs(a - b), 2.0 + 1e-9)

    def test_lone_spike_softened_but_runs_kept(self):
        out = nt.smooth([3, 3, 9, 3, 3, 8, 8, 8, 8, 3])
        self.assertLess(out[2], 6.5)
        self.assertGreaterEqual(out[7], 7.5)

    def test_missing_values_take_the_average(self):
        self.assertEqual(nt.smooth([None, None]), [nt.DEFAULT_INTENSITY, nt.DEFAULT_INTENSITY])


if __name__ == "__main__":
    unittest.main()
