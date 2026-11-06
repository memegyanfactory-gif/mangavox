"""The dynamic edit: the chapter card, particle bursts on scene changes, panels coming and going."""
import os
import tempfile
import unittest

import numpy as np
from PIL import Image

from python import video_generator as vg


def load(path):
    """A badge frame as an RGBA image, its file closed again."""
    with Image.open(path) as img:
        return img.convert("RGBA")


def mood(name, kind="talk", raw=5):
    return {"mood": name, "kind": kind, "raw": raw}


class ChapterCardTests(unittest.TestCase):
    def test_the_card_names_the_chapter(self):
        self.assertEqual(vg.chapter_card_text({"number": 12, "title": "The Duel"}), ("CHAPTER 12", "The Duel"))
        self.assertEqual(vg.chapter_card_text({"number": 3.5, "title": None}), ("CHAPTER 3.5", ""))
        # A title that only repeats the number is left out, a prefixed one loses the prefix.
        self.assertEqual(vg.chapter_card_text({"number": 4, "title": "Chapter 4"}), ("CHAPTER 4", ""))
        self.assertEqual(vg.chapter_card_text({"number": 7, "title": "Ch. 7 - Night Falls"}), ("CHAPTER 7", "Night Falls"))
        self.assertEqual(vg.chapter_card_text({"number": None, "title": "Oneshot"}), ("CHAPTER", "Oneshot"))
        self.assertEqual(vg.chapter_card_text({"number": None, "title": "Chapter 5"}), ("CHAPTER 5", ""))
        self.assertIsNone(vg.chapter_card_text({"number": None, "title": "  "}))
        # A title the card's font cannot draw is left out rather than drawn as boxes.
        self.assertEqual(vg.chapter_card_text({"number": 12, "title": "第12話"}), ("CHAPTER 12", ""))
        self.assertIsNone(vg.chapter_card_text({"number": None, "title": "第12話"}))
        self.assertIsNone(vg.chapter_card_text(None))

    def test_tags_and_junk_never_reach_the_card(self):
        clean = vg.clean_chapter_title
        # A scanlation group's tag in brackets or parentheses, alone or around a real title.
        self.assertEqual(vg.chapter_card_text({"number": 2, "title": "(1r0n)"}), ("CHAPTER 2", ""))
        self.assertEqual(clean("Night Falls (1r0n)"), "Night Falls")
        self.assertEqual(clean("[Asura Scans] Chapter 12 - The Duel"), "The Duel")
        self.assertEqual(clean("【Team X】 Ch.3: Rain"), "Rain")
        # Prefixes that only repeat the number go, however they are written.
        self.assertEqual(clean("Vol.2 Ch.12: Rain"), "Rain")
        self.assertEqual(clean("Episode 3 - A New Dawn"), "A New Dawn")
        self.assertEqual(clean("#12: Home"), "Home")
        for junk in ("1r0n", "Ch. 2", "Chapter 2", "Vol. 3", "v2", "x", "--", "Kaiju Scans", "MTL", "Raw"):
            self.assertEqual(clean(junk), "", junk)
        # Real titles stay as they are, numbers in them too.
        for title in ("The Duel", "100 Girlfriends", "Oneshot", "Team Up!", "Level 99 Hero"):
            self.assertEqual(clean(title), title)

    def test_the_card_can_be_switched_off(self):
        self.assertTrue(vg.chapter_card_on({}, vg.resolve_look("dynamic")))
        self.assertFalse(vg.chapter_card_on({}, vg.resolve_look({"preset": "dynamic", "chapter_card": False})))
        self.assertFalse(vg.chapter_card_on({"chapter_card": False}, vg.resolve_look("clean")))
        self.assertTrue(vg.chapter_card_on({}, vg.resolve_look({"preset": "clean", "chapter_card": True})))

    def test_the_badge_sits_top_right_and_stays_put(self):
        for w, h, bar in ((2560, 1440, 0), (1920, 1080, 0), (1080, 1920, 0), (1920, 1080, 120)):
            x, y = vg.chapter_card_pos(w, h, bar)
            d, pad, size, margin = vg.badge_geometry(w, h)
            # The circle's right edge is the margin in from the frame's (to the even pixel), its top the margin below the bar.
            self.assertLessEqual(abs((x + pad + d) - (w - margin)), 1)
            self.assertLessEqual(abs((y + pad) - (bar + margin)), 1)
            self.assertEqual((x % 2, y % 2), (0, 0))
            self.assertLess(y, bar + 0.1 * min(w, h))

    def test_the_badge_is_a_small_circle(self):
        self.assertEqual(vg.badge_geometry(1920, 1080)[0], 64)
        for w, h in ((1920, 1080), (1080, 1920), (2560, 1440), (1280, 720)):
            d = vg.badge_geometry(w, h)[0]
            self.assertAlmostEqual(d / min(w, h), 0.06, delta=0.01)
            self.assertEqual(d % 2, 0)

    def test_no_number_no_badge(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(vg.make_chapter_card(1920, 1080, {}, d))
            self.assertIsNone(vg.make_chapter_card(1920, 1080, {"number": None, "title": "Oneshot"}, d))
            self.assertIsNone(vg.make_chapter_card(1920, 1080, None, d))
            self.assertEqual(os.listdir(d), [])
        for chapter, number in (({"number": 2}, "2"), ({"number": 12.0}, "12"), ({"number": 12.5}, "12.5"), ({"number": 104}, "104"),
                                ({"number": None, "title": "Chapter 5"}, "5")):
            self.assertEqual(vg.chapter_badge_number(chapter), number)

    def test_the_badge_is_six_seconds_of_frames(self):
        self.assertAlmostEqual(vg.CARD_OUT - vg.CARD_IN, 6.0)
        with tempfile.TemporaryDirectory() as d:
            pattern, count, size = vg.make_chapter_card(1920, 1080, {"number": 2}, d)
            self.assertEqual(count, round(vg.CARD_OUT * vg.FPS))
            self.assertEqual(len(os.listdir(d)), count)
            self.assertEqual(os.path.basename(pattern), "chapter_badge_%04d.png")
            self.assertEqual(size, vg.badge_geometry(1920, 1080)[2])
            alpha = [load(pattern % i).getchannel("A").getextrema()[1] for i in range(count)]
            # Nothing before it appears, solid in the middle, nearly gone as it ends.
            self.assertTrue(all(v == 0 for v in alpha[:round(vg.CARD_IN * vg.FPS)]))
            self.assertGreater(alpha[round((vg.CARD_IN + 0.5) * vg.FPS)], 150)
            self.assertLess(alpha[-1], 40)
            self.assertEqual(load(pattern % 0).size, (size, size))

    def test_the_circle_is_six_percent_of_the_short_side_and_dark(self):
        with tempfile.TemporaryDirectory() as d:
            for w, h in ((1920, 1080), (1080, 1920)):
                pattern, count, size = vg.make_chapter_card(w, h, {"number": 7}, d)
                px = np.asarray(load(pattern % round(3 * vg.FPS)))
                # The disc is what is at least half opaque along its middle row.
                width = int((px[size // 2, :, 3] > 100).sum())
                self.assertAlmostEqual(width / min(w, h), 0.06, delta=0.012)
                self.assertGreater(px[size // 2, size // 2 - int(0.2 * width), 3], 150)  # inside the disc
                self.assertEqual(px[0, 0, 3], 0)  # the corner of the frame is clear

    def test_the_ring_fills_clockwise_from_twelve(self):
        with tempfile.TemporaryDirectory() as d:
            pattern, count, size = vg.make_chapter_card(1920, 1080, {"number": 2}, d)
            dia = vg.badge_geometry(1920, 1080)[0]
            mid = size / 2
            yy, xx = np.mgrid[0:size, 0:size]
            r = np.hypot(xx + 0.5 - mid, yy + 0.5 - mid)
            band = (r > 0.42 * dia) & (r < 0.5 * dia)
            ang = np.degrees(np.arctan2(xx + 0.5 - mid, -(yy + 0.5 - mid))) % 360

            def lit(i):
                """The ring's bright pixels by quarter: from 12 o'clock clockwise."""
                px = np.asarray(load(pattern % i)).astype(int)
                on = band & (px[..., 0] > 200) & (px[..., 3] > 200)
                return [int((on & (ang >= q * 90) & (ang < q * 90 + 90)).sum()) for q in range(4)]

            fps = vg.FPS
            early, half, late = (lit(round((vg.CARD_IN + s) * fps)) for s in (1.5, 3.0, 5.0))
            self.assertLess(sum(early), sum(half))
            self.assertLess(sum(half), sum(late))
            # A quarter of the way: 12 to 3 o'clock is lit, 9 to 12 is not.
            self.assertGreater(early[0], 3 * max(early[3], 1))
            # Half way: the right half is lit, the left is not.
            self.assertGreater(half[1], 0.6 * half[0])
            self.assertLess(half[3], 0.2 * half[0])
            self.assertGreater(late[2], 0.5 * late[0])

    def test_any_number_fits_inside_the_ring(self):
        with tempfile.TemporaryDirectory() as d:
            for number in (2, 12, 104, 12.5, 1000):
                pattern, count, size = vg.make_chapter_card(1920, 1080, {"number": number}, d)
                px = np.asarray(load(pattern % round(3 * vg.FPS))).astype(int)
                dia = vg.badge_geometry(1920, 1080)[0]
                yy, xx = np.mgrid[0:size, 0:size]
                r = np.hypot(xx + 0.5 - size / 2, yy + 0.5 - size / 2)
                digits = (px[..., 0] > 200) & (px[..., 3] > 200) & (r < 0.42 * dia)
                self.assertTrue(digits.any(), number)
                cols = np.where(digits.any(axis=0))[0]
                # Clear of the ring on both sides, and centred.
                self.assertLess(cols[-1] - cols[0] + 1, 0.6 * dia, number)
                self.assertLessEqual(abs((cols[0] + cols[-1]) / 2 - size / 2), 2, number)

    def test_the_badge_shows_on_the_chapters_clock(self):
        g = vg.chapter_card_graph(("chapter_badge_%04d.png", 190, 84), 2560, 1440, 0, "a", "b", "9:v", 90)
        self.assertIn("[9:v]", g)
        self.assertIn("setpts=(N+90)/(", g)
        self.assertIn("overlay=x=", g)
        self.assertNotIn("fade", g)
        self.assertNotIn("loop", g)
        # Once the badge is done, the picture runs on untouched.
        self.assertIn("eof_action=pass", g)

    def test_the_badge_graph_renders(self):
        import shutil
        import subprocess
        if not shutil.which("ffmpeg"):
            self.skipTest("no ffmpeg")
        with tempfile.TemporaryDirectory() as d:
            w, h, a = 640, 360, 30
            pattern, count, size = vg.make_chapter_card(w, h, {"number": 12}, d)
            graph = vg.chapter_card_graph((pattern, count, size), w, h, 0, "0:v", "o", "1:v", a)
            out = os.path.join(d, "o.mp4")
            cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", f"color=c=0x808080:s={w}x{h}:r={vg.FPS}:d=2",
                   "-framerate", str(vg.FPS), "-start_number", str(a), "-i", pattern, "-filter_complex", graph, "-map", "[o]", "-frames:v", "45", out]
            done = subprocess.run(cmd, capture_output=True, timeout=120)
            self.assertEqual(done.returncode, 0, done.stderr.decode(errors="replace")[:400])
            self.assertTrue(os.path.getsize(out) > 0)


class SceneBurstTests(unittest.TestCase):
    def test_a_new_scene_bursts_once_in_a_while(self):
        durs = [8.0] * 12
        pages = list(range(12))
        moods = [mood("calm")] * 3 + [mood("mystery")] * 3 + [mood("romance")] * 3 + [mood("reveal")] * 3
        starts, trans = vg.plan_transitions(durs, pages, moods=moods)
        bursts = [k for k, t in enumerate(trans) if t and t[0] == "particles"]
        self.assertTrue(bursts)
        self.assertEqual(bursts[0], 3)
        for a, b in zip(bursts, bursts[1:]):
            self.assertGreaterEqual(starts[b] - starts[a], vg.SCENE_GAP)

    def test_action_horror_and_sad_scenes_keep_their_own_entrances(self):
        _, trans = vg.plan_transitions([40.0] * 4, [0, 1, 2, 3], moods=[mood("calm"), mood("action", "fight"), mood("horror"), mood("sad")])
        self.assertFalse(any(t and t[0] == "particles" for t in trans))

    def test_beats_of_one_page_never_burst(self):
        _, trans = vg.plan_transitions([8.0] * 4, [1, 1, 1, 1], moods=[mood("calm"), mood("action"), mood("sad"), mood("calm")])
        self.assertFalse(any(t and t[0] == "particles" for t in trans))

    def test_the_burst_is_drawn_once_over_a_pixel_break_up(self):
        starts, trans = vg.plan_transitions([8.0, 8.0], [0, 1], moods=[mood("calm"), mood("mystery")])
        self.assertEqual(trans[1][0], "particles")
        # The clips draw nothing of it (the break-up would blur it); the finish lays it over the join.
        fx = vg.plan_clip_fx(starts, trans, [0, 1])
        self.assertEqual(fx, [{}, {}])
        self.assertIn("pixelize", vg.xfade_chain(["a", "b"], starts, trans, "v"))
        (f0, rgb), = vg.scene_bursts(starts, trans, [(1, 2, 3), (4, 5, 6)])
        self.assertEqual(rgb, (4, 5, 6))
        self.assertEqual(f0, round((8.0 + trans[1][1] / 2 - vg.BURST_PEAK) * vg.FPS))

    def test_burst_frames_are_kept_per_colour(self):
        with tempfile.TemporaryDirectory() as d:
            os.environ["MANGAVOX_FX_CACHE"] = d
            try:
                pat, n = vg.make_flare_frames("burst", 320, 180, d, (96, 226, 170))
                self.assertEqual(n, round(vg.BURST_SEC * vg.FPS))
                self.assertIn("60e2aa", pat)
                self.assertTrue(os.path.isfile(pat % (n - 1)))
            finally:
                os.environ.pop("MANGAVOX_FX_CACHE", None)


class PanelMoveTests(unittest.TestCase):
    def test_panels_of_one_page_come_and_go(self):
        starts = [0.0, 5.0, 10.0, 15.0]
        trans = [None, ("fade", 0.35), ("fade", 0.35), ("smoothleft", 0.5)]
        moves = vg.plan_panel_moves(starts, trans, [1, 1, 1, 2])
        self.assertEqual(moves[0]["panel_out"], (5.0, 0.35, 1))
        self.assertEqual(moves[1]["panel_in"][1], 1)
        # The next swap goes the other way; a page turn keeps its own transition.
        self.assertEqual(moves[1]["panel_out"][2], -1)
        self.assertNotIn("panel_out", moves[2])
        self.assertNotIn("panel_in", moves[3])

    def test_cuts_duos_and_the_editors_transitions_keep_their_entrance(self):
        starts = [0.0, 5.0, 10.0, 15.0]
        trans = [None, ("fade", 0.35), ("fade", 0.35), ("fade", 0.35)]
        moves = vg.plan_panel_moves(starts, trans, [1, 1, 1, 1], joins=[None, "cut", None, None],
                                    duo=[False, False, True, False], forced=[None, None, None, ("wipeleft", 0.45)])
        self.assertEqual(moves, [{}, {}, {}, {}])

    def test_the_same_panel_again_is_a_hold(self):
        # A caption-only page repeats the last panel: it neither slides nor turns a page.
        starts, trans = [0.0, 5.0], [None, ("fade", 0.35)]
        self.assertEqual(vg.plan_panel_moves(starts, trans, [3, 3], holds=[False, True]), [{}, {}])

    def test_the_move_is_an_overlay_offset(self):
        e = vg._panel_move({"panel_in": (0.65, -1), "panel_out": (9.0, 0.35, -1)}, 2560, "x")
        self.assertIn("(-128)*pow(max(0,1-t/0.65),3)", e)
        self.assertIn("(128)*pow(min(1,max(0,(t-9.0)/0.35)),2)", e)
        self.assertEqual(vg._panel_move({"panel_in": (0.65, 1)}, 2560, "y"), "0")
        self.assertEqual(vg._panel_move({}, 2560, "x"), "0")


if __name__ == "__main__":
    unittest.main()
