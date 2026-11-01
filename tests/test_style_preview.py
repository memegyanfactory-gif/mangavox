"""The dashboard's Video style preview (style_preview.py) and the renderer's framing choices it
previews (duo on/off, fill framing, camera motion amount)."""
import os
import unittest

from python.style_preview import pick_beats, preview_beats, preview_config
from python.video_generator import FG_MARGIN, FILL_MARGIN, apply_intensity, panel_margins, resolve_look


class PickTests(unittest.TestCase):
    def test_the_frame_comes_from_a_narrated_beat_past_the_opening(self):
        items = [{"text": ""}] * 3 + [{"text": "He wakes."}] + [{"text": "x"}] * 16
        self.assertEqual(pick_beats(items, 0.15, 2), [3, 4])
        # Near the end the pair stays inside the chapter.
        self.assertEqual(pick_beats([{"text": "a"}] * 3, 0.9, 2), [1, 2])
        self.assertEqual(pick_beats([{"text": "a"}], 0.15, 2), [0])
        self.assertEqual(pick_beats([], 0.15, 2), [])

    def test_beats_are_capped_and_play_from_the_chapter_audio(self):
        sync = {"beat_segments": [
            {"page": 0, "text": "a", "audio_file": "page_0000.wav", "duration_ms": 9000, "audio_offset_ms": 500, "motion": "pan_left", "cut": True},
            {"page": 1, "text": "b", "audio_file": "page_0001.wav", "duration_ms": 2000},
        ]}
        beats, total = preview_beats(sync, "out", [0, 1], max_seconds=6)
        self.assertEqual([b["duration_ms"] for b in beats], [6000, 2000])
        self.assertEqual([b["start_ms"] for b in beats], [0, 6000])
        self.assertEqual(total, 8000)
        self.assertEqual(beats[0]["audio_file"], os.path.join("out", "audio", "page_0000.wav"))
        self.assertEqual(beats[0]["audio_offset_ms"], 500)
        self.assertNotIn("motion", beats[0], "the editor's clip choices belong to its own clip list")
        self.assertNotIn("cut", beats[0])

    def test_the_preview_is_silent_and_leaves_out_what_is_timed_to_the_chapter(self):
        cfg = preview_config({
            "look": {"preset": "dynamic", "layered": "all", "framing": "fill"}, "score": "s.wav", "sfx": "punchy",
            "vfx_python": "py", "edit": {"clips": [1], "overlays": [2], "intensity": {"motion": 1.5}},
            "subtitles": {"style": "karaoke"},
        })
        self.assertEqual(cfg["look"], {"preset": "dynamic", "layered": "off", "framing": "fill"})
        self.assertEqual(cfg["sfx"], "off")
        self.assertNotIn("score", cfg)
        self.assertNotIn("vfx_python", cfg)
        self.assertEqual(cfg["edit"], {"intensity": {"motion": 1.5}})
        self.assertEqual(cfg["subtitles"], {"style": "karaoke"})
        self.assertTrue(cfg["force_rerender"])


class FramingChoiceTests(unittest.TestCase):
    def test_absent_choices_keep_the_look_as_it_was(self):
        look = resolve_look({"preset": "dynamic"})
        self.assertTrue(look["duo"])
        self.assertEqual(look["framing"], "fit")
        self.assertEqual(apply_intensity(look, None)["push"], 0.08)

    def test_side_by_side_and_motion_follow_the_studio_choice(self):
        self.assertFalse(resolve_look({"preset": "dynamic", "duo": False})["duo"])
        self.assertTrue(resolve_look({"preset": "clean", "duo": True})["duo"])
        calm = apply_intensity(resolve_look({"preset": "dynamic", "motion": 0.5}), None)
        self.assertEqual((calm["motion_k"], calm["push"]), (0.5, 0.04))
        # The editor's own motion strength scales on top of the Studio's.
        both = apply_intensity(resolve_look({"preset": "dynamic", "motion": 0.5}), {"motion": 2.0})
        self.assertEqual(both["motion_k"], 1.0)
        self.assertEqual(resolve_look({"motion": "junk"})["motion_amount"], 1.0)

    def test_fill_draws_the_panel_larger_with_a_thinner_margin(self):
        self.assertEqual(panel_margins(1920, 1080, "blur"), (round(1920 * FG_MARGIN), round(1080 * FG_MARGIN)))
        mx, my = panel_margins(1920, 1080, "blur", 0, FILL_MARGIN)
        self.assertLess(mx, round(1920 * FG_MARGIN))
        self.assertGreater(mx, 0, "the panel never touches the edge: it stays whole")
        self.assertEqual(panel_margins(1920, 1080, "black", 0, FILL_MARGIN), (0, 0))


if __name__ == "__main__":
    unittest.main()
