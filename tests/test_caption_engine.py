import json
import os
import struct
import tempfile
import unittest
import wave

from python import caption_engine as ce


def _tone_wav(path, pattern, sr=16000):
    """pattern: [(seconds, voiced?)] -> a WAV with noise bursts for voiced spans."""
    import random
    frames = bytearray()
    for secs, voiced in pattern:
        for _ in range(int(secs * sr)):
            v = int(random.uniform(-0.5, 0.5) * 32767) if voiced else 0
            frames += struct.pack("<h", v)
    with wave.open(path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sr)
        wf.writeframes(bytes(frames))


class CatalogTests(unittest.TestCase):
    def test_catalog_has_many_watchfiwn_templates(self):
        ids = ce.style_ids()
        self.assertGreaterEqual(len(ids), 25)
        for sid in ("hormozi", "karaoke", "kalakar", "popup", "bubble", "neon", "dynHypeDrop", "cinemawide"):
            self.assertIn(sid, ids)
        fonts = ce.catalog()["fonts"]
        for s in ce.catalog()["styles"]:
            self.assertIn(s["font"], fonts, s["id"])
            self.assertTrue(os.path.isfile(os.path.join(ce.FONTS_DIR, fonts[s["font"]]["file"])), s["id"])

    def test_colours(self):
        self.assertEqual(ce.parse_color("#ff0000"), (255, 0, 0, 1.0))
        self.assertEqual(ce.parse_color("rgba(0,0,0,0.5)")[3], 0.5)
        self.assertEqual(ce.ass_bgr((255, 128, 0, 1)), "&H0080FF&")
        self.assertEqual(ce.ass_alpha(1.0), "&H00&")
        self.assertEqual(ce.ass_alpha(0.0), "&HFF&")


class SettingsTests(unittest.TestCase):
    def test_precedence_config_then_chapter_then_studio_default(self):
        with tempfile.TemporaryDirectory() as root:
            studio = os.path.join(root, "studio")
            chapter = os.path.join(studio, "projects", "m", "c")
            os.makedirs(chapter)
            with open(os.path.join(studio, "subtitles.json"), "w") as f:
                json.dump({"default": {"style": "neon", "size": 1.5}}, f)
            self.assertEqual(ce.resolve_settings({}, chapter)["style"], "neon")
            with open(os.path.join(chapter, "subtitles.json"), "w") as f:
                json.dump({"style": "bubble"}, f)
            self.assertEqual(ce.resolve_settings({}, chapter)["style"], "bubble")
            self.assertEqual(ce.resolve_settings({"subtitles": {"style": "hormozi"}}, chapter)["style"], "hormozi")
            self.assertFalse(ce.resolve_settings({"captions": False}, chapter)["enabled"])

    def test_overrides(self):
        s = ce.normalize_settings({"style": "hormozi", "size": 2, "posX": 10, "posY": 20, "uppercase": False, "emphasis": False})
        st = ce.apply_settings(ce.get_style("hormozi"), s)
        self.assertAlmostEqual(st["size"], ce.get_style("hormozi")["size"] * 2)
        self.assertEqual((st["posX"], st["posY"]), (10, 20))
        self.assertFalse(st["uppercase"])
        self.assertFalse(st["highlightWords"])
        self.assertEqual(ce.normalize_settings({"size": 99})["size"], 2.5)


class TimingTests(unittest.TestCase):
    def test_words_cover_the_segment_in_order(self):
        words = ce.time_words("One two three, four five six.", 10.0, 13.0)
        self.assertEqual([w["text"] for w in words], ["One", "two", "three,", "four", "five", "six."])
        self.assertGreaterEqual(words[0]["start"], 10.0)
        self.assertLessEqual(words[-1]["end"], 13.0 + 1e-6)
        for a, b in zip(words, words[1:]):
            self.assertLessEqual(a["end"], b["start"] + 1e-6)

    def test_comma_snaps_to_the_real_pause(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "a.wav")
            # Voice 0.1-1.0 s, pause until 2.0 s, voice 2.0-2.6 s.
            _tone_wav(p, [(0.1, False), (0.9, True), (1.0, False), (0.6, True), (0.1, False)])
            words = ce.time_words("Alpha beta gamma delta, epsilon zeta.", 0.0, 2.7, p, 0.0)
            epsilon = next(w for w in words if w["text"] == "epsilon")
            self.assertAlmostEqual(epsilon["start"], 2.0, delta=0.08)
            delta = next(w for w in words if w["text"] == "delta,")
            self.assertAlmostEqual(delta["end"], 1.0, delta=0.08)

    def test_cues_respect_sentences_and_size(self):
        st = ce.apply_settings(ce.get_style("hormozi"), ce.normalize_settings({"wordsPerLine": 3, "lines": 1}))
        cues = ce.build_cues([{"text": "One two three four five six seven. Eight nine.", "start_ms": 0, "end_ms": 6000}], st)
        for c in cues:
            self.assertLessEqual(len(c["words"]), 4)
        self.assertEqual(cues[-1]["words"][-1]["text"], "nine.")
        self.assertTrue(any(c["words"][-1]["text"] == "seven." for c in cues))


class AssTests(unittest.TestCase):
    def test_every_style_writes_a_valid_script(self):
        segs = [{"text": "The hero steps forward, and the whole city holds its breath.", "start_ms": 0, "end_ms": 4200},
                {"text": "Nobody saw it coming!", "start_ms": 4200, "end_ms": 6000}]
        with tempfile.TemporaryDirectory() as d:
            for sid in ce.style_ids():
                for w, h in ((1280, 720), (720, 1280)):
                    path = ce.write_ass(segs, os.path.join(d, "c.ass"), w, h, {"style": sid})
                    self.assertIsNotNone(path, sid)
                    with open(path, encoding="utf-8") as fh:
                        text = fh.read()
                    self.assertIn(f"PlayResX: {w}", text)
                    events = [l for l in text.splitlines() if l.startswith("Dialogue:")]
                    self.assertTrue(events, sid)
                    self.assertTrue(all(l.count("{") >= 1 for l in events), sid)

    def test_captions_off_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(ce.write_ass([{"text": "Hi there.", "start_ms": 0, "end_ms": 900}], os.path.join(d, "c.ass"), 640, 360, {"enabled": False}))

    def test_position_follows_the_setting(self):
        with tempfile.TemporaryDirectory() as d:
            p = ce.write_ass([{"text": "Top caption", "start_ms": 0, "end_ms": 2000}], os.path.join(d, "c.ass"), 1000, 1000, {"style": "clean", "posX": 50, "posY": 20})
            ys = [float(l.split("\\pos(")[1].split(",")[1].split(")")[0]) for l in open(p, encoding="utf-8") if "\\pos(" in l]
            self.assertTrue(ys and all(150 < y < 260 for y in ys), ys)

    def test_filter_path_escapes_drive_colon(self):
        f = ce.subtitles_filter(r"C:\x\captions.ass")
        self.assertIn("C\\:/x/captions.ass", f)
        self.assertIn("fontsdir=", f)


if __name__ == "__main__":
    unittest.main()
