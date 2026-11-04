"""The GPU power governor (gpu_power): it holds the draw under the target by duty, eases back to
full speed with headroom, cuts at once on a big peak, and falls back to a fixed duty without a
reading. Driven with fake readings: no GPU, no nvidia-smi."""
import os
import time
import unittest
from unittest import mock

from python import gpu_power as gp


def sampler(watts=None, limit=320.0, now=1000.0):
    s = gp.Sampler(smi="")  # no nvidia-smi: fed by hand
    if watts is not None:
        s.feed(watts, limit, now=now)
    return s


class TargetTests(unittest.TestCase):
    def test_the_setting_wins_else_a_share_of_the_cards_limit(self):
        with mock.patch.dict(os.environ, {"MANGAVOX_GPU_WATTS": "230"}):
            self.assertEqual(gp.target_watts(sampler(100)), 230)
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MANGAVOX_GPU_WATTS", None)
            self.assertEqual(gp.target_watts(sampler(100, 320)), round(320 * gp.DEFAULT_SHARE))
            self.assertIsNone(gp.target_watts(sampler()))

    def test_the_engines_frame_ceiling_follows_the_target(self):
        with mock.patch.dict(os.environ, {"MANGAVOX_GPU_WATTS": "200"}):
            self.assertEqual(gp.fps_cap(), round(200 / 1.3))


class ControllerTests(unittest.TestCase):
    def run_plant(self, full_watts, target, seconds=20.0, idle=40.0):
        """A card drawing idle + duty * (full - idle); the governor steered twice a second."""
        s = sampler(full_watts)
        g = gp.Governor("engine", s=s, target=target)
        now = 1000.0
        for _ in range(int(seconds * 2)):
            now += 0.5
            d = g.steer(now)
            s.feed(idle + d * (full_watts - idle), 320, now=now)
        return g, s

    def test_over_the_target_the_duty_comes_down_until_it_holds(self):
        g, s = self.run_plant(300, 200)
        self.assertLess(g.duty, 0.75)
        self.assertLess(abs(s.smooth - 200), 15, "settles near the target")

    def test_with_headroom_it_runs_flat_out(self):
        g, _ = self.run_plant(150, 200)
        self.assertEqual(g.duty, 1.0)

    def test_a_big_peak_cuts_at_once(self):
        s = sampler(150)
        g = gp.Governor("engine", s=s, target=200)
        g.steer(1000.0)
        s.feed(320, 320, now=1000.5)
        before = g.duty
        g.steer(1000.5)
        self.assertLessEqual(g.duty, before * 0.7 + 1e-9)

    def test_the_rest_is_spread_evenly_and_bounded(self):
        s = sampler(300, now=time.monotonic())  # a live reading: the governor's duty applies
        g = gp.Governor("engine", s=s, target=200)
        g.duty = 0.5
        g.seen = s.samples  # no new reading: the duty stays
        with mock.patch.object(gp.time, "sleep") as sleep:
            g.pace(busy_s=0.02)
            self.assertAlmostEqual(sleep.call_args[0][0], 0.02, places=6)  # busy 50%: rest as long as it worked
            g.pace(busy_s=100.0)
            self.assertEqual(sleep.call_args[0][0], gp.MAX_REST)
        # A loop's own cap applies under the governor's duty.
        g.duty = 1.0
        with mock.patch.object(gp.time, "sleep") as sleep:
            g.pace(busy_s=0.3, max_duty=0.6)
            self.assertAlmostEqual(sleep.call_args[0][0], 0.3 * 0.4 / 0.6, places=6)

    def test_without_a_reading_the_fixed_duty_applies(self):
        g = gp.Governor("voice", s=sampler(), target=None)
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("MANGAVOX_GPU_WATTS", None)
            with mock.patch.object(gp.time, "sleep") as sleep:
                g.pace(busy_s=0.6)
                d = gp.FALLBACK_DUTY["voice"]
                self.assertAlmostEqual(sleep.call_args[0][0], 0.6 * (1 - d) / d, places=6)
        self.assertIn("fixed duty", g.summary())

    def test_stale_readings_count_as_none(self):
        s = sampler(300)
        s.updated = time.monotonic() - 60
        self.assertFalse(s.live())
        self.assertIsNone(gp.Governor("engine", s=s, target=200).steer())


if __name__ == "__main__":
    unittest.main()
