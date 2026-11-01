"""
Music ducking and the SFX stem in the final audio mix (video_generator._audio_mix_graph with
sfx.make_duck_gain): measured on synthetic narration (harmonic tone bursts with syllables) over
music (steady filtered noise), by running the real ffmpeg graph and subtracting the narration.
Skipped without ffmpeg.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import sfx  # noqa: E402
import video_generator as vg  # noqa: E402

SR = sfx.SR
TOTAL = 12.0
# Speech: 1.5 s on, 1.0 s off, from 0.5 s.
SPEECH = [(0.5 + 2.5 * i, 2.0 + 2.5 * i) for i in range(4)]


def _speech():
    t = np.arange(int(TOTAL * SR)) / SR
    voice = sum(np.sin(2 * np.pi * 150 * h * t) / h for h in range(1, 12))
    syllables = 0.55 + 0.45 * np.abs(np.sin(2 * np.pi * 2.5 * t))
    on = np.zeros_like(t)
    for a, b in SPEECH:
        on[(t >= a) & (t < b)] = 1.0
    x = voice * syllables * on
    on_rms = np.sqrt(np.mean(x[on > 0] ** 2))
    return (x / on_rms * 10 ** (-20 / 20)).astype(np.float32), on > 0


def _music(level_db=-30.0, seed=1):
    rng = np.random.default_rng(seed)
    x = sfx._lp(rng.standard_normal(int(TOTAL * SR)), 3000.0)
    return (x / np.sqrt(np.mean(x ** 2)) * 10 ** (level_db / 20)).astype(np.float32)


def _db(x):
    return 10 * np.log10(max(float(np.mean(np.square(x, dtype=np.float64))), 1e-12))


def _windows(t0, t1):
    """Sample ranges well inside speech (from 150 ms after it starts) and well inside the gaps
    (from 600 ms after it stops)."""
    sp, gap = [], []
    for i, (a, b) in enumerate(SPEECH):
        sp.append((a + 0.15, b - 0.05))
        nxt = SPEECH[i + 1][0] if i + 1 < len(SPEECH) else TOTAL
        if nxt - (b + 0.6) > 0.2:
            gap.append((b + 0.6, nxt - 0.05))
    cut = lambda ws: np.concatenate([np.arange(int(a * SR), int(b * SR)) for a, b in ws])
    return cut(sp), cut(gap)


def _render(work, tag, narration, music, graph_fn, use_duck):
    nar_path, mus_path = os.path.join(work, f"nar_{tag}.wav"), os.path.join(work, f"mus_{tag}.wav")
    sfx.write_wav(nar_path, narration)
    sfx.write_wav(mus_path, music)
    inputs = ["-i", nar_path, "-i", mus_path]
    duck = None
    if use_duck:
        gain = sfx.make_duck_gain(nar_path, ["-i", mus_path], 1.0, 1.0, TOTAL, os.path.join(work, f"duck_{tag}.wav"))
        assert gain, "gain curve made"
        inputs += ["-i", gain]
        duck = 2
    out = os.path.join(work, f"mix_{tag}.f32")
    cmd = ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", *inputs,
           "-filter_complex", graph_fn(duck) + ";[mix]pan=mono|c0=c0[m]", "-map", "[m]",
           "-ar", str(SR), "-f", "f32le", out]
    subprocess.run(cmd, check=True, capture_output=True, timeout=120)
    return np.fromfile(out, dtype="<f4")


def run_mix(work, narration, music, graph_fn, use_duck):
    """(music, narration) as they leave the mix (one channel): the mix minus the same mix with
    silent music, and that mix itself."""
    mix = _render(work, "a", narration, music, graph_fn, use_duck)
    voice = _render(work, "b", narration, np.zeros_like(music), graph_fn, False)
    m = min(len(mix), len(voice))
    return mix[:m] - voice[:m], voice[:m]


def ducking(music_out, voice_out):
    """(music under speech, music in the gaps, voice) levels in dB, as the mix has them."""
    sp, gap = _windows(0, TOTAL)
    return _db(music_out[sp]), _db(music_out[gap]), _db(voice_out[sp])


OLD_GRAPH = (
    "[0:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=1.0,asplit=2[nar][key];"
    "[1:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=1.0[mus];"
    "[mus][key]sidechaincompress=threshold=0.03:ratio=4:attack=20:release=400[duck];"
    "[nar][duck]amix=inputs=2:duration=first:normalize=0[mix]"
)

# The same mix with no duck at all: the music as it would be.
NO_DUCK = (
    "[0:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=1.0[nar];"
    "[1:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=1.0[mus];"
    "[nar][mus]amix=inputs=2:duration=first:normalize=0[mix]"
)


@unittest.skipUnless(shutil.which("ffmpeg"), "needs ffmpeg")
class MusicDuckTest(unittest.TestCase):
    def setUp(self):
        self.work = tempfile.mkdtemp(prefix="mv-duck-")
        self.narration, self.on = _speech()

    def tearDown(self):
        shutil.rmtree(self.work, ignore_errors=True)

    def test_gain_curve_puts_music_well_under_the_voice(self):
        music = _music(-30.0)
        out, voice_out = run_mix(self.work, self.narration, music, lambda d: vg._audio_mix_graph(0, True, 1.0, 1.0, duck=d), True)
        under, gaps, voice = ducking(out, voice_out)
        # 15-20 dB under the voice while it speaks; the music's own level (within 1 dB) in the gaps.
        self.assertTrue(15.0 <= voice - under <= 21.0, f"voice {voice:.1f} music under it {under:.1f}")
        ref, _ = run_mix(self.work, self.narration, music, lambda d: NO_DUCK, False)
        self.assertAlmostEqual(gaps, ducking(ref, voice_out)[1], delta=1.0)

    def test_duck_timing(self):
        music = _music(-30.0)
        out, voice_out = run_mix(self.work, self.narration, music, lambda d: vg._audio_mix_graph(0, True, 1.0, 1.0, duck=d), True)
        env = sfx.frame_db(out, SR, 0.005)
        base = ducking(out, voice_out)[1]
        a, b = SPEECH[1]
        at = lambda s: float(np.mean(env[int(s / 0.005):int(s / 0.005) + 2]))
        ducked = float(np.mean(env[int((a + 0.3) / 0.005):int((b - 0.1) / 0.005)]))
        self.assertLess(ducked, base - 6.0)
        # Down (within 2 dB) 40 ms after the voice starts: it dips a little ahead of it.
        self.assertLess(at(a + 0.04), ducked + 2.0)
        self.assertGreater(at(a - 0.1), base - 1.0, "not ducked long before the voice")
        # Held through a short pause, back up (within 1 dB) after the hold and the release.
        self.assertLess(at(b + 0.1), ducked + 2.0)
        self.assertGreater(at(b + sfx.DUCK_HOLD + sfx.DUCK_RELEASE + 0.1), base - 1.0)

    def test_an_already_ducked_score_is_left_alone(self):
        # A score that sits 24 dB under the voice while it speaks and 14 dB under it in the gaps
        # (mv-score's own master) needs no more ducking.
        music = _music(-34.0)
        t = np.arange(len(music)) / SR
        dip = np.ones_like(music)
        for a, b in SPEECH:
            dip[(t >= a) & (t < b)] = 10 ** (-10 / 20)
        music = music * dip
        out, voice_out = run_mix(self.work, self.narration, music, lambda d: vg._audio_mix_graph(0, True, 1.0, 1.0, duck=d), True)
        under, gaps, voice = ducking(out, voice_out)
        ref, _ = run_mix(self.work, self.narration, music, lambda d: NO_DUCK, False)
        ref_under, ref_gaps, _ = ducking(ref, voice_out)
        self.assertAlmostEqual(under, ref_under, delta=1.0)
        self.assertAlmostEqual(gaps, ref_gaps, delta=1.0)

    def test_loud_music_is_capped_in_the_gaps(self):
        music = _music(-16.0)
        out, voice_out = run_mix(self.work, self.narration, music, lambda d: vg._audio_mix_graph(0, True, 1.0, 1.0, duck=d), True)
        under, gaps, voice = ducking(out, voice_out)
        self.assertTrue(15.0 <= voice - under <= 21.0)
        self.assertAlmostEqual(voice - gaps, sfx.GAP_UNDER_DB, delta=1.5)

    def test_compressor_fallback_ducks_deeper_than_before(self):
        music = _music(-30.0)
        old, voice_out = run_mix(self.work, self.narration, music, lambda d: OLD_GRAPH, False)
        new, _ = run_mix(self.work, self.narration, music, lambda d: vg._audio_mix_graph(0, True, 1.0, 1.0), False)
        old_under, old_gaps, voice = ducking(old, voice_out)
        new_under, new_gaps, _ = ducking(new, voice_out)
        self.assertLess(old_gaps - old_under, 10.0, "the old compressor ducked only a little")
        self.assertGreater(new_gaps - new_under, 15.0)
        self.assertAlmostEqual(new_gaps, old_gaps, delta=1.0)


class MixGraphTest(unittest.TestCase):
    def test_graph_inputs(self):
        g = vg._audio_mix_graph(0, False, 1.0, 1.0)
        self.assertEqual(g, "[0:a]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume=1.0[mix]")
        g = vg._audio_mix_graph(0, True, 1.0, 0.5, solo=True, duck=2, sfx=3)
        self.assertIn("[2:a]", g)
        self.assertIn("amultiply", g)
        self.assertIn("[3:a]", g)
        self.assertIn("amix=inputs=3", g)
        self.assertIn("[solo]", g)
        self.assertNotIn("sidechaincompress", g)
        g = vg._audio_mix_graph(0, False, 1.0, 1.0, sfx=1)
        self.assertIn("amix=inputs=2", g)
        self.assertNotIn("[solo]", g)
        g = vg._audio_mix_graph(0, True, 1.0, 1.0)
        self.assertIn("sidechaincompress", g)

    def test_sfx_switch(self):
        work = tempfile.mkdtemp(prefix="mv-sfx-switch-")
        try:
            items = [{"sfx": "boom", "sfx_strength": 3}, {}]
            clips = [{"idx": 0, "page": 0, "impact": False, "start": 0.5}, {"idx": 1, "page": 0, "impact": False, "start": 3.0}]
            args = (work, items, clips, [0.5, 3.0], [None, None], 6.0, os.path.join(work, "none.wav"), 1.0, work)
            self.assertIsNone(vg._build_sfx_stem({"sfx": "off"}, *args))
            self.assertIsNone(vg._build_sfx_stem({"sfx": False}, *args))
            os.environ["MANGAVOX_FX_CACHE"] = os.path.join(work, "fx")
            stem = vg._build_sfx_stem({}, *args)
            self.assertTrue(stem and os.path.exists(stem), "on by default")
        finally:
            os.environ.pop("MANGAVOX_FX_CACHE", None)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
