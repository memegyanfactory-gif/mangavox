"""The intro's cast drawings (intro_cast) and casting (intro_maker): the keyer leaves a clean
figure on every key colour, the key colour avoids the character's own colours, bad drawings are
caught, the Magnific request is built right, drawings are kept and never paid for twice, progress
is written for the app, and the final line reports credits even when the intro fails. No network."""
import base64
import io
import json
import os
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from unittest import mock

import numpy as np
from PIL import Image, ImageDraw

from python import intro_cast as ic

SKIN, ROBE, PANTS, INK = (240, 200, 170), (200, 120, 40), (70, 60, 55), (20, 15, 10)
BG = {"green": (8, 238, 12), "blue": (10, 14, 240), "magenta": (238, 10, 236)}


def figure(bg, size=(384, 688), box=(130, 70, 254, 640), eyes=None, extra=(), noise=2.0, s=3):
    """A drawn standing figure (head, robe, legs, an arm with a gap to the body, dark outlines) on a
    flat `bg`, antialiased, with a little noise like a model's flat colour. `box` is the figure's
    extent; `extra`: (shape, xy, fill) drawn too."""
    W, H = size
    x0, y0, x1, y1 = box
    fw, fh = x1 - x0, y1 - y0
    im = Image.new("RGB", (W * s, H * s), bg)
    d = ImageDraw.Draw(im)
    S = lambda *v: [int(round(c * s)) for c in v]
    ow = max(1, int(fw * 0.02 * s))
    cx = x0 + fw / 2
    d.ellipse(S(cx - fw * 0.2, y0, cx + fw * 0.2, y0 + fh * 0.16), fill=SKIN, outline=INK, width=ow)
    d.rectangle(S(cx - fw * 0.32, y0 + fh * 0.16, cx + fw * 0.32, y0 + fh * 0.58), fill=ROBE, outline=INK, width=ow)
    d.rectangle(S(cx - fw * 0.28, y0 + fh * 0.58, cx - fw * 0.04, y1), fill=PANTS, outline=INK, width=ow)
    d.rectangle(S(cx + fw * 0.04, y0 + fh * 0.58, cx + fw * 0.28, y1), fill=PANTS, outline=INK, width=ow)
    d.rectangle(S(x0, y0 + fh * 0.19, x0 + fw * 0.1, y0 + fh * 0.5), fill=SKIN, outline=INK, width=ow)  # an arm, a gap beside it
    d.rectangle(S(x0 + fw * 0.1, y0 + fh * 0.19, cx - fw * 0.32, y0 + fh * 0.23), fill=SKIN)
    if eyes:
        for ex in (cx - fw * 0.09, cx + fw * 0.05):
            d.ellipse(S(ex, y0 + fh * 0.06, ex + fw * 0.05, y0 + fh * 0.085), fill=eyes)
    for shape, xy, fill in extra:
        getattr(d, shape)(S(*xy), fill=fill)
    im = im.resize(size, Image.LANCZOS)
    a = np.asarray(im).astype(np.float32) + np.random.default_rng(1).normal(0, noise, (H, W, 3))
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))


def edge_tint(cut, colour):
    """The strongest key colour left in the semi-transparent edge, composited on black and on white."""
    a = np.asarray(cut).astype(np.float32) / 255
    al = a[..., 3:4]
    edge = (al[..., 0] > 0.02) & (al[..., 0] < 0.98)
    worst = 0.0
    for bg in (0.0, 1.0):
        comp = a[..., :3] * al + bg * (1 - al)
        worst = max(worst, float(ic.strength(comp[edge], colour).max()) if edge.any() else 0.0)
    return worst


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def png_bytes(img):
    buf = io.BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


class KeyerTests(unittest.TestCase):
    def test_each_key_colour_gives_a_clean_figure(self):
        for colour, bg in BG.items():
            cut, info = ic.key_out(figure(bg), colour)
            self.assertIsNotNone(cut, colour)
            self.assertIsNone(ic.check(info), (colour, info))
            a = np.asarray(cut)[..., 3]
            # Cropped to the figure with a small margin: the corners are clear, the robe solid.
            self.assertEqual(int(a[0, 0]), 0, colour)
            self.assertEqual(int(a[-1, -1]), 0, colour)
            self.assertGreater(a.mean(), 60, colour)
            mid = a[a.shape[0] // 3, a.shape[1] // 2]
            self.assertEqual(int(mid), 255, colour)
            self.assertLess(edge_tint(cut, colour), 0.06, colour)

    def test_the_gap_beside_the_arm_is_keyed_out(self):
        cut, info = ic.key_out(figure(BG["green"]), "green")
        x0, y0 = info["bbox"][0], info["bbox"][1]
        m = max(4, int(0.02 * (info["bbox"][3] - info["bbox"][1])))
        # In the source the gap is at x 130 + 124*0.12 .. 130 + 124*0.17, y 70 + 570*0.3.
        gx, gy = int(130 + 124 * 0.145) - x0 + m, int(70 + 570 * 0.35) - y0 + m
        self.assertLess(int(np.asarray(cut)[gy, gx, 3]), 40)

    def test_narrow_gaps_leave_no_key_colour(self):
        # Hair-thin slits of background inside the figure (between strands, fingers, legs): after
        # antialiasing none of their pixels is the pure background colour.
        slits = [("rectangle", (175 + k * 9, 180, 176.2 + k * 9, 330), BG["green"]) for k in range(4)]
        slits.append(("rectangle", (190, 560, 194, 640), BG["green"]))  # between the legs, open below
        cut, info = ic.key_out(figure(BG["green"], extra=slits), "green")
        self.assertIsNone(ic.check(info), info)
        a = np.asarray(cut).astype(np.float32) / 255
        seen = a[..., 3] > 0.02
        self.assertEqual(int((ic.strength(a[..., :3], "green")[seen] > 0.12).sum()), 0)
        self.assertLess(edge_tint(cut, "green"), 0.06)

    def test_specks_and_loose_islands_are_dropped(self):
        extra = [("ellipse", (20, 40, 26, 46), INK), ("rectangle", (300, 300, 330, 320), ROBE)]
        cut, info = ic.key_out(figure(BG["green"], extra=extra), "green")
        self.assertIsNone(ic.check(info), info)
        # The cut is the figure alone: the far blob and the speck are not in it.
        self.assertLessEqual(cut.width, 124 + 2 * 14)

    def test_key_coloured_eyes_inside_the_figure_are_given_back(self):
        cut, info = ic.key_out(figure(BG["green"], eyes=(40, 160, 60)), "green")
        self.assertIsNone(ic.check(info), info)
        a = np.asarray(cut)
        x0, y0 = info["bbox"][0], info["bbox"][1]
        m = max(4, int(0.02 * (info["bbox"][3] - info["bbox"][1])))
        ex, ey = int(192 - 124 * 0.09 + 124 * 0.025) - x0 + m, int(70 + 570 * 0.0725) - y0 + m
        self.assertEqual(int(a[ey, ex, 3]), 255)
        self.assertGreater(int(a[ey, ex, 1]) - int(a[ey, ex, 0]), 50)  # still green

    def test_background_read_from_the_border_not_the_exact_value(self):
        a = np.asarray(figure((30, 200, 50)), dtype=np.float32) / 255
        bg = ic.background(a, "green")
        self.assertTrue(np.allclose(bg, np.array([30, 200, 50]) / 255, atol=0.03))
        self.assertIsNone(ic.background(np.asarray(figure((128, 128, 128)), dtype=np.float32) / 255, "green"))


class CheckTests(unittest.TestCase):
    def reason(self, img, colour="green"):
        cut, info = ic.key_out(img, colour)
        return ic.check(info)

    def test_cut_off_feet(self):
        self.assertEqual(self.reason(figure(BG["green"], box=(130, 120, 254, 700)))[1], "cut")

    def test_wider_than_tall(self):
        self.assertEqual(self.reason(figure(BG["green"], size=(688, 384), box=(60, 40, 620, 350)))[1], "shape")

    def test_two_figures(self):
        two = figure(BG["green"], box=(30, 70, 150, 640))
        second = figure(BG["green"], box=(230, 70, 350, 640))
        a = np.asarray(two).copy()
        b = np.asarray(second)
        a[:, 192:] = b[:, 192:]
        self.assertEqual(self.reason(Image.fromarray(a))[1], "figures")

    def test_too_small(self):
        self.assertEqual(self.reason(figure(BG["green"], box=(170, 300, 214, 560)))[1], "small")

    def test_not_on_the_key_colour(self):
        self.assertEqual(self.reason(figure((240, 240, 240)))[1], "background")

    def test_key_colour_on_the_character(self):
        # A green sash across the robe, out to the figure's edge: the key eats into the character.
        sash = [("rectangle", (151, 250, 233, 300), (60, 190, 70))]
        self.assertEqual(self.reason(figure(BG["green"], extra=sash))[1], "leak")


class ColourTests(unittest.TestCase):
    def test_key_colour_is_farthest_from_the_characters_hues(self):
        green = Image.new("RGB", (64, 64), (40, 200, 60))
        blue = Image.new("RGB", (64, 64), (30, 60, 220))
        pink = Image.new("RGB", (64, 64), (230, 40, 200))
        neutral = Image.new("RGB", (64, 64), (200, 120, 40))
        self.assertNotEqual(ic.choose_colours([green])[0][0], "green")
        self.assertEqual(ic.choose_colours([blue])[0][0], "green")
        self.assertEqual(ic.choose_colours([pink])[0][0], "green")
        self.assertEqual(ic.choose_colours([neutral])[0][0], "green")  # green when in doubt
        self.assertEqual(ic.choose_colours([green, blue])[0][0], "magenta")
        self.assertNotEqual(ic.choose_colours([neutral], "a boy with green eyes")[0][0], "green")
        self.assertEqual(sorted(ic.choose_colours([green])[0]), ["blue", "green", "magenta"])

    def test_black_and_white_references(self):
        grey = Image.fromarray(np.random.default_rng(2).integers(0, 255, (64, 64), dtype=np.uint8)).convert("RGB")
        self.assertTrue(ic.monochrome([grey]))
        self.assertFalse(ic.monochrome([Image.new("RGB", (64, 64), (200, 120, 40))]))


class PromptTests(unittest.TestCase):
    def test_prompt(self):
        p = ic.build_prompt("Mu Gung", "Young monk, orange robe, blood-spattered sleeves, blood on his cheek, yellow eyes",
                            ["body", "body", "face"], "magenta", "left", fixes=["cut"])
        for words in ("Images 1 and 2 show Mu Gung", "Image 3 is a close-up of Mu Gung's face", "full-body", "top of the head",
                      "soles of both feet", "pure magenta (#FF00FF)", "towards the left side", "the images win", "Safe for work",
                      "No gradient", "no cast shadow", "no text", "leave a gap above the head"):
            self.assertIn(words, p)
        self.assertNotIn("blood", p.split("Safe for work")[0])
        self.assertIn("anime adaptation", ic.build_prompt("A", "", ["face"], "green", "right", mono=True))


class RequestTests(unittest.TestCase):
    def test_body(self):
        b = ic.request_body("draw", [("QUJD", "Image 1: Mu Gung, full body in the manga")])
        self.assertEqual((b["aspect_ratio"], b["resolution"], b["thinking_level"]), ("9:16", "1K", "high"))
        self.assertEqual(b["reference_images"], [{"image": "QUJD", "mime_type": "image/jpeg", "text": "Image 1: Mu Gung, full body in the manga"}])
        self.assertNotIn("reference_images", ic.request_body("draw", []))

    def test_jpeg_is_bounded(self):
        big = Image.new("RGB", (3000, 1000), (10, 20, 30))
        small = Image.new("RGB", (100, 80), (10, 20, 30))
        self.assertEqual(max(Image.open(io.BytesIO(base64.b64decode(ic.jpeg_b64(big)))).size), 1536)
        self.assertEqual(max(Image.open(io.BytesIO(base64.b64decode(ic.jpeg_b64(small)))).size), 512)

    def test_client_polls_until_completed(self):
        calls = []

        class Fake(ic.Client):
            def call(self, method, url, body=None):
                calls.append((method, url))
                if method == "POST":
                    return {"data": {"task_id": "t1", "status": "CREATED"}}
                return {"data": {"status": "COMPLETED" if len(calls) > 2 else "IN_PROGRESS", "generated": [{"url": "https://x/img.png"}]}}

            def fetch(self, url):
                return b"PNG:" + url.encode()

        c = Fake("secret-key", poll=0)
        self.assertEqual(c.generate("p", []), b"PNG:https://x/img.png")
        self.assertEqual(calls[0], ("POST", ic.API))
        self.assertEqual(calls[1], ("GET", ic.API + "/t1"))
        self.assertNotIn("secret-key", repr(c))

    def test_errors(self):
        self.assertEqual(ic._error(401, b'{"message": "bad"}').kind, "key")
        self.assertTrue(ic._error(402, b"").fatal and ic._error(403, b"").fatal)
        self.assertFalse(ic._error(429, b"").fatal)

        class Failing(ic.Client):
            def call(self, method, url, body=None):
                return {"data": {"task_id": "t"}} if method == "POST" else {"data": {"status": "FAILED"}}

        with self.assertRaises(ic.MagnificError) as e:
            Failing("k", poll=0).generate("p", [])
        self.assertEqual(e.exception.kind, "filter")

    def test_key_from_studio_then_environment(self):
        d = tempfile.mkdtemp()
        with mock.patch.dict(os.environ, {"MAGNIFIC_API_KEY": "env-key"}):
            self.assertEqual(ic.api_key(d), "env-key")
            with open(os.path.join(d, "magnific.json"), "w") as f:
                json.dump({"api_key": " saved "}, f)
            self.assertEqual(ic.api_key(d), "saved")
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertIsNone(ic.api_key(None))


class ProgressTests(unittest.TestCase):
    def test_written_atomically_on_each_step_and_throttled(self):
        d = tempfile.mkdtemp()
        path = os.path.join(d, "progress.json")
        p = ic.Progress(path, gap=10.0)
        p.set(0.1, "Choosing the cast", "cast")
        self.assertEqual(load_json(path), {"p": 0.1, "step": "Choosing the cast", "stage": "cast"})
        p.set(0.2)  # within the gap: not written
        self.assertEqual(load_json(path)["p"], 0.1)
        p.set(0.3, "Drawing Mu Gung with Magnific (1 of 4)", "draw")  # a new step: written at once
        self.assertEqual(load_json(path)["step"], "Drawing Mu Gung with Magnific (1 of 4)")
        p.set(0.25, "Rendering the transition (frame 2 of 9)", "transition", minor=True)  # a counter: throttled
        self.assertEqual(load_json(path)["stage"], "draw")
        p.flush()
        v = load_json(path)
        self.assertEqual((v["p"], v["stage"]), (0.3, "transition"))  # p never goes back
        self.assertEqual(sorted(os.listdir(d)), ["progress.json"])  # no .tmp left behind
        part = p.part(0.5, 1.0)
        part(0.5, "Saving the intro", "encode")
        self.assertEqual(load_json(path)["p"], 0.75)
        quiet = ic.Progress(None)
        quiet.set(0.5, "x", "cast")
        self.assertEqual(quiet.writes, 0)


class FakeDrawings(ic.Client):
    """Magnific without the network: each task gives the next of `images` (PIL) or raises."""

    def __init__(self, images):
        super().__init__("test-key", poll=0)
        self.images = list(images)
        self.posts = 0

    def call(self, method, url, body=None):
        if method == "POST":
            self.posts += 1
            nxt = self.images.pop(0)
            if isinstance(nxt, ic.MagnificError):
                raise nxt
            self.current = nxt
            return {"data": {"task_id": f"t{self.posts}"}}
        return {"data": {"status": "COMPLETED", "generated": ["https://x/" + url.rsplit("/", 1)[-1]]}}

    def fetch(self, url):
        return png_bytes(self.current)


def a_card(tmp, name="Mu Gung", cid="mu-gung-1"):
    face = os.path.join(tmp, f"{cid}_face.png")
    Image.new("RGB", (160, 160), (230, 190, 160)).save(face)
    return {"name": name, "id": cid, "description": "Young monk in an orange robe", "faces": [face], "shots": []}


def drawer_args(tmp, **kw):
    studio = os.path.join(tmp, "studio")
    os.makedirs(studio, exist_ok=True)
    with open(os.path.join(studio, "magnific.json"), "w") as f:
        json.dump({"api_key": "test-key"}, f)
    a = {"studio_dir": studio, "intro_dir": os.path.join(tmp, "intro"), "work_dir": os.path.join(tmp, "work")}
    a.update(kw)
    return a


class DrawerTests(unittest.TestCase):
    def test_a_good_drawing_is_kept_and_never_paid_twice(self):
        tmp = tempfile.mkdtemp()
        fake = FakeDrawings([figure(BG["green"])])
        d = ic.Drawer(drawer_args(tmp), client=fake)
        card = a_card(tmp)
        d.draw([card])
        self.assertTrue(card.get("drawn"), card.get("draw_fail"))
        self.assertEqual(d.credits, [{"model": "nano-banana-2-1", "credits": 50.0}])
        kept = os.path.join(tmp, "intro", "cast", "mu-gung-1.png")
        self.assertEqual(card["cutout"], kept)
        meta = load_json(kept[:-4] + ".json")
        self.assertEqual((meta["key_colour"], meta["model"], meta["credits"], meta["facing"]), ("green", "nano-banana-2-1", 50.0, "left"))
        with Image.open(kept) as im:
            self.assertEqual(im.mode, "RGBA")
        # The next export: the kept drawing, nothing paid, no request made.
        fake2 = FakeDrawings([])
        d2 = ic.Drawer(drawer_args(tmp), client=fake2)
        card2 = a_card(tmp)
        d2.draw([card2])
        self.assertEqual((card2["cutout"], card2["drawn"], d2.credits, fake2.posts), (kept, True, [], 0))

    def test_the_steps_say_what_is_happening(self):
        class Recorder(ic.Progress):
            def __init__(self):
                super().__init__(None)
                self.steps = []

            def set(self, p=None, step=None, stage=None, minor=False):
                if step and (not self.steps or self.steps[-1] != (step, stage)):
                    self.steps.append((step, stage))
                super().set(p, step, stage, minor)

        tmp = tempfile.mkdtemp()
        cut_off = figure(BG["green"], box=(130, 120, 254, 700))
        d = ic.Drawer(drawer_args(tmp), client=FakeDrawings([cut_off, figure(BG["green"])]))
        rec = Recorder()
        d.draw([a_card(tmp)], rec, (0.1, 0.5))
        self.assertEqual(rec.steps, [("Drawing Mu Gung with Magnific (1 of 1)", "draw"), ("Keying out Mu Gung's background", "key"),
                                     ("Drawing Mu Gung again: the figure is cut off at the feet", "draw"),
                                     ("Drawing Mu Gung with Magnific (1 of 1, second try)", "draw"), ("Keying out Mu Gung's background", "key"),
                                     ("Mu Gung is drawn and keyed (1 of 1 ready)", "key")])
        self.assertTrue(0.1 <= rec.p <= 0.5)
        rec2 = Recorder()
        ic.Drawer(drawer_args(tmp), client=FakeDrawings([])).draw([a_card(tmp)], rec2)
        self.assertEqual(rec2.steps, [("Using the kept drawing of Mu Gung", "draw")])

    def test_bad_drawings_are_redrawn_once_then_fall_back(self):
        tmp = tempfile.mkdtemp()
        cut_off = figure(BG["green"], box=(130, 120, 254, 700))
        fake = FakeDrawings([cut_off, cut_off, cut_off])
        d = ic.Drawer(drawer_args(tmp), client=fake)
        card = a_card(tmp)
        d.draw([card])
        self.assertEqual(fake.posts, 2)  # never more than two drawings per character
        self.assertNotIn("cutout", card)
        self.assertIn("cut off at the feet", card["draw_fail"])
        self.assertEqual(len(d.credits), 2)
        self.assertEqual(load_json(os.path.join(tmp, "intro", "cast", "mu-gung-1.json"))["failed_exports"], 1)

    def test_no_credits_stops_drawing(self):
        tmp = tempfile.mkdtemp()
        fake = FakeDrawings([ic.MagnificError("credits", "no credits")] * 4)
        d = ic.Drawer(drawer_args(tmp), client=fake)
        cards = [a_card(tmp, "Mu Gung", "a"), a_card(tmp, "Mu Yul", "b")]
        d.draw(cards)
        self.assertEqual(d.credits, [])
        self.assertTrue(all("Magnific refused" in c["draw_fail"] for c in cards))
        self.assertLessEqual(fake.posts, 2)

    def test_without_a_key_or_with_drawing_off_nothing_is_drawn(self):
        tmp = tempfile.mkdtemp()
        with mock.patch.dict(os.environ, {}, clear=True):
            a = {"studio_dir": os.path.join(tmp, "none"), "intro_dir": os.path.join(tmp, "intro"), "work_dir": tmp}
            self.assertFalse(ic.Drawer(a).can)
            self.assertFalse(ic.Drawer(drawer_args(tmp, cast_draw=False), client=FakeDrawings([])).can)
            self.assertFalse(ic.Drawer(drawer_args(tmp, intro_dir=None), client=FakeDrawings([])).can)

    def test_a_png_dropped_by_hand_is_used_as_is(self):
        tmp = tempfile.mkdtemp()
        folder = os.path.join(tmp, "intro", "cast")
        os.makedirs(folder)
        hand = Image.new("RGBA", (100, 200), (0, 0, 0, 0))
        ImageDraw.Draw(hand).rectangle((20, 10, 80, 190), fill=(200, 120, 40, 255))
        hand.save(os.path.join(folder, "mu-gung-1.png"))
        card = a_card(tmp)
        ic.Drawer(drawer_args(tmp), client=FakeDrawings([])).draw([card])
        self.assertEqual(card["cutout"], os.path.join(folder, "mu-gung-1.png"))


def painted(size=(1376, 768), seed=4, shapes=14, top=1 / 3):
    """A painted 16:9 background: a sky over ground, shapes in it (from `top` down), a painted
    texture. A manga page when tall, with shapes all over it."""
    w, h = size
    rng = np.random.default_rng(seed)
    yy = np.mgrid[0:h, 0:w][0] / h
    a = np.stack([150 + 80 * yy, 110 + 60 * yy, 160 - 60 * yy], -1)
    im = Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))
    d = ImageDraw.Draw(im)
    for _ in range(shapes):
        x, y = int(rng.integers(0, w)), int(rng.integers(int(h * top), h))
        d.rectangle((x, y, x + int(rng.integers(30, 220)), y + int(rng.integers(20, 160))), fill=tuple(int(v) for v in rng.integers(30, 220, 3)))
    a = np.asarray(im).astype(np.float32) + rng.normal(0, 7, (h, w, 3))
    return Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))


def lettering(size=(1376, 768)):
    """A page of text: black letters on white paper."""
    im = Image.new("RGB", size, (250, 250, 250))
    d = ImageDraw.Draw(im)
    for row in range(40, size[1] - 40, 36):
        for col in range(40, size[0] - 60, 24):
            d.rectangle((col, row, col + 12, row + 20), fill=(15, 15, 15))
    return im


class RecordingDrawings(FakeDrawings):
    """FakeDrawings that keeps the request bodies."""

    def __init__(self, images):
        super().__init__(images)
        self.bodies = []

    def call(self, method, url, body=None):
        if method == "POST":
            self.bodies.append(body)
        return super().call(method, url, body)


def a_place(tmp):
    """(card, pages): a character with a body shot in a temple courtyard, and the pages read around
    it: the courtyard again without anyone, a panel of balloons, and a close-up."""
    page = os.path.join(tmp, "page.png")
    painted((800, 2400), seed=5, shapes=80, top=0).save(page)
    other = os.path.join(tmp, "other.png")
    painted((800, 2400), seed=6, shapes=80, top=0).save(other)
    pages = {
        "0:4": {"image": page, "summary": "In the training courtyard, Mu Gung faces the young monk at sunset.",
                "scene": {"kind": "action", "mood": "tense", "tags": ["shaolin temple", "match"]},
                "panels": [{"bbox": [0, 0, 1, 0.3], "description": "Low-angle wide shot of a temple courtyard at sunset. Mu Gung grins, fists clasped.",
                            "people": 2, "talk": 0, "names": ["Mu Gung", "Mujin"]},
                           {"bbox": [0, 0.3, 1, 0.42], "description": "Large speech balloons over white space.", "people": 0, "talk": 2, "names": []},
                           {"bbox": [0, 0.42, 1, 0.7], "description": "Close-up of Mu Gung's grin against a red wall.", "people": 1, "talk": 1,
                            "names": ["Mu Gung"]}]},
        "0:5": {"image": other, "summary": "The courtyard is quiet.", "scene": {"kind": "calm", "mood": "serene", "tags": ["temple courtyard"]},
                "panels": [{"bbox": [0, 0.1, 1, 0.45], "description": "Establishing shot of the empty temple courtyard with a red hall under a pink "
                                                                        "evening sky, lanterns glowing.", "people": 0, "talk": 0, "names": []}]},
        "3:40": {"image": other, "summary": "", "scene": {}, "panels": [{"bbox": [0, 0, 1, 0.5], "description": "A city street at night.",
                                                                        "people": 0, "talk": 0, "names": []}]},
    }
    cut = os.path.join(tmp, "mu.png")
    Image.new("RGBA", (100, 300), (220, 120, 40, 255)).save(cut)
    card = {"name": "Mu Gung", "id": "mu-gung-1", "cutout": cut, "side": "left", "env": other, "env_crop": [0, 0.1, 1, 0.45],
            "shots": [{"image": page, "panel": [0, 0, 1, 0.3], "page": "0:4", "score": 3.0, "description": pages["0:4"]["panels"][0]["description"],
                       "box": [100, 50, 500, 700]}]}
    return card, pages


class BackdropTests(unittest.TestCase):
    def test_references_start_from_the_drawings_shot_then_the_place_around_it(self):
        tmp = tempfile.mkdtemp()
        card, pages = a_place(tmp)
        refs = ic.backdrop_references(card, pages)
        self.assertEqual([r[2] for r in refs], ["with", "place"])  # no balloons, no close-up, no other place
        self.assertEqual((refs[0][3]["page"], refs[1][3]["page"]), ("0:4", "0:5"))
        self.assertTrue(refs[0][1].startswith("Image 1: a panel of the manga showing Mu Gung"))
        self.assertEqual(refs[0][0].size[0], 800)  # the whole panel, not the character's crop
        notes = ic.place_notes(refs, pages)
        self.assertIn("temple courtyard", notes["place"])
        self.assertFalse(any("Mu Gung" in p for p in notes["place"]))
        self.assertEqual(notes["time"], "sunset")
        self.assertIn("lanterns", notes["light"])
        self.assertEqual(notes["mood"], "tense")
        self.assertIn("shaolin temple", notes["tags"])
        # A character without a body shot has nowhere to be.
        self.assertEqual(ic.backdrop_references(dict(card, shots=[]), pages), [])

    def test_place_phrases(self):
        self.assertEqual(ic.place_phrases("Low-angle wide shot of a temple courtyard at sunset."), ["temple courtyard"])
        self.assertEqual(ic.place_phrases("He lies on the wooden floor of his room at night."), ["wooden floor of the room"])
        self.assertEqual(ic.place_phrases("The tall Hye Jung towers over the monks."), [])

    def test_prompt(self):
        p = ic.backdrop_prompt("Mu Gung", ["with", "place", "place"], {"place": ["temple courtyard", "red temple hall"], "seen": [],
                                                                      "time": "sunset", "weather": ["mist"], "light": ["lanterns"],
                                                                      "mood": "tense", "tags": ["shaolin temple"]},
                               "left", ["orange", "black"], fixes=["text"])
        for words in ("Image 1 is a panel of the manga showing Mu Gung", "Images 2 and 3 show the same place", "manga's own art style",
                      "The place: temple courtyard; red temple hall.", "time of day: sunset", "weather: mist", "light: lanterns",
                      "completely empty of people", "no silhouettes", "leave Mu Gung and everyone else out", "No text anywhere", "speech bubbles",
                      "watermarks", "The left third of the picture is calmer", "on the right side", "depth", "atmospheric",
                      "Mu Gung wears orange and black", "Important: no text"):
            self.assertIn(words, p)
        self.assertIn("anime adaptation", ic.backdrop_prompt("A", ["with"], {}, "right", mono=True))

    def test_checks(self):
        self.assertIsNone(ic.check_backdrop(painted()))
        self.assertEqual(ic.check_backdrop(painted((768, 1376)))[1], "wide")
        self.assertEqual(ic.check_backdrop(Image.new("RGB", (1376, 768), (90, 90, 90)))[1], "detail")
        self.assertEqual(ic.check_backdrop(lettering())[1], "text")

    def test_a_good_background_is_16_9_kept_and_never_paid_twice(self):
        tmp = tempfile.mkdtemp()
        card, pages = a_place(tmp)
        fake = RecordingDrawings([painted()])
        d = ic.Drawer(drawer_args(tmp), client=fake)
        d.paint([card], pages)
        kept = os.path.join(tmp, "intro", "cast", "mu-gung-1.bg.png")
        self.assertEqual((card.get("backdrop"), card.get("backdrop_flip")), (kept, False))
        self.assertEqual(d.credits, [{"model": "nano-banana-2-1", "credits": 50.0}])
        body = fake.bodies[0]
        self.assertEqual((body["aspect_ratio"], body["resolution"]), ("16:9", "1K"))
        self.assertEqual(len(body["reference_images"]), 2)
        self.assertIn("completely empty of people", body["prompt"])
        meta = load_json(kept[:-4] + ".json")
        self.assertEqual((meta["name"], meta["model"], meta["prompt_version"], meta["credits"], meta["side"]),
                         ("Mu Gung", "nano-banana-2-1", ic.BG_PROMPT_VERSION, 50.0, "left"))
        self.assertTrue(meta["made_at"] > 0 and len(meta["references"]) == 2 and meta["references"][0]["kind"] == "with")
        # The next export: the kept one, nothing paid, no request; mirrored when the card's sides swapped.
        again = RecordingDrawings([])
        card2, _ = a_place(tmp)
        card2["side"] = "right"
        d2 = ic.Drawer(drawer_args(tmp), client=again)
        d2.paint([card2], pages)
        self.assertEqual((card2["backdrop"], card2["backdrop_flip"], d2.credits, again.posts), (kept, True, [], 0))

    def test_the_steps_say_what_is_happening(self):
        class Recorder(ic.Progress):
            def __init__(self):
                super().__init__(None)
                self.steps = []

            def set(self, p=None, step=None, stage=None, minor=False):
                if step and (not self.steps or self.steps[-1] != (step, stage)):
                    self.steps.append((step, stage))
                super().set(p, step, stage, minor)

        tmp = tempfile.mkdtemp()
        card, pages = a_place(tmp)
        rec = Recorder()
        ic.Drawer(drawer_args(tmp), client=FakeDrawings([lettering(), painted()])).paint([card], pages, rec, (0.5, 0.8))
        self.assertEqual(rec.steps, [("Painting Mu Gung's background with Magnific (1 of 1)", "backdrop"),
                                     ("Painting Mu Gung's background again: it is mostly lettering or blank paper", "backdrop"),
                                     ("Painting Mu Gung's background with Magnific (1 of 1, second try)", "backdrop"),
                                     ("Mu Gung's background is painted (1 of 1 ready)", "backdrop")])
        self.assertTrue(0.5 <= rec.p <= 0.8)
        rec2 = Recorder()
        card2, _ = a_place(tmp)
        ic.Drawer(drawer_args(tmp), client=FakeDrawings([])).paint([card2], pages, rec2)
        self.assertEqual(rec2.steps, [("Using the kept background of Mu Gung", "backdrop")])

    def test_bad_backgrounds_are_painted_once_more_then_the_panel_stays(self):
        tmp = tempfile.mkdtemp()
        card, pages = a_place(tmp)
        flat = Image.new("RGB", (1376, 768), (60, 60, 60))
        fake = FakeDrawings([flat, flat, flat])
        d = ic.Drawer(drawer_args(tmp), client=fake)
        d.paint([card], pages)
        self.assertEqual(fake.posts, 2)  # never more than two per character per export
        self.assertNotIn("backdrop", card)
        self.assertIn("blank or flat", card["backdrop_fail"])
        self.assertEqual(len(d.credits), 2)
        self.assertEqual(load_json(os.path.join(tmp, "intro", "cast", "mu-gung-1.bg.json"))["failed_exports"], 1)
        # The card shows the manga's establishing panel instead.
        from python import intro_maker
        self.assertEqual(intro_maker.backdrop_image(card).size, (800, 840))

    def test_a_background_dropped_by_hand_is_used_as_is(self):
        tmp = tempfile.mkdtemp()
        folder = os.path.join(tmp, "intro", "cast")
        os.makedirs(folder)
        painted((640, 360)).save(os.path.join(folder, "mu-gung-1.bg.png"))
        card, pages = a_place(tmp)
        card["side"] = "right"
        fake = FakeDrawings([])
        ic.Drawer(drawer_args(tmp), client=fake).paint([card], pages)
        self.assertEqual((card["backdrop"], card["backdrop_flip"], fake.posts), (os.path.join(folder, "mu-gung-1.bg.png"), False, 0))

    def test_drawing_off_uses_kept_files_only(self):
        tmp = tempfile.mkdtemp()
        card, pages = a_place(tmp)
        with mock.patch.object(ic.Client, "call", side_effect=AssertionError("no request may be made")):
            d = ic.Drawer(drawer_args(tmp, cast_draw=False))
            self.assertFalse(d.can)
            d.paint([card], pages)
            self.assertNotIn("backdrop", card)
            self.assertEqual(d.credits, [])
            folder = os.path.join(tmp, "intro", "cast")
            os.makedirs(folder)
            painted((640, 360)).save(os.path.join(folder, "mu-gung-1.bg.png"))
            ic.Drawer(drawer_args(tmp, cast_draw=False)).paint([card], pages)
            self.assertEqual(card["backdrop"], os.path.join(folder, "mu-gung-1.bg.png"))

    def test_no_credits_stops_painting(self):
        tmp = tempfile.mkdtemp()
        cards = []
        for cid in ("a", "b"):
            c, pages = a_place(tmp)
            c["id"] = cid
            cards.append(c)
        fake = FakeDrawings([ic.MagnificError("credits", "no credits")] * 4)
        d = ic.Drawer(drawer_args(tmp), client=fake)
        d.paint(cards, pages)
        self.assertEqual(d.credits, [])
        self.assertTrue(all("Magnific refused" in c["backdrop_fail"] for c in cards))
        self.assertLessEqual(fake.posts, 2)


class CastingTests(unittest.TestCase):
    def setUp(self):
        from python import intro_maker
        self.im = intro_maker
        self.tmp = tempfile.mkdtemp()
        self.page = os.path.join(self.tmp, "page.png")
        Image.new("RGB", (800, 2400), (240, 240, 240)).save(self.page)

    def read(self, panels):
        path = os.path.join(self.tmp, f"read_{time.time_ns()}.json")
        with open(path, "w") as f:
            json.dump({"pages": [{"index": 0, "panels": panels}]}, f)
        return {"read": path, "pages": [self.page]}

    def panel(self, name, desc, y=0.0):
        return {"bbox": [0, y, 1, y + 0.3], "description": desc, "characters": [{"name": name, "face": [0.4, y + 0.02, 0.5, y + 0.05]}]}

    def test_own_name_first_wrong_sex_dropped_faces_only_when_drawable(self):
        face = os.path.join(self.tmp, "f.png")
        Image.new("RGB", (100, 100)).save(face)
        cast = [{"name": "Kosuke Jojima", "aliases": ["Light-haired young man"], "id": "kosuke", "description": "Young man with light hair", "faces": [face]},
                {"name": "Arin Joo", "aliases": [], "id": "arin", "description": "Schoolgirl with a ponytail", "faces": [face]}]
        ch = self.read([self.panel("Light-haired young man", "A light-haired young man in a track jacket stands alone.", 0.0),
                        self.panel("Kosuke Jojima", "Kosuke walks alone in a suit, his hands in his pockets.", 0.35),
                        self.panel("kosuke jojima", "A girl in a plaid skirt wrings her hands.", 0.7)])
        cards = self.im.cast_cards({"chapters": [ch], "cast": cast}, drawable=lambda c: c["id"] == "arin")
        self.assertEqual([c["name"] for c in cards], ["Kosuke Jojima", "Arin Joo"])
        shots = cards[0]["shots"]
        self.assertEqual(len(shots), 2)  # the girl named "kosuke jojima" is someone else
        self.assertTrue(shots[0]["own_name"])  # his own name first, then the alias
        self.assertFalse(shots[1]["own_name"])
        self.assertEqual(cards[1]["shots"], [])  # faces only: shown because it can be drawn
        none = self.im.cast_cards({"chapters": [ch], "cast": cast}, drawable=lambda c: False)
        self.assertEqual([c["name"] for c in none], ["Kosuke Jojima"])

    def test_a_card_with_no_picture_gives_way_to_a_reserve(self):
        notes = []
        cards = [{"name": f"C{i}", "shots": [{"x": 1}]} for i in range(3)] + [{"name": "Faces only", "shots": [], "draw_fail": "Magnific refused"},
                                                                              {"name": "Reserve", "shots": [{"x": 1}], "reserve": True}]
        final = self.im.choose_cards(cards, None, notes)
        self.assertEqual([c["name"] for c in final], ["C0", "C1", "C2", "Reserve"])
        self.assertTrue(notes and notes[0].startswith("Faces only: Magnific refused"))


class ResultTests(unittest.TestCase):
    def test_credits_are_reported_when_the_intro_fails(self):
        from python import intro_maker

        def failing(a, report):
            report["credits"].append({"model": "nano-banana-2-1", "credits": 50.0})
            report["notes"].append("Mu Yul: Magnific refused (not enough credits), cut from a panel instead")
            raise RuntimeError("ffmpeg: broken")

        args = os.path.join(tempfile.mkdtemp(), "args.json")
        with open(args, "w") as f:
            json.dump({}, f)
        out = io.StringIO()
        with mock.patch.object(intro_maker, "render", failing), mock.patch.object(sys, "argv", ["intro_maker.py", args]), \
                redirect_stdout(out), mock.patch.object(sys, "stderr", io.StringIO()):
            intro_maker.main()
        v = json.loads(out.getvalue().strip().splitlines()[-1])
        self.assertEqual(v["ok"], False)
        self.assertEqual(v["message"], "ffmpeg: broken")
        self.assertEqual(v["credits"], [{"model": "nano-banana-2-1", "credits": 50.0}])
        self.assertEqual(len(v["notes"]), 1)
        ok = json.loads(intro_maker.result_line({"credits": [], "notes": []}, "x.mp4", 11.2))
        self.assertEqual((ok["ok"], ok["path"], ok["seconds"], ok["credits"], ok["notes"]), (True, "x.mp4", 11.2, [], []))


if __name__ == "__main__":
    unittest.main()
