import os
import sys
import tempfile
import unittest

# overlay_graph imports caption_engine the way the pipeline does (python/ on the path).
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "python"))

from python.video_generator import (  # noqa: E402
    apply_edit_clips, apply_intensity, look_filters, overlay_graph, plan_clip_fx, plan_transitions, resolve_look, xfade_chain,
)

BEATS = [
    {"page": 0, "text": "One.", "audio_file": "page_0000.wav", "start_ms": 0, "end_ms": 4000, "audio_offset_ms": 0, "duration_ms": 4000},
    {"page": 0, "text": "Two.", "audio_file": "page_0000.wav", "start_ms": 4000, "end_ms": 6000, "audio_offset_ms": 4000, "duration_ms": 2000},
    {"page": 1, "text": "Three.", "audio_file": "page_0001.wav", "start_ms": 6000, "end_ms": 9000},
]


class ApplyEditClipsTests(unittest.TestCase):
    def test_reorders_and_drops(self):
        out = apply_edit_clips(BEATS, [{"src": 2}, {"src": 0}])
        self.assertEqual([s["text"] for s in out], ["Three.", "One."])

    def test_trim_moves_the_audio_slice(self):
        out = apply_edit_clips(BEATS, [{"src": 1, "trim_in_ms": 500, "trim_out_ms": 300}])
        self.assertEqual(out[0]["audio_offset_ms"], 4500)
        self.assertEqual(out[0]["duration_ms"], 1200)
        # The source list is untouched.
        self.assertEqual(BEATS[1]["duration_ms"], 2000)

    def test_page_segments_get_a_duration(self):
        out = apply_edit_clips(BEATS, [{"src": 2, "trim_in_ms": 1000}])
        self.assertEqual(out[0]["duration_ms"], 2000)
        self.assertEqual(out[0]["audio_offset_ms"], 1000)

    def test_overrides_and_bad_entries(self):
        out = apply_edit_clips(BEATS, [
            {"src": 0, "text": "New words", "motion": "shake", "cut": True},
            {"src": 9}, {"src": "x"}, "junk",
            {"src": 1, "trim_in_ms": 1950},
            {"src": 1, "motion": "spin"},
        ])
        self.assertEqual(len(out), 2)
        self.assertEqual(out[0]["text"], "New words")
        self.assertEqual(out[0]["motion"], "shake")
        self.assertTrue(out[0]["cut"])
        self.assertNotIn("motion", out[1])


class EditParametersTests(unittest.TestCase):
    def test_moves_speed_and_transitions_reach_the_render_items(self):
        out = apply_edit_clips(BEATS, [
            {"src": 0, "motion": "ken_burns", "amount": 0.11, "focus_from": [30, 35], "focus_to": [70, 62], "speed": 1.25},
            {"src": 1, "motion": "pan_left", "amount": 9, "transition": {"kind": "glitch", "ms": 250}},
            {"src": 2, "cut": True, "transition": {"kind": "flash"}},
            {"src": 1, "transition": {"kind": "spin"}, "speed": 50},
        ])
        self.assertEqual(out[0]["motion"], "ken_burns")
        self.assertEqual(out[0]["amount"], 0.11)
        self.assertEqual(out[0]["focus_from"], (0.3, 0.35))
        self.assertEqual(out[0]["speed"], 1.25)
        self.assertEqual(out[0]["duration_ms"], 4000, "the narration slice stays whole; the renderer retimes it")
        self.assertEqual(out[1]["amount"], 0.5, "amounts clamp")
        self.assertEqual(out[1]["transition"], ("glitch", 0.25))
        self.assertTrue(out[2]["cut"])
        self.assertNotIn("transition", out[2])
        self.assertNotIn("transition", out[3])
        self.assertEqual(out[3]["speed"], 2.0)

    def test_the_editors_transition_wins_and_is_drawn(self):
        starts, trans = plan_transitions([2, 2, 2], [0, 0, 1], forced=[None, ("glitch", 0.3), ("whipleft", 0.2)])
        self.assertEqual(trans[1], ("glitch", 0.3))
        self.assertEqual(trans[2], ("whipleft", 0.2))
        fx = plan_clip_fx(starts, trans, [0, 0, 1], forced=[None, ("glitch", 0.3), ("whipleft", 0.2)])
        self.assertEqual(fx[0]["glitch_out"], (2.0, 0.3))
        self.assertEqual(fx[1]["glitch_in"], 0.3)
        self.assertIn("whipleft_out", fx[1])
        graph = xfade_chain(["a", "b", "c"], starts, trans, "v")
        self.assertIn("transition=pixelize", graph)
        self.assertIn("transition=slideleft", graph)
        # Same page, not picked by the editor: the usual quick fade.
        _, trans = plan_transitions([2, 2], [0, 0])
        self.assertEqual(trans[1], ("fade", 0.35))

    def test_intensity_scales_the_look(self):
        look = apply_intensity(resolve_look({"preset": "cinematic"}), {"motion": 2, "grain": 0.5, "vignette": 0})
        self.assertAlmostEqual(look["push"], 0.12)
        graph = look_filters(look, 1920, 1080, "a", "b")
        self.assertIn("noise=alls=2", graph)
        self.assertNotIn("vignette", graph)
        self.assertEqual(apply_intensity(resolve_look("clean"), None)["push"], 0.05)


class OverlayGraphTests(unittest.TestCase):
    def test_nothing_passes_the_picture_through(self):
        inputs, graph = overlay_graph([], 1920, 1080, 10.0, "vx", "vo", 3, ".")
        self.assertEqual(inputs, [])
        self.assertEqual(graph, "[vx]null[vo]")

    def test_image_and_text_layers(self):
        with tempfile.TemporaryDirectory() as d:
            logo = os.path.join(d, "logo.png")
            open(logo, "wb").close()
            overlays = [
                {"kind": "image", "path": logo, "x": 90, "y": 10, "scale": 0.1, "opacity": 0.5, "start": 0, "end": 5},
                {"kind": "text", "text": "Hello: world's", "x": 50, "y": 80, "size": 0.05, "color": "#ff0000", "box": True, "start": 2, "end": 99},
                {"kind": "image", "path": os.path.join(d, "missing.png")},
                {"kind": "text", "text": "  "},
            ]
            inputs, graph = overlay_graph(overlays, 1920, 1080, 10.0, "vx", "vo", 3, d)
            self.assertEqual(inputs, ["-loop", "1", "-t", "10.000", "-i", logo])
            self.assertIn("[3:v]scale=192:-2", graph)
            self.assertIn("aa=0.50", graph)
            self.assertIn("between(t,0.000,5.000)", graph)
            # Text is read from a file (no escaping), clamped to the video's end.
            self.assertIn("textfile=", graph)
            self.assertIn("between(t,2.000,10.000)", graph)
            self.assertIn("fontcolor=0xff0000@1.00", graph)
            self.assertIn("box=1", graph)
            with open(os.path.join(d, "overlay_1.txt"), encoding="utf-8") as f:
                self.assertEqual(f.read(), "Hello: world's")
            self.assertTrue(graph.endswith("[vo_o1]null[vo]"))


if __name__ == "__main__":
    unittest.main()
