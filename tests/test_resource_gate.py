"""
The heavy-work gate (python/resource_gate.py): one heavy job at a time across processes, the OS
lock dropping with a killed holder, the waiting order (voice, ocr, render, then first come),
MANGAVOX_GATE=0, and errors that let the stage run ungated. Also the worker's wrapper
(pipeline_worker.heavy_stage). No GPU, no models: the "jobs" are short child processes.
"""
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from unittest import mock

PY_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "python")
sys.path.insert(0, PY_DIR)

import resource_gate as gate  # noqa: E402
import pipeline_worker  # noqa: E402

# A child that takes the gate, says so on stdout, then holds it until killed or until `release`
# appears next to its data folder.
HOLDER_SCRIPT = r"""
import os, sys, time
sys.path.insert(0, sys.argv[1])
import resource_gate
with resource_gate.heavy(sys.argv[2], "child job"):
    print("held", flush=True)
    stop = os.path.join(os.environ["MANGAVOX_DATA"], "release")
    deadline = time.time() + 30
    while not os.path.exists(stop) and time.time() < deadline:
        time.sleep(0.05)
"""


class GateTestCase(unittest.TestCase):
    def setUp(self):
        self.data = tempfile.mkdtemp(prefix="mv-gate-")
        self.env = mock.patch.dict(os.environ, {"MANGAVOX_DATA": self.data, "MANGAVOX_GATE": "1"})
        self.env.start()
        self.poll = mock.patch.object(gate, "POLL_S", 0.05)
        self.poll.start()
        self.children = []

    def tearDown(self):
        for p in self.children:
            if p.poll() is None:
                p.kill()
            p.wait(timeout=10)
            if p.stdout:
                p.stdout.close()
        self.poll.stop()
        self.env.stop()
        gate._held = None
        shutil.rmtree(self.data, ignore_errors=True)

    def start_holder(self, kind="render"):
        p = subprocess.Popen([sys.executable, "-c", HOLDER_SCRIPT, PY_DIR, kind],
                             stdout=subprocess.PIPE, text=True, env=dict(os.environ))
        self.children.append(p)
        self.assertEqual(p.stdout.readline().strip(), "held")
        return p

    def folder(self):
        return gate.gate_dir()


class TestGate(GateTestCase):
    def test_holds_and_releases_with_a_holder_record(self):
        with gate.heavy("render", "Chapter 1") as g:
            self.assertIsNotNone(g)
            h = gate.holder()
            self.assertEqual((h["pid"], h["kind"], h["label"]), (os.getpid(), "render", "Chapter 1"))
        self.assertIsNone(gate.holder())
        self.assertIsNone(gate._held)
        # Free again: a second job takes it at once.
        with gate.heavy("voice") as g:
            self.assertIsNotNone(g)

    def test_nested_heavy_runs_straight_through(self):
        with gate.heavy("render") as outer:
            with gate.heavy("voice") as inner:
                self.assertIsNone(inner)
            self.assertIs(gate._held, outer)
        self.assertIsNone(gate._held)

    def test_waits_for_another_process_and_names_it(self):
        child = self.start_holder("voice")
        seen = []
        start = time.monotonic()

        def on_wait(h):
            seen.append(h)
            open(os.path.join(self.data, "release"), "w").close()

        with gate.heavy("render", "Chapter 2", on_wait=on_wait) as g:
            self.assertIsNotNone(g)
            # The child has left the gate before this one got it.
            child.wait(timeout=10)
        self.assertTrue(seen)
        self.assertEqual((seen[0]["kind"], seen[0]["label"], seen[0]["pid"]), ("voice", "child job", child.pid))
        self.assertLess(time.monotonic() - start, 10)
        self.assertEqual([n for n in os.listdir(self.folder()) if n.startswith("t-")], [])

    def test_a_killed_holder_never_leaves_the_gate_stuck(self):
        child = self.start_holder("render")

        def on_wait(_h):
            if child.poll() is None:
                child.kill()

        with gate.heavy("render", on_wait=on_wait) as g:
            self.assertIsNotNone(g)
            self.assertIsNotNone(child.poll())

    def test_off_switch(self):
        for value in ("0", "off", "false", "no"):
            with mock.patch.dict(os.environ, {"MANGAVOX_GATE": value}):
                with gate.heavy("render") as g:
                    self.assertIsNone(g)
        self.assertFalse(os.path.exists(self.folder()))

    def test_off_switch_skips_a_busy_gate(self):
        self.start_holder("render")
        with mock.patch.dict(os.environ, {"MANGAVOX_GATE": "0"}):
            start = time.monotonic()
            with gate.heavy("render") as g:
                self.assertIsNone(g)
            self.assertLess(time.monotonic() - start, 2)

    def test_a_broken_gate_runs_the_stage_ungated(self):
        ran = []
        with mock.patch.object(gate, "_acquire", side_effect=OSError("disk full")):
            with gate.heavy("render") as g:
                ran.append(g)
        self.assertEqual(ran, [None])
        self.assertIsNone(gate._held)

    def test_an_unusable_gate_folder_runs_the_stage_ungated(self):
        # The data path is a file, so the gate folder cannot be made.
        blocker = os.path.join(self.data, "not-a-dir")
        open(blocker, "w").close()
        with mock.patch.dict(os.environ, {"MANGAVOX_DATA": blocker}):
            with gate.heavy("render") as g:
                self.assertIsNone(g)

    def test_the_stage_error_still_releases_the_gate(self):
        with self.assertRaises(ValueError):
            with gate.heavy("render"):
                raise ValueError("render failed")
        self.assertIsNone(gate.holder())
        with gate.heavy("render") as g:
            self.assertIsNotNone(g)


class TestTickets(GateTestCase):
    def write_ticket(self, name, kind, created, pid=None):
        os.makedirs(self.folder(), exist_ok=True)
        path = os.path.join(self.folder(), name)
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"pid": pid or os.getpid(), "kind": kind, "priority": gate.PRIORITY[kind],
                       "created": created}, f)
        return path

    def test_voice_then_ocr_then_render_then_first_come(self):
        self.write_ticket("t-a.json", "render", 1.0)
        self.write_ticket("t-b.json", "voice", 5.0)
        self.write_ticket("t-c.json", "ocr", 2.0)
        self.write_ticket("t-d.json", "voice", 3.0)
        self.write_ticket("t-e.json", "render", 0.5)
        order = [t["name"] for t in gate.live_tickets(self.folder())]
        self.assertEqual(order, ["t-d.json", "t-b.json", "t-c.json", "t-e.json", "t-a.json"])

    def test_dead_and_stale_tickets_are_cleared(self):
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait(timeout=10)
        dead = self.write_ticket("t-dead.json", "voice", 1.0, pid=p.pid)
        stale = self.write_ticket("t-stale.json", "voice", 1.0)
        old = time.time() - gate.STALE_S - 5
        os.utime(stale, (old, old))
        live = self.write_ticket("t-live.json", "render", 2.0)
        order = [t["name"] for t in gate.live_tickets(self.folder())]
        self.assertEqual(order, ["t-live.json"])
        self.assertFalse(os.path.exists(dead))
        self.assertFalse(os.path.exists(stale))
        self.assertTrue(os.path.exists(live))

    def test_a_better_waiter_goes_first_even_with_the_lock_free(self):
        # A live voice ticket ahead in the queue: a render waits for it although no one holds the lock.
        ticket = self.write_ticket("t-voice.json", "voice", 1.0)
        got = []

        def on_wait(h):
            got.append(("waited", h))
            os.remove(ticket)  # The voicing took its turn and left.

        with gate.heavy("render", on_wait=on_wait) as g:
            self.assertIsNotNone(g)
        self.assertEqual(got, [("waited", None)])

    def test_pid_alive(self):
        self.assertTrue(gate.pid_alive(os.getpid()))
        self.assertFalse(gate.pid_alive(0))
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait(timeout=10)
        self.assertFalse(gate.pid_alive(p.pid))


class TestWorkerWrapper(GateTestCase):
    def test_waiting_notice_goes_out_once_per_holder_kind(self):
        calls = []

        def fake_heavy(kind, label, on_wait=None):
            calls.append((kind, label))
            for h in ({"kind": "voice"}, {"kind": "voice"}, {"kind": "render"}, None, None):
                on_wait(h)
            return mock.MagicMock()

        out = io.StringIO()
        with mock.patch.object(gate, "heavy", fake_heavy), redirect_stdout(out):
            pipeline_worker.heavy_stage("render", "video", "Chapter 3")
        lines = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(calls, [("render", "Chapter 3")])
        self.assertEqual([m["type"] for m in lines], ["info"] * 3)
        self.assertEqual({m["stage"] for m in lines}, {"video"})
        self.assertIn("voicing", lines[0]["message"])
        self.assertIn("video render", lines[1]["message"])
        self.assertIn("heavy job", lines[2]["message"])

    def test_no_gate_module_runs_the_stage(self):
        with mock.patch.dict(sys.modules, {"resource_gate": None}):
            with pipeline_worker.heavy_stage("render", "video", "x") as g:
                self.assertIsNone(g)

    def test_real_gate_through_the_wrapper(self):
        with pipeline_worker.heavy_stage("voice", "tts", "Chapter 4") as g:
            self.assertIsNotNone(g)
            self.assertEqual(gate.holder()["label"], "Chapter 4")
        self.assertIsNone(gate.holder())


if __name__ == "__main__":
    unittest.main()
