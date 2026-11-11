#!/usr/bin/env python3
"""
Word timing from the narration itself: when each word of a page's narration is spoken, found
by forced alignment (wav2vec2-base-960h, Apache-2.0: the model hears the audio, and the known
text is laid along it). The sync map cuts to the next panel on the word that starts its text,
and the captions light each word as it is said, instead of both being spread by text length.

English only (the model's letters are A-Z): a page in another script, or one the model cannot
follow, keeps the estimate. The weights (360 MB) live in the layered motion pack's folder,
`<data>/models/vfx/wav2vec2-base-960h`, fetched on first use (pinned revision, SHA-256).
Without the pack nothing is aligned and everything works as before.

A page is aligned once and kept in `<chapter>/audio/words.json` (by the audio file's identity
and its text), so the audio and the video job share it.

This file runs in two interpreters:
  * any Python 3.9+ (the worker): `align_pages`, `beat_cuts`. Stdlib only.
  * the pack's venv (PyTorch, transformers): `run`.

Usage:
  python align.py run jobs.json    (pack venv) JSON lines: done / error
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import layered_plan as lp  # noqa: E402

# Changing the model or how words are timed aligns every page again.
VERSION = 1
MODEL_FOLDER = "wav2vec2-base-960h"
MODEL_REPO = "facebook/wav2vec2-base-960h"
MODEL_REV = "22aad52d435eb6dbaf354bdad9b0da84ce7d6156"
# (file, bytes, sha256)
MODEL_FILES = [
    ("config.json", 1596, "d3ec255c063d9f95057b553b19c20135b259875834a4fe9deb218a6be25b4cf3"),
    ("vocab.json", 291, "19727f8944fe6459fc3f240ae2c198395b740f6a029bd23e06656266b83bcf64"),
    ("model.safetensors", 377607901, "8aa76ab2243c81747a1f832954586bc566090c83a0ac167df6f31f0fa917d74a"),
]
SAMPLE_RATE = 16000
FRAME_MS = 20.0           # one model frame
# A page whose letters the model scores below this (mean log-probability) was not followed:
# another language, or audio that is not this text. It keeps the estimate.
MIN_SCORE = -4.0
# A cut to the next panel comes this long before the word that starts it (the eye is there
# when the word lands), never before the last word of the panel before has ended.
CUT_LEAD_MS = 80
# A panel is never on screen shorter than this; a page that would need it keeps the estimate.
MIN_BEAT_MS = 250

_LETTERS = re.compile(r"[A-Z']")


def wanted(config=None):
    """Alignment is on: `align_words` in the settings (default on), MANGAVOX_ALIGN=0 turns it off."""
    env = os.environ.get("MANGAVOX_ALIGN")
    if env is not None:
        return env.strip().lower() not in ("0", "off", "false", "no", "")
    config = config if isinstance(config, dict) else {}
    return config.get("align_words", True) is not False


def tokenize(text):
    """The words of a narration text, as the captions split it (caption_engine.tokenize)."""
    out = []
    for tok in (text or "").split():
        if out and not re.search(r"[0-9A-Za-zÀ-￿]", tok):
            out[-1] += tok
        else:
            out.append(tok)
    return out


def letters(word):
    """The letters of a word the model can hear (A-Z and the apostrophe), upper case."""
    return "".join(_LETTERS.findall(word.upper().replace("’", "'")))


def english(text):
    """Most of the text's letters are ones the model knows."""
    alpha = [c for c in text if c.isalpha()]
    return bool(alpha) and sum(1 for c in alpha if c.isascii()) >= 0.8 * len(alpha)


def store(output_dir):
    return os.path.join(output_dir, "audio", "words.json")


def page_key(audio, text):
    try:
        st = os.stat(audio)
        ident = [st.st_size, int(st.st_mtime)]
    except OSError:
        ident = []
    return hashlib.sha1(json.dumps([VERSION, ident, text]).encode("utf-8")).hexdigest()[:16]


def load(output_dir):
    try:
        with open(store(output_dir), "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def save(output_dir, data):
    path = store(output_dir)
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path + ".part", "w", encoding="utf-8") as f:
            json.dump(data, f)
        os.replace(path + ".part", path)
    except OSError:
        pass


def align_pages(pages, output_dir, config=None, work_dir=None):
    """{audio path: [[word, start ms, end ms], ...]} (times in the audio file) for the `pages`
    ([{"audio", "text"}]) that could be aligned. Kept pages are read back; the rest are aligned
    by the pack's Python in one run. Never raises: a page without an entry keeps the estimate."""
    if not wanted(config):
        return {}
    kept = load(output_dir)
    out, todo, fresh = {}, [], {}
    for p in pages:
        audio, text = p.get("audio"), (p.get("text") or "").strip()
        if not audio or not text:
            continue
        name = os.path.basename(audio)
        key = page_key(audio, text)
        old = kept.get(name)
        if isinstance(old, dict) and old.get("key") == key:
            fresh[name] = old
            if old.get("words"):
                out[audio] = old["words"]
            continue
        if not english(text):
            fresh[name] = {"key": key, "words": None}
            continue
        todo.append({"audio": audio, "text": text, "name": name, "key": key})
    if todo:
        py, _ = lp.pack_python(config)
        done = _run(py, todo, work_dir or os.path.join(output_dir, "audio")) if py else None
        if done is None:
            # The pack is missing or failed: nothing is recorded, the next run tries again.
            if fresh != kept and fresh:
                save(output_dir, dict(kept, **fresh))
            return out
        for j in todo:
            words = done.get(j["audio"])
            fresh[j["name"]] = {"key": j["key"], "words": words or None}
            if words:
                out[j["audio"]] = words
    if fresh != kept:
        save(output_dir, fresh)
    return out


def _run(py, todo, work_dir):
    """Runs `run` in the pack's Python on `todo`; {audio: words or None}, or None when it failed."""
    t0 = time.monotonic()
    path = os.path.join(work_dir, "align_jobs.json")
    try:
        os.makedirs(work_dir, exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump([{"audio": j["audio"], "text": j["text"]} for j in todo], f)
        env = os.environ.copy()
        env.update({"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8", "HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1"})
        env.pop("VIRTUAL_ENV", None)
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "BELOW_NORMAL_PRIORITY_CLASS", 0)
        proc = subprocess.run([py, "-I", os.path.abspath(__file__), "run", path], capture_output=True, text=True, encoding="utf-8",
                              errors="replace", env=env, creationflags=flags, timeout=600 + 5 * len(todo))
    except (OSError, subprocess.TimeoutExpired) as e:
        print(f"[sync] Word timing skipped: {e}", file=sys.stderr)
        return None
    finally:
        try:
            os.remove(path)
        except OSError:
            pass
    done = error = None
    for line in proc.stdout.splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("type") == "done":
            done = msg
        elif msg.get("type") == "error":
            error = msg.get("message")
    if not done:
        print(f"[sync] Word timing skipped: {error or proc.stderr.strip()[-300:] or f'exit {proc.returncode}'}", file=sys.stderr)
        return None
    pages = done.get("pages") or {}
    print(f"[sync] Timing: words of {sum(1 for w in pages.values() if w)} of {len(todo)} page(s) timed from the audio in "
          f"{time.monotonic() - t0:.1f} s ({done.get('device')})", file=sys.stderr)
    return pages


def beat_cuts(words, counts):
    """Where a page's audio is cut between its panels: the start (ms in the audio) of every
    beat after the first, from the page's aligned `words` and how many words each beat says
    (`counts`). None when the words do not add up or a panel would flash by."""
    if not words or not counts or any(c <= 0 for c in counts) or sum(counts) != len(words):
        return None
    cuts, k = [], 0
    for c in counts[:-1]:
        k += c
        before, first = words[k - 1], words[k]
        cut = max(min(before[2], first[1]), first[1] - CUT_LEAD_MS)
        cuts.append(int(round(min(cut, first[1]))))
    edges = [0] + cuts
    if any(b - a < MIN_BEAT_MS for a, b in zip(edges, edges[1:])):
        return None
    return cuts


# ------------------------------------------------------------------------------- the model (venv)
def emit(obj):
    print(json.dumps(obj, ensure_ascii=False), flush=True)


def model_dir(root=None):
    return os.path.join(lp.pack_dir(root), MODEL_FOLDER)


def _sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def ensure_model(root=None):
    """The model's folder, its files fetched first when missing. Raises when one cannot be had
    or is not the pinned file."""
    folder = model_dir(root)
    os.makedirs(folder, exist_ok=True)
    for name, size, digest in MODEL_FILES:
        path = os.path.join(folder, name)
        if os.path.isfile(path) and os.path.getsize(path) == size:
            continue
        part = path + ".part"
        req = urllib.request.Request(f"https://huggingface.co/{MODEL_REPO}/resolve/{MODEL_REV}/{name}", headers={"User-Agent": "mangavox"})
        with urllib.request.urlopen(req, timeout=60) as r, open(part, "wb") as f:
            while True:
                block = r.read(1 << 20)
                if not block:
                    break
                f.write(block)
        if os.path.getsize(part) != size or _sha256(part) != digest:
            os.remove(part)
            raise RuntimeError(f"the word timing model's {name} did not download whole")
        os.replace(part, path)
    return folder


def read_audio(path):
    """The audio as float32 mono at 16 kHz (numpy), or None."""
    import wave
    import numpy as np
    try:
        with wave.open(path, "rb") as wf:
            sr, ch, sw = wf.getframerate(), wf.getnchannels(), wf.getsampwidth()
            raw = wf.readframes(wf.getnframes())
    except (OSError, wave.Error, EOFError):
        return None
    if sw == 2:
        x = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    elif sw == 4:
        x = np.frombuffer(raw, dtype=np.int32).astype(np.float32) / 2147483648.0
    elif sw == 1:
        x = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128) / 128.0
    else:
        return None
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    if sr != SAMPLE_RATE and len(x):
        if sr > SAMPLE_RATE:
            # Low-pass under the new Nyquist first (windowed sinc), then sample by interpolation.
            taps = 63
            n = np.arange(taps) - (taps - 1) / 2
            cut = 0.45 * SAMPLE_RATE / sr
            kernel = (2 * cut * np.sinc(2 * cut * n) * np.hanning(taps)).astype(np.float32)
            x = np.convolve(x, kernel / kernel.sum(), mode="same")
        m = int(round(len(x) * SAMPLE_RATE / sr))
        x = np.interp(np.arange(m) * (sr / SAMPLE_RATE), np.arange(len(x)), x).astype(np.float32)
    return x


def forced(em, tokens, blank=0):
    """The frame each of `tokens` (ids) is said in, in order, on the emissions `em` (frames x
    vocabulary, log-probabilities): the best path that says them all. (frames, mean
    log-probability of the tokens), or None when the audio is too short for the text."""
    import numpy as np
    T, N = em.shape[0], len(tokens)
    if N == 0 or T < N:
        return None
    NEG = -1e30
    tok = np.asarray(tokens)
    dp = np.full(N + 1, NEG, dtype=np.float64)
    dp[0] = 0.0
    back = np.zeros((T + 1, N + 1), dtype=bool)
    for t in range(T):
        stay = dp + em[t, blank]
        move = np.full(N + 1, NEG, dtype=np.float64)
        move[1:] = dp[:-1] + em[t, tok]
        back[t + 1] = move > stay
        dp = np.maximum(stay, move)
    if dp[N] <= NEG / 2:
        return None
    frames, j = [0] * N, N
    for t in range(T, 0, -1):
        if j == 0:
            break
        if back[t, j]:
            frames[j - 1] = t - 1
            j -= 1
    if j != 0:
        return None
    score = float(np.mean([em[f, k] for f, k in zip(frames, tokens)]))
    return frames, score


def time_page(em, text, vocab, total_ms):
    """[[word, start ms, end ms], ...] for every word of `text` (tokenize), or None."""
    words = tokenize(text)
    if not words:
        return None
    sep = vocab.get("|", 4)
    tokens, spans = [], []
    for w in words:
        ids = [vocab[c] for c in letters(w) if c in vocab]
        if ids and tokens:
            tokens.append(sep)
        spans.append((len(tokens), len(tokens) + len(ids)) if ids else None)
        tokens += ids
    if not tokens:
        return None
    got = forced(em, tokens, vocab.get("<pad>", 0))
    if got is None or got[1] < MIN_SCORE:
        return None
    frames = got[0]
    timed = []
    for w, sp in zip(words, spans):
        if sp is None:
            timed.append([w, None, None])
        else:
            timed.append([w, frames[sp[0]] * FRAME_MS, (frames[sp[1] - 1] + 1) * FRAME_MS + 40.0])
    # A word with nothing to hear by (a number, a symbol) sits between its neighbours.
    n = len(timed)
    i = 0
    while i < n:
        if timed[i][1] is None:
            j = i
            while j < n and timed[j][1] is None:
                j += 1
            a = timed[i - 1][2] if i else 0.0
            b = timed[j][1] if j < n else float(total_ms)
            b = max(b, a)
            step = (b - a) / (j - i)
            for k in range(i, j):
                timed[k][1], timed[k][2] = a + step * (k - i), a + step * (k - i + 1)
            i = j
        else:
            i += 1
    out = []
    for k, (w, a, b) in enumerate(timed):
        nxt = timed[k + 1][1] if k + 1 < n else float(total_ms)
        a = min(max(a, out[-1][1] if out else 0.0), float(total_ms))
        b = min(max(b, a + 10.0), max(nxt, a + 10.0))
        out.append([w, int(round(a)), int(round(b))])
    return out


def run(jobs_path):
    """Aligns every job {audio, text} of the JSON file (a list)."""
    import numpy as np
    import torch
    from transformers import Wav2Vec2ForCTC
    t0 = time.monotonic()
    with open(jobs_path, encoding="utf-8") as f:
        jobs = json.load(f)
    folder = ensure_model()
    with open(os.path.join(folder, "vocab.json"), encoding="utf-8") as f:
        vocab = json.load(f)
    model = Wav2Vec2ForCTC.from_pretrained(folder, local_files_only=True).eval()
    device = "cpu"
    if torch.cuda.is_available():
        try:
            model = model.to("cuda")
            device = "cuda"
        except RuntimeError:
            model = model.to("cpu")
    pages = {}
    for j in jobs:
        words = None
        try:
            x = read_audio(j["audio"])
            if x is not None and len(x) > SAMPLE_RATE // 5:
                x = (x - x.mean()) / (x.std() + 1e-7)
                inp = torch.from_numpy(x.astype(np.float32))[None]
                with torch.inference_mode():
                    try:
                        logits = model(inp.to(device)).logits[0]
                    except RuntimeError:
                        # The card is full (a render beside this): the processor does it.
                        if device != "cuda":
                            raise
                        torch.cuda.empty_cache()
                        model, device = model.to("cpu"), "cpu"
                        logits = model(inp).logits[0]
                    em = torch.log_softmax(logits.float(), dim=-1).cpu().numpy()
                words = time_page(em, j["text"], vocab, len(x) * 1000.0 / SAMPLE_RATE)
        except Exception as e:  # one page keeps the estimate; the rest go on
            print(f"[align] {os.path.basename(j['audio'])}: {e}", file=sys.stderr)
        pages[j["audio"]] = words
    emit({"type": "done", "pages": pages, "seconds": round(time.monotonic() - t0, 1), "device": device})


def main(argv):
    if len(argv) >= 3 and argv[1] == "run":
        try:
            run(argv[2])
        except Exception as e:
            emit({"type": "error", "message": str(e)[:400]})
            return 1
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
