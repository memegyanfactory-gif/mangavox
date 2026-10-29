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
