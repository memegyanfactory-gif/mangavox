"""The GPU engine's plan (gpu_timeline) and the ffmpeg expressions it is made from (ffexpr).
Pure numpy: no GPU, no ffmpeg."""
import math
import unittest

import numpy as np

from python import ffexpr
from python import gpu_timeline as gt
from python import video_generator as vg


class ExpressionTests(unittest.TestCase):
    def test_precedence_and_functions_as_ffmpeg_has_them(self):
        ev = lambda s, **v: float(ffexpr.evaluate(s, **v))
        self.assertEqual(ev("1+2*3"), 7)
        self.assertEqual(ev("-2^2"), -4)  # the sign takes the whole power (eval.c)
        self.assertEqual(ev("2^3^2"), 64)  # left to right
        self.assertEqual(ev("mod(-1,30)"), 29)
        self.assertEqual(ev("clip(300,0,255)"), 255)
        self.assertEqual(ev("if(gte(t,1),5,7)", t=1.0), 5)
        self.assertEqual(ev("between(2,1,3)*lt(1,2)"), 1)
        self.assertEqual(ev("trunc(-2.5)+hypot(3,4)"), 3)
        self.assertAlmostEqual(ev("(0.5-0.5*cos(PI*on/89))", on=89), 1.0)
        self.assertEqual(ev("st(0,4);ld(0)*2"), 8)

    def test_perspective_counts_from_one(self):
        # vf_perspective: on = frame_count_in + 1; crop and overlay count n from 0.
        self.assertEqual(list(ffexpr.frames("on", 3, on_base=1)), [1, 2, 3])
        self.assertEqual(list(ffexpr.frames("n", 3, on_base=1)), [0, 1, 2])
        self.assertEqual(list(gt.camera_series("on", 3)), [1, 2, 3])

    def test_a_whole_clip_at_once(self):
        e = ffexpr.frames(vg._ease("on", 90), 90)
        self.assertEqual(e.shape, (90,))
        self.assertAlmostEqual(e[0], 0.0)
        self.assertAlmostEqual(e[-1], 1.0)
        # A constant is a constant on every frame.
        self.assertTrue(np.all(ffexpr.frames("42", 5) == 42))

    def test_the_renderers_own_expressions(self):
        t = np.arange(30) / 30
        shake = ffexpr.evaluate(vg._shake(20, "x"), t=t)
        self.assertAlmostEqual(float(shake[3]), 20 * math.sin(2 * math.pi * 0.1 * 13) * math.exp(-0.1 * round(3.5 / vg.IMPACT_SEC, 2)), places=6)
        slide = ffexpr.evaluate(vg._panel_move({"panel_in": (0.65, -1)}, 2560, "x"), t=t)
        self.assertAlmostEqual(float(slide[0]), -128)
        _, at = vg.edge_moves(2560, 1440, 0)
        x = ffexpr.evaluate(at["light_top"][0], t=t)
        self.assertEqual(x.shape, (30,))


def single_clip(**kw):
    c = {"images": {"fg": "fg.jpg", "bg": "bg.jpg", "fg_pos": (130, 72), "fg_size": (2300, 1296), "drift": 0},
         "motion": "push", "move": None, "impact": False, "ramp": False, "push": 0.05, "motion_k": 1.0,
         "punch": 0.12, "region": None, "fx": {}, "assets": {}, "duo": None}
    c.update(kw)
    return c


class ClipPlanTests(unittest.TestCase):
    def test_a_push_starts_on_the_whole_panel_and_closes_in(self):
        p = gt.plan_clip(single_clip(), 90, 2560, 1440)
        bg, fg = p["layers"]
        self.assertEqual(bg["crop"], [0, 0])
        x, y, cw, ch = fg["warp"]
        self.assertAlmostEqual(cw[0], 2300, places=1)
        self.assertLess(cw[-1], 2300 / 1.04)
        self.assertEqual(fg["pos"], [130, 72])
        self.assertIsNone(p["flash"])
        self.assertEqual(p["fx"], [])

    def test_an_impact_jolts_on_even_pixels_and_flashes(self):
        p = gt.plan_clip(single_clip(impact=True), 30, 2560, 1440)
        xs = p["layers"][1]["pos"][0]
        self.assertIsInstance(xs, list)
        self.assertTrue(all(v % 2 == 0 for v in xs))
        self.assertGreater(p["flash"][0], 0.3)
        self.assertEqual(p["flash"][-1], 0.0)

    def test_parallax_crops_the_background_inside_it(self):
        c = single_clip()
        c["images"]["drift"] = 40
        bx, by = gt.plan_clip(c, 120, 2560, 1440)["layers"][0]["crop"]
        for v in (bx, by):
            v = v if isinstance(v, list) else [v]
            self.assertTrue(all(0 <= a <= 80 and a % 2 == 0 for a in v))

    def test_clip_effects_land_on_their_frames(self):
        c = single_clip(fx={"zoomblur_out": (2.0, 0.35), "whipleft_in": 0.22}, assets={"leak": ("leak_%03d.png", 33)},
                        chroma=True, impact=True, chroma_k=1.0)
        c["fx"]["leak"] = True
        ops = {f["op"]: f for f in gt.plan_fx(c, 90, 2560, 1440)}
        self.assertEqual(ops["zoomblur"]["on"], [60, 90])
        self.assertEqual(ops["boxblur"]["on"], [0, round(0.22 * 30)])
        self.assertEqual(ops["sequence"]["on"], [0, 33])
        self.assertIn("rgbshift", ops)

    def test_an_unknown_effect_goes_to_the_cpu_path(self):
        with self.assertRaises(gt.Unsupported):
            gt.plan_fx(single_clip(fx={"vortex_in": 1.0}), 30, 2560, 1440)

    def test_a_layered_panel_is_animated_by_the_engine(self):
        c = single_clip()
        c["images"]["fg_layered"] = {"id": "j1", "cache": "c", "size": [2300, 1296], "frames": 90}
        fg = gt.plan_clip(c, 90, 2560, 1440)["layers"][1]
        self.assertEqual(fg["animate"]["id"], "j1")
        self.assertFalse(fg["video"])
        self.assertIn("warp", fg)

    def test_a_clip_already_rendered_is_read_as_it_is(self):
        self.assertEqual(gt.plan_clip(single_clip(), 30, 2560, 1440, prerendered="c.mp4"), {"kind": "video", "frames": 30, "path": "c.mp4"})


class VideoPlanTests(unittest.TestCase):
    def video(self, trans):
        return {"starts": [0.0, 3.0], "trans": [None, trans], "look": None, "overlays": [], "work_dir": "."}

    def test_joins_are_xfades_on_the_videos_clock(self):
        plan = gt.plan_video(self.video(("burn", 0.9)), [{}, {}], 2560, 1440, 6.0)
        self.assertEqual(plan["first"], [0, 90])
        self.assertEqual(plan["joins"][1]["name"], "fade")  # a burn's join is a plain fade under the clips' flare
        self.assertAlmostEqual(plan["joins"][1]["offset"], 2.999)
        self.assertEqual(plan["frames"], 180)

    def test_a_transition_it_cannot_draw_goes_to_the_cpu_path(self):
        with self.assertRaises(gt.Unsupported):
            gt.plan_video(self.video(("hblur", 0.5)), [{}, {}], 2560, 1440, 6.0)

    def test_the_card_drops_in_and_fades(self):
        v = dict(self.video(("fade", 0.5)), chapter_card=("card.png", 800, 180))
        card = gt.plan_video(v, [{}, {}], 2560, 1440, 8.0)["card"]
        self.assertEqual(card["on"], [0, round(vg.CARD_OUT * 30)])
        self.assertEqual(card["alpha"][0], 0.0)
        self.assertEqual(max(card["alpha"]), 1.0)
        # Top right: it glides in from beyond the right edge, rests CARD_MARGIN in, glides out.
        xs = card["pos"][0]
        rest = (2560 - round(vg.CARD_MARGIN * 1440 / 1080) - 800) & ~1
        self.assertGreaterEqual(xs[0], 2560)
        self.assertEqual(xs[round(3.0 * 30)], rest)
        self.assertTrue(all(v % 2 == 0 for v in xs))

    def test_the_lookup_table_is_the_lutyuv_one(self):
        look = gt.plan_look({}, {"grade": True, "color": None}, 2560, 1440, 30)
        y = look["lut"][0]
        self.assertEqual(len(y), 256)
        self.assertEqual(y[100], 100)
        self.assertEqual(y[200], int(min(255, (200 - 100) * 1.08 + 100)))


if __name__ == "__main__":
    unittest.main()
