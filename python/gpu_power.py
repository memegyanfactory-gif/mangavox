"""Keeps MangaVox's GPU work under a power target, so a render or a voicing never holds the card
near its limit (the PC's power supply has cut out under sustained full GPU load).

Every GPU-heavy loop calls `governor(kind).pace()` once per frame, step or block. A background
sampler reads the card's power draw from nvidia-smi twice a second (one small process for the
whole worker), and the governor turns it into a duty fraction: after each piece of work it rests
busy * (1 - duty) / duty, so the GPU is busy that share of the time, in short even gaps (never a
burst then a long stop). The duty is steered by a PI controller on the smoothed draw: below the
target it eases back up to 1 (no cost when there is headroom), above it comes down, and a peak
well over the target cuts it at once.

The target is MANGAVOX_GPU_WATTS (Settings -> System -> GPU power limit; watts), else
DEFAULT_SHARE of the card's default power limit (200 W of an RTX 3080's 320). Without
nvidia-smi (no NVIDIA GPU, no driver tools) each kind gets a fixed duty instead (FALLBACK_DUTY).

What it cannot do: it lowers the average and sustained draw and the time spent near the limit,
not the card's own millisecond spikes while it works (only a hardware power limit, nvidia-smi -pl
as administrator, or a stronger power supply does that). Stdlib only."""
import atexit
import collections
import os
import shutil
import subprocess
import threading
import time

# The target without MANGAVOX_GPU_WATTS: this share of the card's default power limit.
DEFAULT_SHARE = 0.62
# Each kind's duty when the draw cannot be read (no nvidia-smi).
FALLBACK_DUTY = {"engine": 0.5, "voice": 0.6, "layers": 0.6}
# The controller: how far the duty moves per second of error (as a share of the target), how
# low it may go, and how far over the target a recent peak cuts it at once.
GAIN = 0.8
MIN_DUTY = 0.12
PEAK_OVER = 1.3
# Longest single rest (a long block of work is evened out over the next calls instead).
MAX_REST = 2.0

_NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def _env_float(name, lo, hi):
    try:
        v = float(os.environ.get(name, ""))
    except ValueError:
        return None
    return min(hi, max(lo, v))


def find_smi():
    smi = shutil.which("nvidia-smi")
    if not smi and os.name == "nt":
        smi = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32", "nvidia-smi.exe")
    return smi if smi and os.path.isfile(smi) else None


class Sampler:
    """The card's power draw, read by one `nvidia-smi -lms 500` for the whole process: the last
    reading, a smoothed one (EMA) and the peak of the last few seconds."""

    def __init__(self, smi=None, interval_ms=500, gpu=None):
        self.smi = smi if smi is not None else find_smi()
        self.interval_ms = interval_ms
        self.gpu = str(gpu if gpu is not None else os.environ.get("MANGAVOX_GPU_INDEX", "0"))
        self.watts = None
        self.smooth = None
        self.default_limit = None
        self.recent = collections.deque()
        self.samples = 0
        self.updated = 0.0
        self.proc = None
        self.lock = threading.Lock()
        self.ready = threading.Event()
        if self.smi:
            self._start()

    def _start(self):
        try:
            self.proc = subprocess.Popen(
                [self.smi, f"--id={self.gpu}", "--query-gpu=power.draw,power.default_limit", "--format=csv,noheader,nounits",
                 "-lms", str(self.interval_ms)],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, creationflags=_NO_WINDOW)
        except OSError:
            self.proc = None
            return
        threading.Thread(target=self._read, daemon=True).start()
        atexit.register(self.stop)

    def _read(self):
        for line in self.proc.stdout:
            parts = [p.strip() for p in line.split(",")]
            try:
                w = float(parts[0])
            except (ValueError, IndexError):
                continue
            self.feed(w, parts[1] if len(parts) > 1 else None)
        self.ready.set()

    def feed(self, watts, default_limit=None, now=None):
        """One reading (also how the tests drive it)."""
        now = time.monotonic() if now is None else now
        with self.lock:
            self.watts = watts
            self.smooth = watts if self.smooth is None else self.smooth + 0.4 * (watts - self.smooth)
            try:
                self.default_limit = float(default_limit) if default_limit not in (None, "", "[N/A]") else self.default_limit
            except ValueError:
                pass
            self.recent.append((now, watts))
            while self.recent and now - self.recent[0][0] > 3.0:
                self.recent.popleft()
            self.samples += 1
            self.updated = now
        self.ready.set()

    def peak(self):
        with self.lock:
            return max((w for _, w in self.recent), default=None)

    def live(self, now=None):
        """Whether readings are coming (one in the last few seconds)."""
        now = time.monotonic() if now is None else now
        return self.samples > 0 and now - self.updated < 5.0

    def stop(self):
        if self.proc and self.proc.poll() is None:
            try:
                self.proc.kill()
            except OSError:
                pass


_SAMPLER = None
_SAMPLER_LOCK = threading.Lock()


def sampler():
    global _SAMPLER
    with _SAMPLER_LOCK:
        if _SAMPLER is None:
            _SAMPLER = Sampler()
            if _SAMPLER.proc:
                # The first reading (and the card's default limit) before any work starts.
                _SAMPLER.ready.wait(3.0)
        return _SAMPLER


def target_watts(s=None):
    """The power target in watts: MANGAVOX_GPU_WATTS, else DEFAULT_SHARE of the card's default
    limit, else None (unknown: the fixed duty applies)."""
    w = _env_float("MANGAVOX_GPU_WATTS", 40.0, 2000.0)
    if w:
        return w
    s = s or sampler()
    if s.default_limit:
        return round(s.default_limit * DEFAULT_SHARE)
    return None


class Governor:
    """Paces one GPU-heavy loop (see the module docstring). pace() after each piece of work;
    `max_duty` caps it further (a loop's own fixed duty, Chatterbox's MANGAVOX_GPU_DUTY)."""

    def __init__(self, kind="engine", s=None, target=None):
        self.kind = kind
        self.s = s if s is not None else sampler()
        self.fixed_target = target
        self.duty = 1.0
        self.fallback = FALLBACK_DUTY.get(kind, 0.6)
        self.seen = 0
        self.last_step = None
        self.end = time.monotonic()
        self.rested = 0.0
        self.calls = 0
        self.duty_sum = 0.0
        self.over_s = 0.0

    def target(self):
        return self.fixed_target or target_watts(self.s)

    def steer(self, now=None):
        """Moves the duty on a new reading: integral control on the smoothed draw (a share of the
        target per second), with an immediate cut when the recent peak is far over it."""
        now = time.monotonic() if now is None else now
        target = self.target()
        if not target or not self.s.live(now):
            return None
        if self.s.samples == self.seen:
            return self.duty
        dt = 0.5 if self.last_step is None else min(2.0, max(0.05, now - self.last_step))
        self.last_step = now
        self.seen = self.s.samples
        err = (target - self.s.smooth) / target
        self.duty = min(1.0, max(MIN_DUTY, self.duty + GAIN * err * dt))
        peak = self.s.peak()
        if peak and peak > target * PEAK_OVER:
            self.duty = max(MIN_DUTY, self.duty * 0.7)
        if self.s.smooth > target:
            self.over_s += dt
        return self.duty

    def pace(self, busy_s=None, max_duty=1.0):
        """Rests after `busy_s` seconds of GPU work (default: the time since the last pace()) for
        the share that keeps the draw under the target. Returns the seconds rested."""
        now = time.monotonic()
        busy = (now - self.end) if busy_s is None else busy_s
        d = self.steer(now)
        if d is None:
            d = self.fallback
        d = max(MIN_DUTY, min(d, max_duty, 1.0))
        rest = 0.0
        if d < 1.0 and busy > 0:
            rest = min(MAX_REST, busy * (1.0 - d) / d)
            time.sleep(rest)
        self.rested += rest
        self.calls += 1
        self.duty_sum += d
        self.end = time.monotonic()
        return rest

    def summary(self):
        """A line for the log: target, mean duty, time rested, the draw seen."""
        t = self.target()
        mean = self.duty_sum / self.calls if self.calls else 1.0
        how = f"target {t:.0f} W" if t and self.s.live() else f"no power reading, fixed duty {self.fallback:.2f}"
        seen = f", draw now {self.s.smooth:.0f} W, {self.over_s:.0f} s over" if self.s.smooth is not None else ""
        return f"GPU power governor ({self.kind}): {how}, mean duty {mean:.2f}, rested {self.rested:.1f} s{seen}"


_GOVERNORS = {}


def governor(kind="engine"):
    """The process's governor for `kind` ("engine", "voice", "layers"), sharing one sampler."""
    g = _GOVERNORS.get(kind)
    if g is None:
        g = _GOVERNORS[kind] = Governor(kind)
    return g


def fps_cap():
    """A frame-rate ceiling for the GPU engine from the watt target (it draws about 1.3 W per
    frame per second at 1440p on an RTX 3080: 150 fps, 190 W): the governor works under it."""
    t = target_watts()
    return max(30.0, round(t / 1.3)) if t else 45.0
