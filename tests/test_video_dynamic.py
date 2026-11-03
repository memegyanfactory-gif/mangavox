"""The dynamic edit: the chapter card, particle bursts on scene changes, panels coming and going."""
import os
import tempfile
import unittest

from python import video_generator as vg


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

    def test_the_card_is_drawn_and_fits_the_frame(self):
        with tempfile.TemporaryDirectory() as d:
            path, cw, ch = vg.make_chapter_card(1920, 1080, {"number": 1, "title": "A very long chapter title " * 6}, d)
            self.assertTrue(os.path.isfile(path))
            self.assertLess(cw, 0.7 * 1920)
            self.assertLess(ch, 0.2 * 1080)
            self.assertIsNone(vg.make_chapter_card(1920, 1080, {}, d))
            # Long titles (one long word too, and no number) fit a vertical frame.
            for chapter in ({"number": None, "title": "The Day The Whole World Ended And Everyone We Knew Was Gone"}, {"number": 2, "title": "A" * 80}):
                self.assertLess(vg.make_chapter_card(1080, 1920, chapter, d)[1], 1080)

    def test_the_card_moves_on_the_chapters_clock(self):
        g = vg.chapter_card_graph(("card.png", 800, 180), 2560, 1440, 0, "a", "b", "9:v", 90)
        self.assertIn("[9:v]", g)
        self.assertIn("trim=start_frame=90,setpts=(N+90)", g)
        self.assertIn("fade=in", g)
        self.assertIn("fade=out", g)
        # Once the card is done, the picture runs on untouched.
        self.assertIn("eof_action=pass", g)
        self.assertIn(f"loop=loop={round(vg.CARD_OUT * vg.FPS) - 1}", g)


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
