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


class PacingTest(unittest.TestCase):
    """Read like a person: pauses that differ, sentences that flow together, no digital black."""

    def test_sentence_gaps_stay_in_range_and_differ_with_the_words(self):
        gaps = set()
        for kind in nt.KINDS:
            for i in range(0, 11):
                self.assertTrue(260 <= nt.sentence_gap_ms(kind, i) <= 620, (kind, i))
        for words in ("He ran.", "The gate opened slowly.", "Nobody spoke.", "It was already too late for that."):
            g = nt.sentence_gap_ms("other", 3, words)
            self.assertTrue(260 <= g <= 620)
            self.assertEqual(g, nt.sentence_gap_ms("other", 3, words))  # the same every render
            gaps.add(g)
        self.assertGreater(len(gaps), 2)
        self.assertLess(nt.sentence_gap_ms("fight", 8), nt.sentence_gap_ms("emotional", 3))

    def test_two_sentences_that_fit_are_voiced_together(self):
        a = "The guild had laughed when he signed up for the raid."
        b = "They stopped laughing when he walked out of the dungeon alone."
        self.assertEqual([c["text"] for c in nt.split_chunks(f"{a} {b}")], [f"{a} {b}"])
        long = "The army marched north through the frozen pass for nine days without any rest at all, and nobody complained."
        chunks = nt.split_chunks(f"{long} {long}")
        self.assertEqual(len(chunks), 2)
        for c in nt.split_chunks(" ".join([a, b] * 4)):
            self.assertLessEqual(len(c["text"]), 250)
        # A shout still stands alone.
        self.assertEqual([c["text"] for c in nt.split_chunks(f"{a} STOP RIGHT THERE! {b}")], [a, "STOP RIGHT THERE!", b])

    @unittest.skipIf(nt.np is None, "needs numpy")
    def test_a_page_has_room_tone_and_no_steps(self):
        np, sr = nt.np, 24000
        t = np.arange(sr) / sr
        word = (0.5 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
        page = nt.assemble_page([(word, 0), (word, 400)], sr, lead_ms=120, tail_ms=300)
        self.assertLess(float(np.mean(np.abs(page) < 1e-6)), 0.01)  # no stretch of exact zeros
        lead = page[: int(sr * 0.1)]
        level = 20 * np.log10(float(np.sqrt(np.mean(lead ** 2))))
        self.assertTrue(-80 < level < -68, level)
        self.assertLess(float(np.max(np.abs(np.diff(page)))), 0.05)  # 220 Hz at 0.5 steps 0.029 at most
        self.assertEqual(nt.room_tone(1000, sr).tolist(), nt.room_tone(1000, sr).tolist())


if __name__ == "__main__":
    unittest.main()
