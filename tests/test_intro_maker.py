"""The intro (intro_maker): it is the cards plus the ribbon transition, exactly that many frames at any
frame rate; the last card stays and turns into the transition; the music is read so its big moment
lands as the camera passes through the ribbons whatever the number of cards; the colours come from
the cards; and nothing but the cards' own pictures is read: a cover, a painted world plate or any
other leftover of an older app in the args is never opened. Rendered small so it runs quickly."""
import builtins
import colorsys
import json
import math
import os
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import numpy as np
from PIL import Image, ImageDraw

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "python"))
import intro_maker as im  # noqa: E402
import intro_transition as tr  # noqa: E402

W, H = 192, 108


def figure(path, colour=(40, 90, 220)):
    """A standing figure on transparency (a kept drawing)."""
    im_ = Image.new("RGBA", (120, 300), (0, 0, 0, 0))
    d = ImageDraw.Draw(im_)
    d.ellipse((40, 5, 80, 50), fill=(235, 200, 170, 255))
    d.rectangle((25, 50, 95, 290), fill=colour + (255,))
    im_.save(path)
    return path


def painting(path, colour=(200, 60, 40), size=(1376, 768)):
    """A painted 16:9 background in one hue family."""
    w, h = size
    yy, xx = np.mgrid[0:h, 0:w]
    hh, ll, ss = colorsys.rgb_to_hls(*(c / 255 for c in colour))
    base = np.array(colorsys.hls_to_rgb(hh, 0.45, ss)) * 255
    a = base[None, None, :] * (0.55 + 0.45 * (yy / h))[..., None] + np.random.default_rng(1).normal(0, 8, (h, w, 3))
    Image.fromarray(np.clip(a, 0, 255).astype(np.uint8)).save(path)
    return path


def frame_count(video):
    out = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-select_streams", "v", "-show_entries", "stream=nb_read_frames",
                          "-of", "csv=p=0", video], capture_output=True, text=True).stdout.strip()
    return int(out)


def last_frame(video, w, h):
    raw = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-sseof", "-0.2", "-i", video, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                         capture_output=True).stdout
    n = len(raw) // (w * h * 3)
    return np.frombuffer(raw[(n - 1) * w * h * 3: n * w * h * 3], np.uint8).reshape(h, w, 3)


class LengthTests(unittest.TestCase):
    def test_the_cards_then_the_transition(self):
        with open(im.__file__, encoding="utf-8") as f:
            src = f.read()
        # The app reads these as plain "NAME = number" lines (and uses the same values).
        self.assertEqual(float(re.search(r"^CARD_SEC = ([0-9.]+)", src, re.M).group(1)), 2.5)
        self.assertEqual(float(re.search(r"^SLIDE_SEC = ([0-9.]+)", src, re.M).group(1)), 0.32)
        self.assertEqual(int(re.search(r"^MAX_CARDS = ([0-9]+)", src, re.M).group(1)), 4)
        for n in range(1, im.MAX_CARDS + 1):
            self.assertAlmostEqual(im.total_seconds(n), n * im.CARD_SEC + tr.RIBBON_SEC + tr.REVEAL_SEC)
        # Four cards: 4 x 2.5 + 1.2 + 0.5.
        self.assertAlmostEqual(im.total_seconds(4), 11.7)

    def test_frames_at_any_rate(self):
        for fps in (24.0, 25.0, 30000 / 1001, 30.0, 50.0, 60.0):
            for n in range(1, im.MAX_CARDS + 1):
                total, cards = im.frame_counts(n, fps)
                self.assertEqual(total, int(round(im.total_seconds(n) * fps)), (fps, n))
                self.assertEqual(cards, int(round(n * im.CARD_SEC * fps)), (fps, n))
                self.assertGreater(total - cards, 0)
        # Four cards, exactly: 351 frames at 30 fps (300 of cards), 702 at 60, 281 at 24, 293 at 25.
        self.assertEqual(im.frame_counts(4, 30.0), (351, 300))
        self.assertEqual(im.frame_counts(4, 60.0), (702, 600))
        self.assertEqual(im.frame_counts(4, 24.0), (281, 240))
        self.assertEqual(im.frame_counts(4, 25.0), (292, 250))
        self.assertEqual(im.frame_counts(4, 30000 / 1001), (351, 300))


class MusicTests(unittest.TestCase):
    def test_the_big_moment_lands_on_the_pass_with_any_number_of_cards(self):
        # The app renders the music with its big moment at MAX_CARDS x CARD_SEC + PASS_SEC: with
        # fewer cards it is read from later on, so the moment still lands as the camera passes.
        music = os.path.join(tempfile.mkdtemp(), "music.wav")
        im.write_wav_stereo(music, np.zeros((im.SR, 2)))
        for n in range(1, im.MAX_CARDS + 1):
            calls = []

            def fake(path, seconds, start=0.0):
                calls.append((path, seconds, start))
                return np.zeros((int(seconds * im.SR), 2))

            with mock.patch.object(im, "decode_stereo", fake):
                x = im.soundtrack(n, music, "epic")
            self.assertEqual(len(calls), 1)
            _, seconds, start = calls[0]
            self.assertAlmostEqual(start, (im.MAX_CARDS - n) * im.CARD_SEC)
            self.assertAlmostEqual(start + n * im.CARD_SEC + tr.PASS_SEC, im.MAX_CARDS * im.CARD_SEC + tr.PASS_SEC)
            self.assertEqual(len(x), int(round(im.total_seconds(n) * im.SR)))
            self.assertTrue(np.isfinite(x).all() and np.abs(x).max() <= 10 ** (-1 / 20) + 1e-9)

    def test_the_transition_is_heard_where_the_camera_passes(self):
        x = im.soundtrack(2, None, "epic")
        hop = int(0.02 * im.SR)
        rms = np.sqrt(np.convolve(np.mean(x ** 2, axis=1), np.ones(hop) / hop, mode="same"))[::hop]
        t = np.arange(len(rms)) * 0.02
        self.assertLess(abs(t[np.argmax(rms)] - (2 * im.CARD_SEC + tr.PASS_SEC)), 0.1)


class CardTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.spec = {"name": "Mu Gung", "cutout": figure(os.path.join(self.tmp, "cut.png")), "backdrop": painting(os.path.join(self.tmp, "bg.png")),
                     "drawn": True, "facing": "left"}

    def test_the_last_card_stays_and_its_profile_goes_first(self):
        spec = dict(self.spec, profile={"role": "Top new disciple", "bio": "A cocky trainee monk.", "traits": ["Smug", "Sharp", "Proud"],
                                        "facts": ["Known to the elders"]})
        c = im.Card(spec, W, H, (240, 180, 60), 0)
        mid = lambda img: np.asarray(img.convert("RGB"))[H // 3: 2 * H // 3, W // 3: 2 * W // 3].mean()
        # Any other card has slid out by its end; the last one is still there.
        self.assertLess(mid(c.frame(im.CARD_SEC - 0.001, leaves=True)), 5)
        self.assertGreater(mid(c.frame(im.CARD_SEC, leaves=False)), 30)
        # Its profile (and the shade behind it) is gone by then: what turns into light is the card
        # and the character alone, exactly as without a profile.
        self.assertEqual(c.layout(im.CARD_SEC, leaves=False)[7], 1.0)
        bare = im.Card(dict(self.spec, name=""), W, H, (240, 180, 60), 0)
        end = np.asarray(c.frame(im.CARD_SEC, leaves=False).convert("RGB")).astype(int)
        self.assertEqual(np.abs(end - np.asarray(bare.frame(im.CARD_SEC, leaves=False).convert("RGB")).astype(int)).max(), 0)
        # ...while for most of the card it is all there.
        self.assertGreater(np.abs(np.asarray(c.frame(1.5, leaves=False).convert("RGB")).astype(int)
                                  - np.asarray(bare.frame(1.5, leaves=False).convert("RGB")).astype(int)).max(), 100)
        m = np.asarray(c.surface(im.CARD_SEC), np.float32) / 255
        self.assertGreater(m.mean(), 0.2)

    def test_a_painted_background_mirrored_for_the_other_side(self):
        bg = np.asarray(Image.open(self.spec["backdrop"]).convert("RGB"))
        self.assertTrue(np.array_equal(np.asarray(im.backdrop_image(self.spec)), bg))
        flipped = np.asarray(im.backdrop_image(dict(self.spec, backdrop_flip=True)))
        self.assertTrue(np.array_equal(flipped, bg[:, ::-1]))
        # Without one, the manga's establishing panel.
        page = os.path.join(self.tmp, "page.png")
        Image.new("RGB", (100, 300), (10, 120, 30)).save(page)
        env = im.backdrop_image({"backdrop": None, "env": page, "env_crop": [0, 0, 1, 0.5]})
        self.assertEqual(env.size, (100, 150))

    def test_the_colours_come_from_the_cards_the_last_one_most(self):
        red = dict(self.spec, backdrop=painting(os.path.join(self.tmp, "red.png"), (200, 40, 40)),
                   cutout=figure(os.path.join(self.tmp, "red_cut.png"), (210, 50, 50)))
        blue = dict(self.spec, backdrop=painting(os.path.join(self.tmp, "blue.png"), (40, 70, 210)),
                    cutout=figure(os.path.join(self.tmp, "blue_cut.png"), (40, 80, 220)))
        hue = lambda hx: colorsys.rgb_to_hls(*(v / 255 for v in tr.hex_rgb(hx)))[0] * 360
        pal = im.card_palette([red, red, blue])
        self.assertTrue(200 <= hue(pal["primary"]) <= 250, pal)
        pal2 = im.card_palette([blue, blue, red])
        self.assertTrue(hue(pal2["primary"]) <= 20 or hue(pal2["primary"]) >= 340, pal2)
        self.assertEqual(im.accent_of(pal2, "epic"), tr.hex_rgb(pal2["accent"]))
        self.assertEqual(im.accent_of({}, "sad"), im.STYLE_COLORS["sad"][0])
        self.assertEqual(im.card_palette([]), {})


def wide_figure(path):
    """A standing figure with its arms out and a long flared coat (a kept drawing)."""
    im_ = Image.new("RGBA", (300, 400), (0, 0, 0, 0))
    d = ImageDraw.Draw(im_)
    d.ellipse((125, 5, 175, 60), fill=(235, 200, 170, 255))
    d.rectangle((110, 60, 190, 390), fill=(230, 230, 235, 255))
    d.rectangle((0, 120, 300, 150), fill=(230, 230, 235, 255))
    d.polygon([(110, 200), (190, 200), (262, 396), (38, 396)], fill=(230, 230, 235, 255))
    im_.save(path)
    return path


LONG = {"role": "Former Special Forces Soldier Turned Trainer",
        "bio": "An ex-special forces soldier who falls asleep reading a martial arts novel and wakes up inside it as a Shaolin novice.",
        "traits": ["Fitness-Obsessed", "Boisterous", "Confrontational"], "facts": ["Owns five gyms in Gangnam", "Hosts Ganghyeok Style Fitness"]}


def ink(img, x, y, level=200):
    """The box (frame or block coordinates) of the clearly drawn pixels of a part placed at (x, y)."""
    bb = img.getchannel("A").point(lambda v: 255 if v > level else 0).getbbox()
    return None if bb is None else (x + bb[0], y + bb[1], x + bb[2], y + bb[3])


class ProfileTests(unittest.TestCase):
    """The profile on each card: what it says (the app's profile, else one made from the
    description and traits), how it is laid out (in its column, lines broken at spaces only, smaller
    when it must be, the traits as evenly spaced chips), where it sits (low on the card's other
    side: never over the character, the card's edges or the frame's, at 1080p and at 720p) and how
    it comes and goes."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.bg = painting(os.path.join(self.tmp, "bg.png"))

    def test_without_a_profile_the_description_and_traits_stand_in(self):
        spec = {"name": "Mu Gung", "description": "Young man with a shaved head, orange robe; a fellow trainee monk",
                "traits": ["skeptical", "irritable", "smug", "provocative"], "profile": None}
        self.assertEqual(im.profile_of(spec), {"role": "Fellow trainee monk", "bio": "", "traits": ["Skeptical", "Irritable", "Smug"], "facts": []})
        self.assertEqual(im.role_from("Tall bald monk, yellow robe; appears to be a senior monk"), "Senior monk")
        self.assertEqual(im.role_from("Big man; seen only in close-up while yelling; likely a senior monk or instructor."), "Senior monk or instructor")
        self.assertEqual(im.role_from("Wild hair, stubble; Level 6 guild master of the Gray Dog guild"), "Level 6 guild master")
        self.assertEqual(im.role_from("Long dark hair; member of the Shinseong Guild escorted by bodyguards"), "Member of the Shinseong Guild")
        self.assertEqual(im.role_from("A girl in a red coat."), "")
        self.assertEqual(im.profile_of({"name": "X"}), {"role": "", "bio": "", "traits": [], "facts": []})
        self.assertEqual(im.title_case("self-critical"), "Self-Critical")
        # The app's profile is used as it is (tidied, at most three traits and two facts); what it
        # leaves out is filled in the same way.
        p = {"role": " Top  new disciple ", "bio": "A cocky trainee.", "traits": ["Smug", "Sharp", "Proud", "Extra"], "facts": ["One", "Two", "Three"]}
        self.assertEqual(im.profile_of(dict(spec, profile=p)),
                         {"role": "Top new disciple", "bio": "A cocky trainee.", "traits": ["Smug", "Sharp", "Proud"], "facts": ["One", "Two"]})
        self.assertEqual(im.profile_of(dict(spec, profile={"bio": "A cocky trainee."}))["role"], "Fellow trainee monk")
        # So a card without a profile still says who the character is (and never looks broken).
        cut = figure(os.path.join(self.tmp, "cut.png"))
        c = im.Card({"name": "Mu Gung", "cutout": cut, "backdrop": self.bg, "drawn": True, "facing": "left",
                     "description": spec["description"], "traits": spec["traits"]}, 1920, 1080, (240, 180, 60), 0)
        self.assertEqual([p[0] for p in c.info.parts], ["name", "bar", "role", "chip", "chip", "chip"])
        self.assertEqual(c.info.lines["role"], ["FELLOW TRAINEE MONK"])
        bare = im.Card({"name": "Mu Gung", "cutout": cut, "backdrop": self.bg, "drawn": True, "facing": "left"}, 1920, 1080, (240, 180, 60), 0)
        self.assertEqual([p[0] for p in bare.info.parts], ["name", "bar"])

    def test_long_text_wraps_or_shrinks_within_its_column(self):
        for h in (1080, 720):
            for share in (0.75, 0.5, 0.36, 0.25):
                mw, mh = share * h, 0.42 * h
                pr = im.Profile("Rodolphe Frankenstein von Hohenheim", LONG, (240, 180, 60), h, mw, mh)
                why = (h, share, pr.scale, pr.dropped)
                self.assertLessEqual(pr.width, mw + 0.5, why)
                self.assertLessEqual(pr.height, mh + 0.5, why)
                for kind, img, x, y, at in pr.parts:
                    box = ink(img, x, y)
                    self.assertGreaterEqual(box[0], -1, (kind, why))
                    self.assertLessEqual(box[2], pr.width + 1, (kind, why))
                    self.assertGreaterEqual(box[1], -2, (kind, why))
                # Lines break at spaces only: every word whole, in order, nothing lost.
                self.assertEqual(" ".join(pr.lines["name"]), "RODOLPHE FRANKENSTEIN VON HOHENHEIM")
                self.assertLessEqual(len(pr.lines["name"]), 2)
                if "bio" not in pr.dropped:
                    self.assertEqual(" ".join(pr.lines["bio"]), LONG["bio"])
                    self.assertLessEqual(len(pr.lines["bio"]), 3)
                if "role" not in pr.dropped:
                    self.assertEqual(" ".join(pr.lines["role"]), LONG["role"].upper())
            # With room enough, all of it at full size, the bio in two lines.
            pr = im.Profile("Ganghyeok Choi", LONG, (240, 180, 60), h, 0.75 * h, 0.42 * h)
            self.assertEqual((pr.scale, pr.dropped, len(pr.lines["name"]), len(pr.lines["bio"])), (1.0, (), 1, 2))
            # Very little room: smaller, then the least important parts left out, never overflowing.
            tight = im.Profile("Ganghyeok Choi", LONG, (240, 180, 60), h, 0.75 * h, 0.16 * h)
            self.assertIn("facts", tight.dropped)
            self.assertLessEqual(tight.height, 0.16 * h + 0.5)
        # A name too long for one line goes on two, broken between words; a single long word shrinks.
        two = im.Profile("Alexandrina Victoria Wettin Saxe-Coburg", {}, (240, 180, 60), 1080, 400, 400)
        self.assertEqual(len(two.lines["name"]), 2)
        one = im.Profile("Supercalifragilisticexpialidocious", {}, (240, 180, 60), 1080, 300, 400)
        self.assertEqual(one.lines["name"], ["SUPERCALIFRAGILISTICEXPIALIDOCIOUS"])
        self.assertLessEqual(one.width, 300.5)

    def test_the_bio_is_set_as_prose(self):
        width = lambda s: len(s) * 10.0
        for text in ("Built in a lab to be a destroyer, he only wants a normal human life of part-time jobs and ice cream.",
                     "Branded a heartless killer by the Fear squad's chief, he is really a cheerful crepe lover who wants human friends."):
            for mw in (450, 520, 600, 700):
                lines = im.wrap(text.split(), width, mw, 3, em=24)
                self.assertEqual(" ".join(lines), text)
                self.assertTrue(len(lines) <= 3 and all(width(ln) <= mw for ln in lines), (mw, lines))
                # No line ends on a little word ("a", "of", "he"...) when another break fits.
                self.assertFalse([ln for ln in lines[:-1] if ln.split()[-1].lower() in im.SHORT_END], (mw, lines))
        # As few lines as fit, as even as the words allow; None when it cannot fit.
        self.assertEqual(len(im.wrap("one two three four five six".split(), width, 300, 3)), 1)
        self.assertEqual(im.wrap("a b c".split(), width, 5, 3), None)
        self.assertEqual(im.wrap(["unbreakable"], width, 50, 3), None)

    def test_the_traits_are_evenly_spaced_chips_coming_in_one_by_one(self):
        for align in ("left", "right"):
            pr = im.Profile("Mu Gung", {"role": "Top new disciple", "traits": ["Smug", "Provocative", "Skeptical"]}, (240, 180, 60), 1080, 800, 500, align)
            chips = [(ink(img, x, y), at) for kind, img, x, y, at in pr.parts if kind == "chip"]
            self.assertEqual(len(chips), 3)
            boxes = [b for b, _ in chips]
            self.assertEqual(len({b[1] for b in boxes}), 1)                     # one row
            self.assertEqual(len({b[3] - b[1] for b in boxes}), 1)              # the same height
            gaps = [b[0] - a[2] for a, b in zip(boxes, boxes[1:])]
            self.assertLessEqual(max(gaps) - min(gaps), 1, gaps)                # evenly spaced
            self.assertGreater(min(gaps), 3)
            self.assertLess(boxes[0][2] - boxes[0][0], boxes[1][2] - boxes[1][0])  # sized to their words
            self.assertEqual(sorted(at for _, at in chips), [at for _, at in chips])
            self.assertEqual(len({at for _, at in chips}), 3)
            edge = boxes[0][0] if align == "left" else boxes[-1][2]
            self.assertLessEqual(abs(edge - (0 if align == "left" else pr.width)), 1.5)  # on the block's outer edge
            self.assertEqual(pr.lines["traits"], ["SMUG", "PROVOCATIVE", "SKEPTICAL"])

    def test_low_on_the_other_side_never_over_the_character_or_an_edge(self):
        cut = wide_figure(os.path.join(self.tmp, "wide.png"))
        for w, h in ((1920, 1080), (1280, 720)):
            for facing in ("left", "right"):
                spec = {"name": "Rodolphe Frankenstein", "cutout": cut, "backdrop": self.bg, "drawn": True, "facing": facing, "profile": LONG}
                c = im.Card(spec, w, h, (240, 180, 60), 1)
                ix, iy = c.info_xy
                box = (ix, iy, ix + c.info.width, iy + c.info.height + 0.01 * h)  # (and the last line's descenders)
                why = (w, h, facing)
                # On the side away from the character, in the card's lower part, inside the frame.
                self.assertEqual(c.info.align, "left" if c.right else "right")
                self.assertTrue(0 < box[0] and box[2] < w and 0 < box[1] and box[3] < h, why)
                self.assertGreater(box[3], c.top + 0.75 * c.ch, why)
                # Inside the tilted card, a clear margin from each of its edges.
                quad = [(qx + c.home, qy + c.top) for qx, qy in c.quad]
                cx_, cy_ = sum(p[0] for p in quad) / 4, sum(p[1] for p in quad) / 4
                for a, b in zip(quad, quad[1:] + quad[:1]):
                    nx, ny = b[1] - a[1], a[0] - b[0]
                    n = math.hypot(nx, ny)
                    sgn = 1 if (cx_ - a[0]) * nx + (cy_ - a[1]) * ny > 0 else -1
                    for px, py in ((box[0], box[1]), (box[2], box[1]), (box[2], box[3]), (box[0], box[3])):
                        self.assertGreater(sgn * ((px - a[0]) * nx + (py - a[1]) * ny) / n, 0.035 * h, (why, a, b))
                # Never over the character while it shows (the character drifts towards it).
                for t in (0.3, 0.6, 1.0, 1.6, 2.1, im.CARD_SEC - im.SLIDE_SEC):
                    x, y, fx, fy = c.layout(t)[:4]
                    m = Image.new("L", (w, h), 0)
                    m.paste(c.cut.getchannel("A"), (round(fx), round(fy)))
                    self.assertEqual(m.crop(tuple(round(v) for v in box)).point(lambda v: 255 if v > 24 else 0).getbbox(), None, (why, t))
                # At 720p it is the same layout scaled with the frame (whole pixels can make a line
                # break one word earlier, so the bio may take a line more).
                if h == 720:
                    big = im.Card(spec, 1920, 1080, (240, 180, 60), 1)
                    self.assertEqual((c.info.scale, c.info.dropped, c.info.align), (big.info.scale, big.info.dropped, big.info.align))
                    self.assertAlmostEqual((c.info_xy[1] + c.info.height) / 720, (big.info_xy[1] + big.info.height) / 1080, delta=0.003)
                    self.assertAlmostEqual(c.info.max_w / 720, big.info.max_w / 1080, delta=0.01)
                    self.assertAlmostEqual(c.info.height / 720, big.info.height / 1080, delta=0.05)
                    side = c.info_xy[0] if c.info.align == "left" else c.info_xy[0] + c.info.width
                    side_big = big.info_xy[0] if big.info.align == "left" else big.info_xy[0] + big.info.width
                    self.assertAlmostEqual(side / 720, side_big / 1080, delta=0.003)

    def test_it_builds_up_in_turn_and_reads_for_most_of_the_card(self):
        cut = figure(os.path.join(self.tmp, "cut.png"))
        c = im.Card({"name": "Mu Gung", "cutout": cut, "backdrop": self.bg, "drawn": True, "facing": "left", "profile": LONG}, 1920, 1080, (240, 180, 60), 0)
        parts = c.info.parts
        order = ["name", "bar", "role", "bio", "chip", "fact"]
        firsts = [min(at for k, _, _, _, at in parts if k == kind) for kind in order]
        self.assertEqual(firsts, sorted(firsts))
        # Not before the card has landed; all of it in within the first second.
        self.assertGreaterEqual(min(firsts), im.SLIDE_SEC * 0.75)
        everything = max(at for *_, at in parts) + im.INFO_IN
        self.assertLess(everything, 1.0)
        # All of it readable from then until the card starts to leave (most of the card).
        self.assertGreater((im.CARD_SEC - im.SLIDE_SEC) - everything, 1.15)
        self.assertGreater(im.CARD_SEC - im.SLIDE_SEC - (firsts[0] + im.INFO_IN), 0.6 * im.CARD_SEC)
        # It leaves with the card (riding out with it as it fades), and is gone when the card is.
        mid = im.CARD_SEC - im.SLIDE_SEC / 2
        x, _, _, _, _, ix, iy, gone = c.layout(mid, leaves=True)
        self.assertAlmostEqual(ix - c.info_xy[0], x - c.home, delta=1e-6)
        self.assertTrue(0 < gone < 1)
        self.assertEqual(c.layout(im.CARD_SEC, leaves=True)[7], 1.0)
        # The shade comes and goes with it.
        self.assertIsNotNone(c.scrim)
        self.assertEqual(c.info.shown(0.0), 0.0)
        self.assertEqual(c.info.shown(1.5), 1.0)


class RenderTests(unittest.TestCase):
    def test_only_the_cards_are_read_and_it_lands_on_the_recap(self):
        tmp = tempfile.mkdtemp()
        cover = painting(os.path.join(tmp, "cover.png"), (240, 230, 20), (300, 450))
        plate = painting(os.path.join(tmp, "plate.png"), (20, 230, 40))
        recap = os.path.join(tmp, "recap.mp4")
        subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", f"color=c=0x3060c0:s={W}x{H}:r=30",
                        "-t", "0.5", "-pix_fmt", "yuv420p", recap], check=True)
        cards = [{"name": "Mu Gung", "cutout": figure(os.path.join(tmp, "a.png")), "backdrop": painting(os.path.join(tmp, "a_bg.png")),
                  "drawn": True, "facing": "left", "profile": {"role": "Top new disciple", "bio": "A cocky trainee monk.",
                                                               "traits": ["Smug", "Sharp", "Proud"], "facts": ["Known to the elders"]}},
                 {"name": "Hye Jung", "cutout": figure(os.path.join(tmp, "b.png"), (230, 190, 40)),
                  "backdrop": painting(os.path.join(tmp, "b_bg.png"), (180, 90, 160)), "drawn": True, "facing": "right",
                  "description": "Tall bald monk; a senior monk", "traits": ["stern"]}]
        out = os.path.join(tmp, "intro.mp4")
        a = {"width": W, "height": H, "fps": "30/1", "style": "epic", "cards": cards, "reveal": recap, "music": None, "out": out,
             "work_dir": os.path.join(tmp, "work"), "progress_file": os.path.join(tmp, "progress.json"),
             # What an older app sent: none of it is used, none of it is even opened.
             "cover": cover, "plate": plate, "plan": {"palette": {"primary": "#00ff00"}}, "blender": os.path.join(tmp, "blender.exe"),
             "world_cache": tmp, "samples": 48}
        seen = []
        real_open, real_image_open, real_popen, real_run = builtins.open, Image.open, subprocess.Popen, subprocess.run

        def spy_open(file, *x, **k):
            seen.append(str(file))
            return real_open(file, *x, **k)

        def spy_image_open(fp, *x, **k):
            seen.append(str(fp))
            return real_image_open(fp, *x, **k)

        def spy_popen(cmd, *x, **k):
            seen.extend(str(c) for c in cmd)
            return real_popen(cmd, *x, **k)

        def spy_run(cmd, *x, **k):
            seen.extend(str(c) for c in cmd)
            return real_run(cmd, *x, **k)

        report = {"credits": [], "notes": []}
        with mock.patch("builtins.open", spy_open), mock.patch.object(Image, "open", spy_image_open), \
                mock.patch.object(subprocess, "Popen", spy_popen), mock.patch.object(subprocess, "run", spy_run):
            path, secs = im.render(a, report)
        self.assertNotIn(cover, seen)
        self.assertNotIn(plate, seen)
        self.assertTrue(any(s == cards[0]["backdrop"] for s in seen))  # (the spy does see what is read)
        self.assertEqual(path, out)
        self.assertAlmostEqual(secs, im.total_seconds(2))
        self.assertEqual(frame_count(out), int(round(im.total_seconds(2) * 30)))
        # It ends on the recap's first frame.
        self.assertLess(np.abs(last_frame(out, W, H).astype(int) - np.array([0x30, 0x60, 0xc0])).mean(), 12)
        self.assertEqual(report, {"credits": [], "notes": []})
        with open(a["progress_file"], encoding="utf-8") as f:
            self.assertEqual(json.load(f), {"p": 1.0, "step": "Saving the intro", "stage": "encode"})


if __name__ == "__main__":
    unittest.main()
