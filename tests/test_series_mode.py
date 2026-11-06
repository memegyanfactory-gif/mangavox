"""
Series mode (beta): the narrator stays smooth while each character acts their line in their own
cloned voice, a change of speaker gets a breath of silence, who speaks travels from the Studio
beats through the sync map to the video, and a character's first line brings up their name card.
"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import name_cards  # noqa: E402
import narration_tts as nt  # noqa: E402
import sync_map  # noqa: E402
import tts_bridge  # noqa: E402


class CastDelivery(unittest.TestCase):
    def test_the_narrator_stays_smooth_at_every_peak(self):
        for kind in ("calm", "fight", "horror", "emotional"):
            for i in (2, 5, 8, 10):
                p = nt.chatterbox_params(kind, i, "narrator", 2.0)
                self.assertLessEqual(p["exaggeration"], 0.55, (kind, i))
                self.assertEqual(p["temperature"], 0.7)

    def test_a_character_acts_their_line(self):
        cry = nt.chatterbox_params("fight", 8, "speech", 1.0)
        calm = nt.chatterbox_params("calm", 3, "speech", 1.0)
        narrator = nt.chatterbox_params("fight", 8, "narrator", 1.0)
        self.assertGreater(cry["exaggeration"], calm["exaggeration"])
        self.assertGreater(cry["exaggeration"], narrator["exaggeration"])
        # More exaggeration speeds Chatterbox up: a lower cfg keeps the pace.
        self.assertLess(cry["cfg_weight"], calm["cfg_weight"] + 1e-9)
        thought = nt.chatterbox_params("fight", 8, "inner", 1.0)
        self.assertLess(thought["exaggeration"], cry["exaggeration"])

    def test_a_new_speaker_gets_a_breath_and_keeps_their_voice(self):
        units = [
            {"text": "He walks in.", "kind": "calm", "intensity": 3, "line_kind": "narrator"},
            {"text": "I've waited a year.", "kind": "fight", "intensity": 7, "line_kind": "speech",
             "speaker": "Arhan", "voice_ref": "a.wav", "voice_make_with": "am_adam", "voice_pitch": -2.0},
            {"text": "Then you waited to die.", "kind": "reveal", "intensity": 6, "line_kind": "speech",
             "speaker": "Karlos", "voice_ref": "k.wav", "voice_make_with": "am_onyx", "voice_pitch": 0.0},
        ]
        chunks = nt.plan_chunks(units)
        self.assertEqual([c.get("speaker") for c in chunks], [None, "Arhan", "Karlos"])
        self.assertEqual(chunks[1]["voice_ref"], "a.wav")
        same = nt.plan_chunks([dict(units[1], text="One."), dict(units[1], text="Two.")])
        self.assertGreaterEqual(chunks[1]["pause_before_ms"], same[1]["pause_before_ms"] + nt.SPEAKER_GAP_MS - 1)

    def test_a_characters_chunk_is_voiced_from_their_clip(self):
        class Cb:
            CHATTERBOX_VERSION = "test"
        with tempfile.TemporaryDirectory() as d:
            clip = os.path.join(d, "arhan.wav")
            with open(clip, "wb") as f:
                f.write(b"\0" * 64)
            units = [
                {"text": "He walks in.", "kind": "calm", "intensity": 3, "line_kind": "narrator"},
                {"text": "I've waited a year.", "kind": "fight", "intensity": 7, "line_kind": "speech",
                 "speaker": "Arhan", "voice_ref": clip},
            ]
            out = tts_bridge._chatterbox_chunks(nt, Cb, 0, units, "narrator_voice", 1.0, "narrator-sig")
            self.assertNotIn("reference", out[0])
            self.assertEqual(out[1]["reference"], os.path.abspath(clip))
            self.assertEqual(out[1]["speaker"], "Arhan")
            # A missing clip falls back to the narrator's voice instead of failing.
            units[1]["voice_ref"] = os.path.join(d, "missing.wav")
            self.assertNotIn("reference", tts_bridge._chatterbox_chunks(nt, Cb, 0, units, "narrator_voice", 1.0, "narrator-sig")[1])


class WhoSpeaksReachesTheVideo(unittest.TestCase):
    def test_the_sync_map_keeps_the_speaker_and_name_card(self):
        segments = [{"page": 0, "start_ms": 0, "end_ms": 4000, "text": "He walks in. I've waited a year.", "audio_file": "page_0000.wav"}]
        beats = [
            {"page": 0, "panel": 0, "panel_image": "a.jpg", "text": "He walks in.", "line_kind": "narrator"},
            {"page": 0, "panel": 1, "panel_image": "b.jpg", "text": "I've waited a year.", "line_kind": "speech",
             "speaker": "Arhan", "name_card": "Arhan"},
        ]
        out = sync_map._build_beat_segments(segments, beats, 0)
        self.assertEqual(out[0].get("line_kind"), "narrator")
        self.assertNotIn("name_card", out[0])
        self.assertEqual((out[1]["speaker"], out[1]["name_card"]), ("Arhan", "Arhan"))


class NameCards(unittest.TestCase):
    def test_one_card_per_character_on_their_first_line(self):
        segs = [
            {"start_ms": 0, "end_ms": 2000, "text": "Narration."},
            {"start_ms": 2000, "end_ms": 3000, "name_card": "Arhan"},
            {"start_ms": 3000, "end_ms": 9000, "name_card": "Karlos"},
            {"start_ms": 9000, "end_ms": 9500, "name_card": "Arhan"},
        ]
        lines = name_cards.card_events(segs, 1920, 1080)
        names = [l for l in lines if "\\p1" not in l]
        self.assertEqual(len(names), 2)
        self.assertTrue(names[0].endswith("ARHAN") and names[1].endswith("KARLOS"))
        # Arhan's card ends where Karlos's begins (never two at once), Karlos's after 3 s.
        self.assertIn("0:00:02.00,0:00:03.00", names[0])
        self.assertIn("0:00:03.00,0:00:06.00", names[1])
        self.assertTrue(all(l.startswith("Dialogue: 7,") and ",W,," in l for l in lines))

    def test_cards_join_the_captions_or_make_their_own_track(self):
        segs = [{"start_ms": 1000, "end_ms": 4000, "name_card": "Mira {the} \\Queen"}]
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(name_cards.add_cards(None, [{"start_ms": 0, "end_ms": 1000}], 1920, 1080, d))
            own = name_cards.add_cards(None, segs, 1920, 1080, d)
            self.assertEqual(own, os.path.join(d, "namecards.ass"))
            with open(own, encoding="utf-8") as f:
                text = f.read()
            self.assertIn("PlayResX: 1920", text)
            self.assertIn("MIRA (THE) \\\\QUEEN", text)
            caps = os.path.join(d, "captions.ass")
            with open(caps, "w", encoding="utf-8") as f:
                f.write(name_cards.HEADER.format(w=1920, h=1080) + "Dialogue: 0,0:00:00.00,0:00:01.00,W,,0,0,0,,Hi\n")
            self.assertEqual(name_cards.add_cards(caps, segs, 1920, 1080, d), caps)
            with open(caps, encoding="utf-8") as f:
                lines = f.read().splitlines()
            self.assertEqual(lines[-3], "Dialogue: 0,0:00:00.00,0:00:01.00,W,,0,0,0,,Hi")
            self.assertEqual(len([l for l in lines if l.startswith("Dialogue: 7,")]), 2)


if __name__ == "__main__":
    unittest.main()
