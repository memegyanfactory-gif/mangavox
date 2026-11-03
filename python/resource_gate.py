"""
One heavy job at a time, across every MangaVox process on the PC.

A Chatterbox voicing (GPU, about 4 GB of VRAM) and a video render (CPU compositing + NVENC) that
run together both crawl: the GPU fills, work spills into system RAM and the PC stutters. Each
heavy stage of pipeline_worker.py runs inside `heavy(kind, label)`, so the one that runs gets the
whole allowance and the others wait their turn:

    with heavy("voice", "Chapter 12", on_wait=lambda holder: ...):
        run_tts(...)

- The gate is an OS file lock on <data>/temp/gate/heavy.lock (msvcrt on Windows, fcntl
  elsewhere). The OS drops it when its process ends, so a crashed or killed job never leaves
  the gate stuck.
- Waiters queue with ticket files in the same folder: voice before ocr before render (a voicing
  is short and unblocks the next render), then first come first served. Only the best live
  ticket may take the lock; tickets of dead processes, or ones not refreshed for STALE_S, are
  removed.
- While waiting, on_wait(holder) is called every WAIT_NOTICE_S; holder is {"pid", "kind",
  "label"} of the job holding the gate, or None when it is not known.
- MANGAVOX_GATE=0 turns the gate off. Any unexpected error in the gate lets the stage run
  ungated: the gate never fails a stage.

Stdlib only.
"""
import contextlib
import json
import os
import sys
import time
import uuid

PRIORITY = {"voice": 0, "ocr": 1, "render": 2}
POLL_S = 0.5
WAIT_NOTICE_S = 5.0
# A waiter refreshes its ticket every poll; one untouched this long belongs to no live waiter
# (its process died, or its pid was reused by another program).
STALE_S = 30.0

LOCK_NAME = "heavy.lock"
HOLDER_NAME = "holder.json"

# The gate this process holds: a nested heavy() runs straight through instead of waiting on itself.
_held = None


def enabled():
    return os.environ.get("MANGAVOX_GATE", "1").strip().lower() not in ("0", "false", "off", "no")


def gate_dir():
    base = os.environ.get("MANGAVOX_DATA") or os.path.join(os.path.expanduser("~"), ".mangavox")
    return os.path.join(base, "temp", "gate")


def _log(msg):
    print(f"[gate] {msg}", file=sys.stderr, flush=True)


# ── Process liveness ──

if sys.platform == "win32":
    import ctypes
    from ctypes import wintypes

    _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _k32.OpenProcess.restype = wintypes.HANDLE
    _k32.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    _k32.GetExitCodeProcess.restype = wintypes.BOOL
    _k32.GetExitCodeProcess.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
    _k32.CloseHandle.argtypes = (wintypes.HANDLE,)

    def pid_alive(pid):
        """True while process `pid` runs (never signals it: os.kill would end it on Windows)."""
        pid = int(pid)
        if pid <= 0:
            return False
        handle = _k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not handle:
            return ctypes.get_last_error() == 5  # ERROR_ACCESS_DENIED: it exists
        try:
            code = wintypes.DWORD()
            if not _k32.GetExitCodeProcess(handle, ctypes.byref(code)):
                return True
            return code.value == 259  # STILL_ACTIVE
        finally:
            _k32.CloseHandle(handle)
else:
    def pid_alive(pid):
        pid = int(pid)
        if pid <= 0:
            return False
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True


# ── The OS lock ──

def _try_lock(fd):
    """Takes the lock without waiting. True when taken."""
    try:
        if sys.platform == "win32":
            import msvcrt
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except OSError:
        return False


def _unlock(fd):
    try:
        if sys.platform == "win32":
            import msvcrt
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


# ── Small files ──

def _write_json(path, data):
    tmp = f"{path}.{os.getpid()}.tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f)
    for attempt in range(5):
        try:
            os.replace(tmp, path)
            return
        except PermissionError:
            # Windows: another process has the target open for a moment (reading it).
            if attempt == 4:
                _remove(tmp)
                raise
            time.sleep(0.05)


def _read_json(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return None


def _remove(path):
    try:
        os.remove(path)
    except OSError:
        pass


def holder(folder=None):
    """{"pid", "kind", "label"} of the live job holding the gate, else None."""
    h = _read_json(os.path.join(folder or gate_dir(), HOLDER_NAME))
    try:
        if h and pid_alive(h.get("pid", 0)):
            return {"pid": int(h["pid"]), "kind": h.get("kind") or "", "label": h.get("label") or ""}
    except Exception:
        pass
    return None


def live_tickets(folder):
    """Waiting tickets, best first. Dead or stale ones are deleted on the way."""
    out = []
    now = time.time()
    for name in os.listdir(folder):
        path = os.path.join(folder, name)
        if name.endswith(".tmp"):
            # A write that never finished (its process died mid-write).
            try:
                if now - os.path.getmtime(path) > STALE_S:
                    _remove(path)
            except OSError:
                pass
            continue
        if not (name.startswith("t-") and name.endswith(".json")):
            continue
        t = _read_json(path)
        if not isinstance(t, dict):
            # Unreadable this moment (another process has it open); garbage is cleared once stale.
            try:
                if now - os.path.getmtime(path) > STALE_S:
                    _remove(path)
            except OSError:
                pass
            continue
        try:
            pid = int(t.get("pid", 0))
            age = now - os.path.getmtime(path)
        except (OSError, TypeError, ValueError):
            continue
        if age > STALE_S or not pid_alive(pid):
            _remove(path)
            continue
        out.append(((int(t.get("priority", 9)), float(t.get("created", now)), name), t))
    out.sort(key=lambda x: x[0])
    return [dict(t, name=key[2]) for key, t in out]


# ── The gate ──

class _Gate:
    def __init__(self, folder, fd, kind):
        self.folder, self.fd, self.kind = folder, fd, kind

    def release(self):
        global _held
        _held = None
        try:
            h = _read_json(os.path.join(self.folder, HOLDER_NAME))
            if isinstance(h, dict) and h.get("pid") == os.getpid():
                _remove(os.path.join(self.folder, HOLDER_NAME))
        finally:
            _unlock(self.fd)


def _acquire(kind, label, on_wait):
    folder = gate_dir()
    os.makedirs(folder, exist_ok=True)
    fd = os.open(os.path.join(folder, LOCK_NAME), os.O_RDWR | os.O_CREAT, 0o666)
    name = f"t-{os.getpid()}-{uuid.uuid4().hex[:8]}.json"
    ticket_path = os.path.join(folder, name)
    ticket = {"pid": os.getpid(), "kind": kind, "label": label,
              "priority": PRIORITY.get(kind, len(PRIORITY)), "created": time.time()}
    taken = False
    try:
        _write_json(ticket_path, ticket)
        next_notice = 0.0
        while True:
            # Refresh the ticket (rewrite it if another waiter removed it as stale).
            try:
                os.utime(ticket_path)
            except OSError:
                _write_json(ticket_path, ticket)
            tickets = live_tickets(folder)
            best = tickets[0]["name"] if tickets else name
            if best == name and _try_lock(fd):
                taken = True
                break
            now = time.monotonic()
            if on_wait is not None and now >= next_notice:
                next_notice = now + WAIT_NOTICE_S
                try:
                    on_wait(holder(folder))
                except Exception:
                    pass
            time.sleep(POLL_S)
    finally:
        _remove(ticket_path)
        if not taken:
            os.close(fd)
    try:
        _write_json(os.path.join(folder, HOLDER_NAME), {"pid": os.getpid(), "kind": kind, "label": label,
                                                        "since": time.time()})
    except Exception:
        pass
    return _Gate(folder, fd, kind)


@contextlib.contextmanager
def heavy(kind, label="", on_wait=None):
    """Runs the block as the PC's one heavy job: waits for its turn first (see the module doc)."""
    global _held
    gate = None
    if enabled() and _held is None:
        try:
            gate = _acquire(kind, label or "", on_wait)
            _held = gate
        except Exception as exc:
            # Never fail the stage over the gate: it runs ungated.
            _log(f"gate unavailable ({exc}); running without it")
            gate = None
    try:
        yield gate
    finally:
        if gate is not None:
            try:
                gate.release()
            except Exception as exc:
                _log(f"gate release failed ({exc})")
            _held = None
