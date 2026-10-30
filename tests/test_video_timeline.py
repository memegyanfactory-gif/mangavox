import math
import re
import unittest

from python.video_generator import (
    FPS, PAGE_TRANSITIONS, SAME_PAGE_TRANSITION, _Progress,
    clip_frames, clip_key, pick_motion, plan_chunks, plan_transitions, xfade_chain,
)


class TransitionPlanTests(unittest.TestCase):
    def test_starts_are_running_sum_of_durations(self):
        durations = [4.117, 7.253, 5.504, 0.6]
        starts, trans = plan_transitions(durations, [0, 1, 1, 2])
        self.assertEqual(len(starts), 4)
        for k in range(1, 4):
            self.assertAlmostEqual(starts[k], sum(durations[:k]), places=9)
        self.assertIsNone(trans[0])

    def test_same_page_dissolves_and_page_changes_rotate(self):
        durations = [5.0] * 5
        _, trans = plan_transitions(durations, [0, 0, 1, 2, 3], base=0.5)
        self.assertEqual(trans[1], (SAME_PAGE_TRANSITION[0], SAME_PAGE_TRANSITION[1]))
        self.assertEqual([t[0] for t in trans[2:]], PAGE_TRANSITIONS[:3])
        self.assertTrue(all(t[1] == 0.5 for t in trans[2:]))

    def test_short_clips_get_shorter_fades(self):
        _, trans = plan_transitions([5.0, 0.6, 5.0], [0, 1, 2], base=0.5)
        self.assertLessEqual(trans[1][1], 0.45 * 0.6 + 1e-9)
        self.assertLessEqual(trans[2][1], 0.45 * 0.6 + 1e-9)

    def test_none_is_a_one_frame_cut(self):
        _, trans = plan_transitions([3.0, 3.0], [0, 1], kind="none")
        self.assertAlmostEqual(trans[1][1], round(1 / FPS, 3))


class XfadeChainTests(unittest.TestCase):
    def test_offsets_put_each_clip_at_its_narration_start(self):
        durations = [7.336, 2.562, 7.36, 0.6, 5.83]
        starts, trans = plan_transitions(durations, [3, 3, 4, 5, 6])
        graph = xfade_chain([f"c{i}" for i in range(5)], starts, trans, "v")
        offsets = [float(x) for x in re.findall(r"offset=([\d.]+)", graph)]
        self.assertEqual(len(offsets), 4)
        for k, off in enumerate(offsets, start=1):
            self.assertAlmostEqual(off, round(starts[k] * FPS) / FPS, delta=0.0015)
            self.assertLess(abs(off - starts[k]), 0.5 / FPS + 0.0015)
        self.assertTrue(graph.endswith("[v]"))

    def test_chunk_offsets_are_relative_to_first_clip(self):
        starts, trans = plan_transitions([2.0] * 6, list(range(6)))
        graph = xfade_chain(["a", "b", "c"], starts[3:6], [None] + trans[4:6], "v")
        offsets = [float(x) for x in re.findall(r"offset=([\d.]+)", graph)]
        self.assertEqual([round(o, 2) for o in offsets], [2.0, 4.0])

    def test_single_clip_passes_through(self):
        self.assertEqual(xfade_chain(["c0"], [0.0], [None], "v"), "[c0]null[v]")

    def test_clip_frames_cover_next_start_plus_fade(self):
        durations = [4.117, 7.253, 5.504]
        starts, trans = plan_transitions(durations, [0, 1, 2])
        for k in range(2):
            frames = clip_frames(starts, trans, k, durations[-1])
            end = round(starts[k] * FPS) + frames
            need = round(starts[k + 1] * FPS) + trans[k + 1][1] * FPS
            self.assertGreaterEqual(end, need)
        last = clip_frames(starts, trans, 2, durations[-1])
        self.assertGreaterEqual(last, math.ceil(durations[-1] * FPS))


class ChunkAndMotionTests(unittest.TestCase):
    def test_chunks_cover_every_clip_once(self):
        chunks = plan_chunks(100, 14)
        self.assertEqual(chunks[0], (0, 14))
        self.assertEqual(chunks[-1][1], 100)
        for (a, b), (c, _) in zip(chunks, chunks[1:]):
            self.assertEqual(b, c)
        self.assertEqual(plan_chunks(5, 14), [(0, 5)])

    def test_motion_never_repeats_back_to_back(self):
        prev = None
        for turn in range(12):
            motion = pick_motion(0.7, 1.6, turn, prev)
            self.assertNotEqual(motion, prev)
            prev = motion


class ResumeAndProgressTests(unittest.TestCase):
    def test_clip_key_is_stable_and_follows_the_render_settings(self):
        a = clip_key(__file__, [0, 0, 500, 500], "push", 90, 1920, 1080, "blur")
        self.assertEqual(a, clip_key(__file__, [0, 0, 500, 500], "push", 90, 1920, 1080, "blur"))
        self.assertNotEqual(a, clip_key(__file__, [0, 0, 500, 500], "pull", 90, 1920, 1080, "blur"))
        self.assertNotEqual(a, clip_key(__file__, [0, 0, 500, 500], "push", 91, 1920, 1080, "blur"))
        self.assertNotEqual(a, clip_key(__file__, None, "push", 90, 1080, 1920, "blur"))

    def test_progress_climbs_through_every_stage_to_the_end(self):
        seen = []
        report = _Progress(lambda stage, cur, total: seen.append(cur), has_chunks=False)
        report("prepare", 1.0)
        for f in (0.0, 0.5, 1.0):
            report("clips", f)
        report("clips", 0.4)  # never goes backwards
        for f in (0.0, 0.5, 1.0):
            report("finalize", f)
        self.assertEqual(seen, sorted(seen))
        # Clips done is not the end: the join and mix still move the bar (no stall at 2/3).
        after_clips, mid_mix, end = seen[-3], seen[-2], seen[-1]
        self.assertLess(after_clips, mid_mix)
        self.assertLess(mid_mix, end)
        self.assertEqual(end, 1000)


if __name__ == "__main__":
    unittest.main()


class LookTests(unittest.TestCase):
    def test_presets_and_overrides(self):
        from python.video_generator import DEFAULT_LOOK, resolve_look
        self.assertEqual(resolve_look(None)["preset"], DEFAULT_LOOK)
        self.assertEqual(resolve_look("nope")["preset"], DEFAULT_LOOK)
        clean = resolve_look("clean")
        self.assertFalse(clean["duo"] or clean["grade"] or clean["impact"])
        mixed = resolve_look({"preset": "dynamic", "glow": False, "grain": True, "duo": False})
        self.assertFalse(mixed["glow"])
        self.assertTrue(mixed["grain"])
        # Only the effect switches can be overridden, not the layout.
        self.assertTrue(mixed["duo"])

    def test_action_beats(self):
        from python.video_generator import is_impact
        self.assertTrue(is_impact("He slams the blade down"))
        self.assertTrue(is_impact("Run!"))
        self.assertFalse(is_impact("A year has passed since then."))
        self.assertFalse(is_impact(""))

    def test_duo_halves_sit_apart_and_tall_panels_pan(self):
        from python.video_generator import duo_geometry
        w, h = 1920, 1080
        left = duo_geometry(800, 1290, w, h, 0)
        right = duo_geometry(800, 1290, w, h, 1)
        self.assertLess(left["pos"][0] + left["size"][0], right["pos"][0])
        self.assertLessEqual(right["pos"][0] + right["size"][0], w)
        self.assertEqual(left["slack"], 0)
        self.assertEqual(left["vis"], left["size"][1])
        strip = duo_geometry(400, 2400, w, h, 0)
        self.assertEqual(strip["vis"], h)
        self.assertGreater(strip["slack"], 0)
        self.assertTrue(all(v % 2 == 0 for v in strip["size"] + strip["pos"]))

    def test_duo_cuts_and_impact_flashes(self):
        starts, trans = plan_transitions([3.0] * 4, [0, 1, 2, 3], names=["zoomin"], joins=[None, "cut", "impact", None])
        self.assertAlmostEqual(trans[1][1], round(1 / FPS, 3))
        self.assertEqual(trans[2], ("fade", 0.12))
        self.assertEqual(trans[3], ("zoomin", 0.5))

    def test_finish_has_only_what_is_on(self):
        from python.video_generator import look_filters
        self.assertEqual(look_filters({}, 1280, 720, "a", "b"), "[a]null[b]")
        full = look_filters({"grade": True, "vignette": True, "grain": True}, 1280, 720, "a", "b")
        self.assertIn("lutyuv", full)
        self.assertIn("vignette", full)
        self.assertIn("noise", full)
        self.assertTrue(full.startswith("[a]") and full.endswith("[b]"))


class NewEffectTests(unittest.TestCase):
    def test_new_switches_and_colour_default_to_the_preset(self):
        from python.video_generator import resolve_look
        dyn = resolve_look("dynamic")
        self.assertTrue(dyn["leaks"] and dyn["chroma"] and dyn["parallax"] and dyn["ramp"])
        self.assertFalse(dyn["particles"] or dyn["letterbox"])
        self.assertIn("burn", dyn["transitions"])
        self.assertIn("zoomblur", dyn["transitions"])
        cine = resolve_look("cinematic")
        self.assertTrue(cine["leaks"] and cine["letterbox"])
        self.assertEqual(cine["color"], "warm")
        # An old look file (no new keys) keeps the preset's; junk colours are ignored.
        old = resolve_look({"preset": "cinematic", "grain": False, "color": "sepia"})
        self.assertFalse(old["grain"])
        self.assertTrue(old["letterbox"])
        self.assertEqual(old["color"], "warm")
        mixed = resolve_look({"preset": "clean", "particles": True, "letterbox": True, "color": "noir"})
        self.assertTrue(mixed["particles"] and mixed["letterbox"])
        self.assertEqual(mixed["color"], "noir")

    def test_colour_presets_share_one_table_with_the_grade(self):
        from python.video_generator import COLOR_PRESETS, color_lut, look_filters
        self.assertIsNone(color_lut(False, "none"))
        for name in COLOR_PRESETS:
            if name != "none":
                y, u, v = color_lut(True, name)
                self.assertIn("val", y)
        noir = color_lut(True, "noir")
        self.assertEqual(noir[1:], ("128", "128"))
        graph = look_filters({"grade": True, "color": "vivid"}, 1280, 720, "a", "b")
        self.assertEqual(graph.count("lutyuv"), 1)

    def test_letterbox_and_particles_in_the_finish(self):
        from python.video_generator import letterbox_bar, look_filters
        self.assertEqual(letterbox_bar(1920, 1080), 138)
        self.assertEqual(letterbox_bar(1080, 1920), 0)
        graph = look_filters({"letterbox": True, "particles": True}, 1280, 720, "a", "b", particles="3:v")
        self.assertEqual(graph.count("drawbox"), 2)
        self.assertIn("[3:v]overlay", graph)
        # Particles need their stream; upright frames get no bars.
        self.assertNotIn("overlay", look_filters({"particles": True}, 1280, 720, "a", "b"))
        self.assertEqual(look_filters({"letterbox": True}, 720, 1280, "a", "b"), "[a]null[b]")

    def test_clip_drawn_transitions_dissolve_and_both_sides_draw_them(self):
        from python.video_generator import plan_clip_fx
        durations = [4.0] * 5
        pages = [0, 1, 2, 3, 3]
        starts, trans = plan_transitions(durations, pages, names=["burn", "zoomblur", "fade"])
        graph = xfade_chain([f"c{i}" for i in range(5)], starts, trans, "v")
        self.assertNotIn("burn", graph)
        self.assertNotIn("zoomblur", graph)
        fx = plan_clip_fx(starts, trans, pages, leaks=True, gap=0)
        self.assertEqual(fx[0]["burn_out"], (4.0, 0.5))
        self.assertEqual(fx[1]["burn_in"], 0.5)
        self.assertEqual(fx[0]["burn_flip"], fx[1]["burn_flip_in"])
        self.assertEqual(fx[1]["zoomblur_out"], (4.0, 0.5))
        self.assertEqual(fx[2]["zoomblur_in"], 0.5)
        self.assertTrue(fx[0]["leak"] and fx[3]["leak"])
        # Same page: no page-turn effects.
        self.assertEqual(fx[4], {})

    def test_leaks_are_spaced_and_skip_action_beats(self):
        from python.video_generator import plan_clip_fx
        durations = [3.0] * 8
        pages = list(range(8))
        starts, trans = plan_transitions(durations, pages, names=["fade"])
        fx = plan_clip_fx(starts, trans, pages, leaks=True, impacts=[False] * 4 + [True] + [False] * 3, gap=6.0)
        lit = [k for k, f in enumerate(fx) if f.get("leak")]
        self.assertEqual(lit, [0, 2, 5, 7])
        self.assertEqual(plan_clip_fx(starts, trans, pages), [{}] * 8)

    def test_speed_ramp_snaps_short_beats_and_drifts_long_ones(self):
        from python.video_generator import ramp_ease

        def at(expr, n):
            env = {"min": min, "pow": pow, "cos": math.cos, "PI": math.pi, "on": n}
            return eval(expr, {"__builtins__": {}}, env)

        short = ramp_ease("on", 30, True)
        self.assertGreater(at(short, 7), 0.6)  # most of the move in its first quarter
        self.assertAlmostEqual(at(short, 29), 1.0)
        long = ramp_ease("on", 300, True)
        self.assertGreater(at(long, 15), 0.03)  # already drifting
        self.assertAlmostEqual(at(long, 299), 1.0)
        self.assertEqual(ramp_ease("on", 300, False), ramp_ease("on", 300))
