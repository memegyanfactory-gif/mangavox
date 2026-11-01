"""The story-driven edit (mood plan, transitions, atmosphere, edge glow) and the framing rule
that nothing narrated is ever cut."""
import json
import os
import tempfile
import unittest

from python.video_generator import (
    ACCENT_GAP, EDGE_COLORS, IMPACT_GAP, PANEL_TRIM,
    apply_edit_clips, crop_region, crop_to_region, duo_geometry, edge_segments, focus_region, frame_clip,
    framing_worst, load_panels, load_scenes, look_filters, mood_transition, particle_kind, plan_clip_fx,
    plan_impacts, plan_moods, plan_transitions, punch_ok, resolve_look, scene_mood, smooth_moods,
)


def scene(kind, intensity, mood="", tags=()):
    return {"kind": kind, "mood": mood, "intensity": intensity, "tags": list(tags)}


class MoodPlanTests(unittest.TestCase):
    def test_scene_moods(self):
        self.assertEqual(scene_mood(scene("fight", 8)), "action")
        self.assertEqual(scene_mood(scene("horror", 5)), "horror")
        self.assertEqual(scene_mood(scene("emotional", 4, "heartfelt, warm")), "romance")
        self.assertEqual(scene_mood(scene("emotional", 4, "melancholy")), "sad")
        self.assertEqual(scene_mood(scene("dialogue", 4, "ominous")), "horror")
        self.assertEqual(scene_mood(scene("dialogue", 4, "playful")), "comedy")
        self.assertEqual(scene_mood(scene("other", 3, "neutral")), "neutral")
        self.assertEqual(scene_mood(None), "neutral")

    def test_smoothing_drops_a_lone_beat_but_keeps_a_run_of_two(self):
        self.assertEqual(smooth_moods(list("AAABAAA")), list("AAAAAAA"))
        self.assertEqual(smooth_moods(list("AAABBAAA"))[3:5], ["B", "B"])
        self.assertEqual(smooth_moods(list("AABBBB")), list("AABBBB"))

    def test_plan_reads_each_beats_page_and_smooths(self):
        scenes = {0: scene("calm", 2), 1: scene("fight", 9, tags=["explosion"]), 2: scene("calm", 2), 3: scene("calm", 2)}
        items = [{"page": p, "text": t} for p, t in ((0, "Quiet."), (0, "Still."), (1, "He strikes!"), (2, "Calm."), (3, "Calm."), (9, "No page."))]
        plan = plan_moods(items, scenes)
        self.assertEqual(len(plan), len(items))
        # The one fight beat between calm ones does not flip the grade...
        self.assertEqual([m["mood"] for m in plan], ["calm"] * 6)
        # ...but it keeps its own page's intensity for the punch.
        self.assertTrue(plan[2]["punch"])
        self.assertFalse(plan[0]["punch"])
        # A page the reading lacks borrows the nearest beat's.
        self.assertEqual(plan[5]["raw"], 2)
        # No reading: no plan.
        self.assertEqual(plan_moods(items, {}), [None] * len(items))
        self.assertEqual(plan_moods(items, None), [None] * len(items))

    def test_punch_needs_an_intense_page_and_a_hit(self):
        scenes = {0: scene("dialogue", 6), 1: scene("dialogue", 7), 2: scene("fight", 8), 3: scene("dialogue", 9, tags=["ambush"])}
        items = [{"page": 0, "text": "He attacks!"}, {"page": 1, "text": "He attacks!"}, {"page": 1, "text": "They talk."},
                 {"page": 2, "text": "They circle."}, {"page": 3, "text": "Silence."}]
        plan = plan_moods(items, scenes)
        # Intensity 6 with a loud line: no. 7 with one: yes; 7 without: no. A fight at 8 or a hit tag: yes.
        self.assertEqual([m["punch"] for m in plan], [False, True, False, True, True])

    def test_punches_are_spaced_in_time_and_denser_early(self):
        # Every clip wants a punch; 4-second clips.
        starts = [4.0 * k for k in range(20)]
        hits = plan_impacts(starts, [True] * 20)
        times = [t for t, h in zip(starts, hits) if h]
        self.assertEqual(times[0], 0.0)
        early = [t for t in times if t < 30]
        late = [t for t in times if t >= 30]
        self.assertTrue(all(b - a >= 10 for a, b in zip(early, early[1:])))
        self.assertGreaterEqual(len(early), 3, "the first 30 s may punch every 10 s")
        for a, b in zip(times, times[1:]):
            if b >= 30:
                self.assertGreaterEqual(b - a, 20)
        self.assertTrue(late)
        self.assertTrue(punch_ok(12.0, IMPACT_GAP - 1, 0.0, 0))
        self.assertFalse(punch_ok(40.0, 10, 25.0, 4), "15 s after the last one is too soon after the first 30 s")

    def test_editor_overrides_win(self):
        starts = [0.0, 3.0, 6.0, 9.0]
        # The editor's shake punches although it is too soon; its other move stops a wanted one.
        hits = plan_impacts(starts, [True, False, True, True], overrides=[None, True, False, None])
        self.assertEqual(hits, [True, True, False, False])
        # The editor's transition beats the mood's.
        moods = [{"mood": "action", "raw": 9}] * 3
        _, trans = plan_transitions([4, 4, 4], [0, 1, 2], forced=[None, ("fade", 0.5), None], moods=moods)
        self.assertEqual(trans[1], ("fade", 0.5))
        self.assertIn(trans[2][0], ("whipleft", "zoomblur", "whipright", "whipup", "glitch"))
        # The editor's per-clip motion reaches the render item (and wins in generate_video).
        self.assertEqual(apply_edit_clips([{"page": 0, "duration_ms": 3000}], [{"src": 0, "motion": "still"}])[0]["motion"], "still")


class TransitionTests(unittest.TestCase):
    def test_transitions_follow_the_mood(self):
        self.assertIn(mood_transition("sad", 3, 0, 0.5)[0], ("dissolve", "fade", "smoothleft"))
        self.assertGreater(mood_transition("sad", 3, 0, 0.5)[1], 0.5, "calm moods dissolve slowly")
        self.assertEqual(mood_transition("horror", 5, 0, 0.5)[0], "fadeblack")
        name, dur = mood_transition("action", 6, 0, 0.5)
        self.assertIn(name, ("whipleft", "zoomblur", "whipright", "whipup"))
        self.assertLess(dur, 0.5)
        self.assertIn(mood_transition("reveal", 5, 0, 0.5, accent=True)[0], ("fadewhite", "burn"))
        self.assertEqual(mood_transition("action", 9, 0, 0.5, accent=True)[0], "glitch")
        self.assertNotEqual(mood_transition("action", 6, 0, 0.5, accent=True)[0], "glitch", "only a peak gets the accent")

    def test_accents_are_rare(self):
        n = 30
        moods = [{"mood": "action", "raw": 9}] * n
        starts, trans = plan_transitions([3.0] * n, list(range(n)), moods=moods)
        accents = [starts[k] for k in range(1, n) if trans[k][0] in ("glitch", "burn", "fadewhite", "circleopen")]
        self.assertTrue(accents)
        self.assertTrue(all(b - a >= ACCENT_GAP for a, b in zip(accents, accents[1:])))

    def test_mood_whips_are_drawn_by_both_clips_and_quiet_scenes_skip_leaks(self):
        moods = [{"mood": "calm", "raw": 2}, {"mood": "action", "raw": 6}, {"mood": "horror", "raw": 5}, {"mood": "calm", "raw": 2}]
        starts, trans = plan_transitions([4.0] * 4, [0, 1, 2, 3], moods=moods)
        fx = plan_clip_fx(starts, trans, [0, 1, 2, 3], leaks=True, gap=0, quiet=[False, False, True, False])
        name = trans[1][0]
        self.assertIn(f"{name}_in", fx[1])
        self.assertIn(f"{name}_out", fx[0])
        self.assertEqual(trans[2][0], "fadeblack")
        self.assertNotIn("leak", fx[2], "no warm flare in a horror scene")


class AtmosphereTests(unittest.TestCase):
    def test_particles_follow_tags_and_mood(self):
        self.assertEqual(particle_kind("calm", 2, ["winter", "village"]), "snow")
        self.assertEqual(particle_kind("sad", 3, ["rain", "funeral"]), "rain")
        self.assertEqual(particle_kind("sad", 3, ["funeral"]), "rain")
        self.assertEqual(particle_kind("neutral", 4, ["fire"]), "embers")
        self.assertEqual(particle_kind("action", 8, ["arena"]), "embers")
        self.assertIsNone(particle_kind("action", 6, ["arena"]))
        self.assertEqual(particle_kind("mystery", 4, ["cave"]), "dust")
        self.assertIsNone(particle_kind("comedy", 3, ["ice cream", "beach"]), "ice cream is not snow")
        self.assertIsNone(particle_kind("neutral", 4, ["brainstorm"]))

    def test_mood_switches_default_per_look(self):
        self.assertTrue(resolve_look("dynamic")["mood"] and resolve_look("dynamic")["edge_glow"])
        self.assertTrue(resolve_look("cinematic")["mood"])
        self.assertFalse(resolve_look("cinematic")["edge_glow"])
        self.assertFalse(resolve_look("clean")["mood"] or resolve_look("recap")["edge_glow"])
        self.assertFalse(resolve_look({"preset": "dynamic", "edge_glow": False})["edge_glow"])


class EdgeGlowTests(unittest.TestCase):
    def test_glow_only_with_its_switch(self):
        on = look_filters({"edge_glow": True}, 1920, 1080, "a", "b")
        off = look_filters({"edge_glow": False}, 1920, 1080, "a", "b")
        self.assertIn("drawbox", on)
        self.assertEqual(off, "[a]null[b]")
        self.assertTrue(on.startswith("[a]") and on.endswith("[b]"))
        # Filled boxes only (an outline drawbox walks every pixel of the frame).
        self.assertNotIn(":t=1", on)
        self.assertEqual(on.count("drawbox"), on.count("t=fill"))

    def test_glow_sits_inside_letterbox_bars_and_takes_the_mood_colour(self):
        graph = look_filters({"edge_glow": True, "letterbox": True}, 1920, 1080, "a", "b",
                             edge=edge_segments([(0.0, EDGE_COLORS["action"]), (6.0, EDGE_COLORS["sad"])], 0, 10))
        self.assertIn("y=138:", graph)
        self.assertIn("0xff7a34", graph)  # action orange
        self.assertIn("0x70a2ff", graph)  # sad blue
        self.assertIn("between(t,6.000", graph)

    def test_a_long_graph_goes_through_a_file(self):
        from python.video_generator import GRAPH_INLINE, graph_args
        self.assertEqual(graph_args("[a]null[b]", "unused.txt"), ["-filter_complex", "[a]null[b]"])
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "g.txt")
            long = "[a]" + ",".join(["null"] * GRAPH_INLINE) + "[b]"
            self.assertEqual(graph_args(long, path), ["-/filter_complex", path])
            with open(path, encoding="utf-8") as f:
                self.assertEqual(f.read(), long)

    def test_segments_merge_runs_and_cover_the_part(self):
        segs = edge_segments([(0, (1, 2, 3)), (4, (1, 2, 3)), (8, (9, 9, 9))], 2.0, 12.0)
        self.assertEqual(segs, [(2.0, 8, (1, 2, 3)), (8, 12.0, (9, 9, 9))])


class FramingTests(unittest.TestCase):
    def single(self, motion="push", region=(PANEL_TRIM, PANEL_TRIM, 1 - PANEL_TRIM, 1 - PANEL_TRIM), **kw):
        clip = {"motion": motion, "push": 0.08, "ramp": True, "motion_k": 1.0, "region": region}
        clip.update(kw)
        return clip

    def test_duo_tall_panel_hides_nothing(self):
        for iw, ih in ((800, 1290), (400, 2400), (300, 4000)):
            geo = duo_geometry(iw, ih, 1920, 1080, 0)
            self.assertEqual(geo["slack"], 0)
            self.assertEqual(framing_worst({"duo": {"geo": geo}}, 120), 1.0)

    def test_pans_and_pushes_never_hide_the_region(self):
        for motion in ("push", "pull", "pan_left", "pan_right", "pan_up", "pan_down"):
            for frames in (20, 90, 300):
                self.assertGreaterEqual(framing_worst(self.single(motion, motion_k=2.0), frames), 0.999, (motion, frames))
        # Without a region (the old camera) the pan's zoom trims the picture's edge.
        self.assertLess(framing_worst(self.single("pan_right", region=None), 90), 0.999)

    def test_punch_is_centred_on_the_region(self):
        # A face near the right edge: the punch zooms towards it and keeps it whole.
        region = (0.55, 0.2, 0.95, 0.6)
        clip = self.single("push", region=region, impact=True, punch=0.12)
        self.assertGreaterEqual(framing_worst(clip, 60), 0.999)
        # A region as wide as the panel leaves no room to zoom: the punch flattens, still whole.
        clip = self.single("push", region=(0.0, 0.1, 1.0, 0.9), impact=True, punch=0.12)
        self.assertGreaterEqual(framing_worst(clip, 60), 0.999)

    def test_qa_replans_a_clip_that_would_cut(self):
        # A clip whose region is the whole panel but which (wrongly) carries no region for the
        # camera: QA measures the true region and re-plans it.
        bad = {"motion": "pan_right", "push": 0.08, "ramp": False, "motion_k": 2.0, "region": None}
        rec = frame_clip(bad, 90)
        self.assertLess(rec["worst"], 0.999, "with no region the whole picture counts, and the pan's zoom cuts it")
        self.assertIsNotNone(rec["fix"])
        self.assertGreaterEqual(rec["worst_after"], 0.999)
        self.assertNotEqual(bad["motion"], "pan_right")
        # The editor's own move is kept (and reported).
        own = {"motion": "edit", "move": {"kind": "zoom", "amount": 0.3}, "push": 0.08, "region": (0.0, 0.0, 1.0, 1.0)}
        rec = frame_clip(own, 90)
        self.assertEqual(rec["fix"], "editor move kept")
        self.assertEqual(own["motion"], "edit")

    def test_qa_fixes_a_punch_that_overshoots(self):
        # Force an over-strong push next to a punch: the QA's re-plan brings it back to whole.
        clip = {"motion": "pull", "push": 0.5, "ramp": True, "motion_k": 1.0, "impact": True, "punch": 0.4,
                "region": (0.1, 0.1, 0.9, 0.9)}
        rec = frame_clip(clip, 40)
        self.assertGreaterEqual(rec.get("worst_after", rec["worst"]), 0.999)

    def test_panel_crop_region_includes_faces(self):
        panel = {"bbox": [0.1, 0.0, 0.5, 0.5], "characters": [{"face": [0.1, 0.0, 0.16, 0.08]}, {"face": None}]}
        r = crop_region(panel)
        # The face touches the panel's top-left corner: the region reaches the crop's edge there.
        self.assertLess(r[0], PANEL_TRIM)
        self.assertLess(r[1], PANEL_TRIM)
        self.assertEqual((r[2], r[3]), (1 - PANEL_TRIM, 1 - PANEL_TRIM))
        self.assertEqual(crop_region(None), (PANEL_TRIM, PANEL_TRIM, 1 - PANEL_TRIM, 1 - PANEL_TRIM))

    def test_focus_snaps_to_its_panel_and_faces(self):
        panels = [{"bbox": [0.0, 0.0, 1.0, 0.4], "characters": []},
                  {"bbox": [0.0, 0.4, 0.6, 1.0], "characters": [{"face": [0.45, 0.42, 0.58, 0.55]}]}]
        # A box on part of the lower panel snaps out to all of it (and its face).
        r = focus_region([600, 100, 800, 300], panels)  # ymin, xmin, ymax, xmax of 0..1000
        self.assertLessEqual(r[0], 0.0 + 1e-9)
        self.assertLessEqual(r[1], 0.4)
        self.assertGreaterEqual(r[2], 0.6)
        self.assertGreaterEqual(r[3], 0.99)
        # A small box in a splash page stays small but grows to the face it touches.
        splash = [{"bbox": [0, 0, 1, 1], "characters": [{"face": [0.30, 0.30, 0.40, 0.40]}]}]
        r = focus_region([350, 350, 450, 450], splash)
        self.assertLess(r[2] - r[0], 0.5)
        self.assertLessEqual(r[0], 0.30 - 0.025 + 1e-9)
        # The crop only ever widens around the region.
        from PIL import Image
        with tempfile.TemporaryDirectory() as d:
            page = os.path.join(d, "page.jpg")
            Image.new("RGB", (1000, 1500), (200, 200, 200)).save(page)
            path, inner = crop_to_region(page, (0.2, 0.3, 0.4, 0.9), 16 / 9, os.path.join(d, "c.jpg"))
            w, h = Image.open(path).size
            self.assertGreaterEqual(h, 0.6 * 1500 - 2)
            self.assertTrue(0 <= inner[0] < inner[2] <= 1 and 0 <= inner[1] < inner[3] <= 1)
            self.assertAlmostEqual((inner[3] - inner[1]) * h, 0.6 * 1500, delta=3)

    def test_reading_loads_scenes_and_panels(self):
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "studio"))
            with open(os.path.join(d, "studio", "read.json"), "w", encoding="utf-8") as f:
                json.dump({"pages": [{"index": 0, "scene": scene("fight", 8), "panels": [{"bbox": [0, 0, 1, 1]}]}, {"index": 1}]}, f)
            self.assertEqual(load_scenes(d)[0]["kind"], "fight")
            self.assertEqual(load_panels(d)[0][0]["bbox"], [0, 0, 1, 1])
            self.assertEqual(load_scenes(os.path.join(d, "missing")), {})


if __name__ == "__main__":
    unittest.main()
