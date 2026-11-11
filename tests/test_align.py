"""Word timing from the narration (align.py): the path through the model's output, how a
page's words become panel cuts and caption timings, and everything carrying on without the
model. No network, no model, no GPU."""
import json
import os
import tempfile
import unittest
import wave
from unittest import mock

import numpy as np

from python import align
from python import caption_engine as ce
from python import sync_map as sm

VOCAB = {"<pad>": 0, "<s>": 1, "</s>": 2, "<unk>": 3, "|": 4, "E": 5, "T": 6, "A": 7, "O": 8, "N": 9, "I": 10, "H": 11, "S": 12,
         "R": 13, "D": 14, "L": 15, "U": 16, "M": 17, "W": 18, "C": 19, "F": 20, "G": 21, "Y": 22, "P": 23, "B": 24, "V": 25,
         "K": 26, "'": 27, "X": 28, "J": 29, "Q": 30, "Z": 31}


def emissions(frames, said):
    """Log-probabilities of `frames` frames where `said` {frame: token id} are spoken and
    every other frame is blank."""
    em = np.full((frames, len(VOCAB)), -12.0)
    em[:, 0] = -0.01
    for f, k in said.items():
        em[f] = -12.0
        em[f, k] = -0.01
    return em


def wav(path, seconds=1.0, rate=24000):
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"\x00\x00" * int(seconds * rate))
    return path


class TextTests(unittest.TestCase):
    def test_words_are_split_as_the_captions_split_them(self):
        for text in ("He is in luck, a townsman tells him.", "Wait — what?! No...", "  ", "It's 41 years... later"):
            self.assertEqual(align.tokenize(text), ce.tokenize(text))

    def test_only_what_the_model_hears_is_laid_on_the_audio(self):
        self.assertEqual(align.letters("luck,"), "LUCK")
        self.assertEqual(align.letters("don’t"), "DON'T")
        self.assertEqual(align.letters("41"), "")
        self.assertTrue(align.english("He is in luck."))
        self.assertFalse(align.english("그는 운이 좋다"))
        self.assertFalse(align.english("41 ... !"))


class PathTests(unittest.TestCase):
    def test_tokens_land_on_the_frames_that_say_them(self):
        em = emissions(40, {5: VOCAB["H"], 9: VOCAB["I"], 20: VOCAB["|"], 24: VOCAB["N"], 30: VOCAB["O"]})
        frames, score = align.forced(em, [VOCAB[c] for c in "HI|NO"])
        self.assertEqual(frames, [5, 9, 20, 24, 30])
        self.assertGreater(score, -0.1)

    def test_audio_too_short_for_the_text_is_not_aligned(self):
        self.assertIsNone(align.forced(emissions(3, {}), [5, 6, 7, 8]))
        self.assertIsNone(align.forced(emissions(10, {}), []))

    def test_a_page_gets_a_time_for_every_word_even_one_with_no_letters(self):
        em = emissions(100, {10: VOCAB["H"], 12: VOCAB["E"], 30: VOCAB["|"], 60: VOCAB["O"], 62: VOCAB["N"]})
        words = align.time_page(em, "He 41 on", VOCAB, 2000)
        self.assertEqual([w[0] for w in words], ["He", "41", "on"])
        self.assertEqual(words[0][1], 200)
        self.assertEqual(words[2][1], 1200)
        self.assertTrue(words[0][2] <= words[1][1] <= words[1][2] <= words[2][1], words)
        self.assertLessEqual(words[2][2], 2000)

    def test_a_page_the_model_cannot_follow_keeps_the_estimate(self):
        # Every frame is blank: the text is not in this audio.
        self.assertIsNone(align.time_page(emissions(100, {}), "He is in luck", VOCAB, 2000))
        self.assertIsNone(align.time_page(emissions(100, {}), "41 ...", VOCAB, 2000))


class CutTests(unittest.TestCase):
    WORDS = [["He", 200, 300], ["waits.", 340, 800], ["Then", 2000, 2200], ["runs.", 2240, 2700]]

    def test_a_panel_comes_in_just_before_the_word_that_starts_it(self):
        self.assertEqual(align.beat_cuts(self.WORDS, [2, 2]), [2000 - align.CUT_LEAD_MS])
        # No pause between the words: the cut waits for the word before to end.
        tight = [["He", 200, 300], ["waits", 340, 800], ["then", 810, 1000], ["runs.", 1040, 1500]]
        self.assertEqual(align.beat_cuts(tight, [2, 2]), [800])

    def test_words_that_do_not_add_up_or_a_flash_of_a_panel_keep_the_estimate(self):
        self.assertIsNone(align.beat_cuts(self.WORDS, [2, 3]))
        self.assertIsNone(align.beat_cuts(self.WORDS, [4, 0]))
        self.assertIsNone(align.beat_cuts([["A", 0, 50], ["b", 60, 120], ["c", 900, 1000]], [1, 2]), "the first panel would show 60 ms")
        self.assertIsNone(align.beat_cuts(None, [1]))


class SyncMapTests(unittest.TestCase):
    def build(self, words):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "audio"))
            path = wav(os.path.join(d, "audio", "page_0000.wav"), 3.0)
            beats = [{"page": 0, "panel": "p0000_00.jpg", "panel_image": "a.jpg", "text": "He waits."},
                     {"page": 0, "panel": "p0000_01.jpg", "panel_image": "b.jpg", "text": "Then runs."}]
            # sync_map imports the module by its bare name (the worker runs it as a script).
            import align as bare
            with mock.patch.object(bare, "align_pages", return_value={path: words} if words else {}):
                return sm.generate_sync_map([{"page": 0, "text": "He waits. Then runs."}], [{"page": 0, "path": path, "text": "He waits. Then runs."}], d, beats=beats)

    def test_panels_change_on_the_word_and_carry_their_words(self):
        got = self.build(CutTests.WORDS)["beat_segments"]
        self.assertEqual([(b["start_ms"], b["end_ms"]) for b in got], [(0, 1920), (1920, 3000)])
        self.assertEqual(got[1]["audio_offset_ms"], 1920)
        self.assertEqual([w[0] for w in got[0]["words"]], ["He", "waits."])
        self.assertEqual(got[1]["words"], CutTests.WORDS[2:])

    def test_without_word_timing_panels_are_shared_out_by_text_length(self):
        got = self.build(None)["beat_segments"]
        self.assertEqual([(b["start_ms"], b["end_ms"]) for b in got], [(0, 1421), (1421, 3000)])
        self.assertNotIn("words", got[0])


class CaptionTests(unittest.TestCase):
    def test_captions_follow_the_aligned_words_on_the_clip(self):
        # The beat plays from 1920 ms of its page audio, on the video from 10 s, at 1x.
        seg = {"words": CutTests.WORDS[2:], "word_offset_ms": 1920}
        got = ce.aligned_words(seg, "Then runs.", 10.0, 11.58)
        self.assertEqual([(w["text"], w["start"]) for w in got], [("Then", 10.08), ("runs.", 10.32)])
        # Twice as fast: half the distance into the clip.
        fast = ce.aligned_words(dict(seg, speed=2.0), "Then runs.", 10.0, 10.79)
        self.assertEqual([w["start"] for w in fast], [10.04, 10.16])

    def test_a_caption_the_editor_rewrote_falls_back_to_the_estimate(self):
        seg = {"words": CutTests.WORDS[2:], "word_offset_ms": 1920}
        self.assertIsNone(ce.aligned_words(seg, "Then he runs.", 10.0, 11.58))
        self.assertIsNone(ce.aligned_words({}, "Then runs.", 10.0, 11.58))
        cues = ce.build_cues([dict(seg, text="Then he runs.", start_ms=10000, end_ms=11580)], {"wordsPerLine": 4})
        self.assertEqual([w["text"] for c in cues for w in c["words"]], ["Then", "he", "runs."])

    def test_cues_use_the_aligned_times(self):
        seg = {"text": "Then runs.", "start_ms": 10000, "end_ms": 11580, "words": CutTests.WORDS[2:], "word_offset_ms": 1920}
        cues = ce.build_cues([seg], {"wordsPerLine": 4})
        self.assertEqual([round(w["start"], 2) for w in cues[0]["words"]], [10.08, 10.32], "each word waits for the voice")


class StoreTests(unittest.TestCase):
    def test_without_the_pack_nothing_is_aligned_and_nothing_is_recorded(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "audio"))
            path = wav(os.path.join(d, "audio", "page_0000.wav"))
            with mock.patch.object(align.lp, "pack_python", return_value=(None, "not installed")):
                self.assertEqual(align.align_pages([{"audio": path, "text": "He waits."}], d), {})
            self.assertFalse(os.path.exists(align.store(d)), "the next run tries again")

    def test_a_page_is_aligned_once_and_read_back(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "audio"))
            path = wav(os.path.join(d, "audio", "page_0000.wav"))
            other = wav(os.path.join(d, "audio", "page_0001.wav"))
            pages = [{"audio": path, "text": "He waits."}, {"audio": other, "text": "그는 기다린다"}]
            words = [["He", 100, 200], ["waits.", 240, 700]]
            with mock.patch.object(align.lp, "pack_python", return_value=("py", None)), \
                    mock.patch.object(align, "_run", return_value={path: words}) as run:
                self.assertEqual(align.align_pages(pages, d), {path: words})
                self.assertEqual([j["audio"] for j in run.call_args[0][1]], [path], "the Korean page is never sent")
            with mock.patch.object(align, "_run", side_effect=AssertionError("aligned again")):
                self.assertEqual(align.align_pages(pages, d), {path: words})
            # Another text for the same audio is another alignment.
            with mock.patch.object(align.lp, "pack_python", return_value=("py", None)), \
                    mock.patch.object(align, "_run", return_value={path: None}) as run:
                self.assertEqual(align.align_pages([{"audio": path, "text": "He waited."}], d), {})
                self.assertTrue(run.called)
            with open(align.store(d), encoding="utf-8") as f:
                self.assertIsNone(json.load(f)["page_0000.wav"]["words"])

    def test_it_can_be_turned_off(self):
        with mock.patch.dict(os.environ, {"MANGAVOX_ALIGN": "0"}):
            self.assertEqual(align.align_pages([{"audio": "x.wav", "text": "He waits."}], "nowhere"), {})
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MANGAVOX_ALIGN", None)
            self.assertFalse(align.wanted({"align_words": False}))
            self.assertTrue(align.wanted(None))


if __name__ == "__main__":
    unittest.main()
