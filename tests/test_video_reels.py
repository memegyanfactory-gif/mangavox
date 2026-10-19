import unittest

from python.video_generator import get_resolution, plan_reel_cuts


class ReelCutTests(unittest.TestCase):
    def test_short_video_is_one_reel(self):
        self.assertEqual(plan_reel_cuts([10, 20, 40], 70), [(0.0, 70.0)])

    def test_cuts_snap_to_nearest_narration_boundary(self):
        # 3 minutes -> 3 reels; ideal cuts at 60 and 120.
        boundaries = [14.2, 33.0, 52.5, 57.1, 71.0, 98.4, 118.6, 131.0, 160.0]
        cuts = plan_reel_cuts(boundaries, 180.0)
        self.assertEqual(cuts, [(0.0, 57.1), (57.1, 118.6), (118.6, 180.0)])

    def test_no_boundary_nearby_falls_back_to_even_split(self):
        cuts = plan_reel_cuts([5.0], 120.0)
        self.assertEqual(cuts, [(0.0, 60.0), (60.0, 120.0)])

    def test_reels_cover_whole_video_without_tiny_tail(self):
        boundaries = [i * 7.3 for i in range(1, 40)]
        total = 290.0
        cuts = plan_reel_cuts(boundaries, total)
        self.assertEqual(cuts[0][0], 0.0)
        self.assertEqual(cuts[-1][1], total)
        for (a, b), (c, _) in zip(cuts, cuts[1:]):
            self.assertEqual(b, c)
        for a, b in cuts:
            self.assertTrue(40.0 <= b - a <= 80.0, (a, b))

    def test_reels_are_upright(self):
        self.assertEqual(get_resolution("1080p", "reels"), (1080, 1920))
        self.assertEqual(get_resolution("1080p"), (1920, 1080))


if __name__ == "__main__":
    unittest.main()
