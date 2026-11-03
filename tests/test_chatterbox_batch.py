"""
Chatterbox chapter voicing in batches (tts_bridge._run_chatterbox): every missing chunk goes to
the worker's "voice" op first, then pages are put together from the cache; progress never goes
back; pages already voiced are not sent; a failed batch falls back to page-by-page voicing.
No GPU, no models: the worker is a fake that writes short tones into the cache.
"""
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

PY_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "python")
sys.path.insert(0, PY_DIR)

import numpy as np  # noqa: E402

import chatterbox_tts as cb  # noqa: E402
import narration_tts as nt  # noqa: E402
import tts_bridge as tb  # noqa: E402

SR = 24000


def tone(seconds=1.0):
    t = np.arange(int(SR * seconds)) / SR
    return (0.2 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)


class FakeWorker:
    instances = []
    fail_voice = False

    def __init__(self, *a, **k):
        self.voice_calls, self.page_calls = [], []
        self.ready_info = {"device": "cuda", "gpu": "fake", "sr": SR, "load_s": 0.1, "torch": "x", "fast": True, "precision": "bf16"}
        FakeWorker.instances.append(self)

    def start(self, *a, **k):
        return self.ready_info

    def alive(self):
        return True

    def close(self):
        pass

    def voice(self, voice, chunks, cache_dir, reference=None):
        self.voice_calls.append([c["key"] for c in chunks])
        if FakeWorker.fail_voice:
            raise RuntimeError("batch failed")
        for c in chunks:
            nt.write_wav(os.path.join(cache_dir, f"{c['key']}.wav"), tone(), SR)
        return {"voiced": len(chunks), "chunks": [], "stats": {"t3_s": 0.1}}

    def page(self, out, voice, chunks, reference=None, cache_dir=None, lead_ms=0, tail_ms=0, tempo=1.0):
        self.page_calls.append(out)
        nt.write_wav(out, tone(2.0), SR)
        return {"path": out, "duration": 2.0, "elapsed": 0.0, "chunks": [], "tempo": tempo, "device": "cuda"}


def unit(text):
    return {"text": text, "kind": "other", "mood": "", "intensity": 3.0, "raw_intensity": None,
            "line_kind": "narration", "source": "default"}


def plan_of(pages):
    return [{"page": i + 1, "text": t, "units": [unit(t)] if t else []} for i, t in enumerate(pages)]


class BatchVoicingTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="cb-batch-")
        self.audio = os.path.join(self.dir, "audio")
        os.makedirs(self.audio)
        FakeWorker.instances.clear()
        FakeWorker.fail_voice = False
        self.patch = mock.patch.object(cb, "Worker", FakeWorker)
        self.patch.start()

    def tearDown(self):
        self.patch.stop()
        shutil.rmtree(self.dir, ignore_errors=True)

    def run_plan(self, plan, force_regen=True):
        progress, log = [], {"pages": []}
        files = tb._run_chatterbox(plan, self.audio, "cb_narrator_m", 1.0, 1.0, force_regen,
                                   lambda stage, c, t: progress.append((c, t)), log)
        return files, progress, log

    def test_chapter_voiced_in_one_batch_then_pages_from_cache(self):
        plan = plan_of(["The rain fell on the city. Nobody moved.", "", "He opened the door and walked in."])
        files, progress, log = self.run_plan(plan)
        w = FakeWorker.instances[0]
        self.assertEqual(len(w.voice_calls), 1)
        self.assertEqual(w.page_calls, [])
        expected = [c["key"] for p in plan if p["text"]
                    for c in tb._chatterbox_chunks(nt, cb, p["page"], p["units"], "cb_narrator_m", 1.0, "builtin", 0)]
        self.assertEqual(w.voice_calls[0], expected)
        spoken = [f for f in files if f.get("text")]
        self.assertEqual(len(spoken), 2)
        for f in spoken:
            self.assertTrue(os.path.isfile(f["path"]), f)
            self.assertNotIn("error", f)
        currents = [c for c, _ in progress]
        self.assertEqual(currents, sorted(currents), "progress went back")
        self.assertEqual(progress[-1], (len(plan), len(plan)))
        self.assertEqual(log["batchVoicing"]["voiced"], len(expected))

    def test_pages_already_voiced_are_not_sent(self):
        plan = plan_of(["First page text here.", "Second page text here."])
        nt.write_wav(os.path.join(self.audio, "page_0001.wav"), tone(1.0), SR)
        self.run_plan(plan, force_regen=False)
        w = FakeWorker.instances[0]
        sent = [k for call in w.voice_calls for k in call]
        second = [c["key"] for c in tb._chatterbox_chunks(nt, cb, 2, plan[1]["units"], "cb_narrator_m", 1.0, "builtin", 0)]
        self.assertEqual(sent, second)

    def test_nothing_to_voice_starts_no_worker(self):
        plan = plan_of(["Only page."])
        self.run_plan(plan)
        FakeWorker.instances.clear()
        # The chunks are cached now; voicing the chapter again only puts the page together.
        files, _, log = self.run_plan(plan)
        self.assertEqual(FakeWorker.instances, [])
        self.assertNotIn("batchVoicing", log)
        self.assertTrue(os.path.isfile(files[0]["path"]))

    def test_failed_batch_falls_back_to_page_by_page(self):
        FakeWorker.fail_voice = True
        plan = plan_of(["One sentence.", "Another sentence."])
        files, progress, _ = self.run_plan(plan)
        w = FakeWorker.instances[0]
        self.assertEqual(len(w.page_calls), 2)
        self.assertTrue(all(os.path.isfile(f["path"]) for f in files))
        currents = [c for c, _ in progress]
        self.assertEqual(currents, sorted(currents))


class RunawayLimitTest(unittest.TestCase):
    def test_limit_grows_with_text(self):
        self.assertAlmostEqual(cb.runaway_limit(""), 2.0)
        self.assertAlmostEqual(cb.runaway_limit("x" * 80), 12.0)


if __name__ == "__main__":
    unittest.main()
