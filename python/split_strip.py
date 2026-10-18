#!/usr/bin/env python3
"""
MangaVox Webtoon-Strip Splitter

Slices tall vertical strips (webtoons / long-strip manhwa) into ~1:1 panels
so downstream OCR / vision / reader components have a sane aspect ratio to
work with.

Strategy
--------
For each page image:
  1. Skip if `aspect = height / width` is below `min_aspect` (default 1.4).
     A normal manga page is ~1.4 — taller than that we treat as a strip.
  2. Compute the *desired* number of chunks: round(height / target_chunk_h)
     where target_chunk_h = width (square chunks).
  3. For each desired cut at `i * (height / N)`, search ±15 % of the chunk
     height for the row with the LOWEST variance — i.e. a flat band of
     uniform colour, which is almost always a real panel gutter.
  4. Slice the image at those rows, name the slices `<base>_a.jpg`,
     `<base>_b.jpg`, … (keeps lexicographic sort order intact).

Originals go to `pages_original/<filename>` on first run, so the existing
`CROP_RESTORE` flow puts the chapter back to its untouched state.

Communicates via JSON-lines on stdout (same protocol as autocrop.py):
  {"type": "progress", "current": N, "total": T, "percent": P}
  {"type": "done", "split_pages": K, "total_chunks": M, "skipped": S}
  {"type": "error", "message": "..."}

Usage: python3 split_strip.py <job.json>
job.json schema:
  {
    "pages_dir": "...",            # required
    "min_aspect": 1.4,             # optional, default 1.4
    "target_aspect": 1.0           # optional, default 1.0 (square chunks)
  }
"""
import sys
import json
import os
import shutil
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print(json.dumps({"type": "error", "message": "Pillow not installed. Run: pip install Pillow"}), flush=True)
    sys.exit(1)


def emit(obj):
    print(json.dumps(obj), flush=True)


# ────────────────────────────────────────────────────────────────────
# Cut-row detection
# ────────────────────────────────────────────────────────────────────

def find_cut_rows(img: "Image.Image", n_chunks: int, search_window_ratio: float = 0.15) -> list[int]:
    """
    Return a sorted list of row indices where the image should be cut to
    produce `n_chunks` chunks. Each cut is snapped to the row with the
    lowest pixel variance within ±search_window_ratio of the ideal split.
    """
    if n_chunks <= 1:
        return []

    # Use a small grayscale thumbnail for speed — variance pattern is
    # preserved at low resolution. 200px wide is plenty.
    w, h = img.size
    thumb_w = min(200, w)
    thumb_h = int(h * (thumb_w / w))
    thumb = img.convert("L").resize((thumb_w, thumb_h), Image.BILINEAR)
    pixels = thumb.load()

    # Per-row variance (cheap one-pass mean+variance) on the thumbnail
    row_var = []
    for ty in range(thumb_h):
        s = 0
        sq = 0
        for tx in range(thumb_w):
            v = pixels[tx, ty]
            s += v
            sq += v * v
        mean = s / thumb_w
        var = (sq / thumb_w) - (mean * mean)
        row_var.append(var)

    # Map ideal split positions on the FULL image back to thumbnail rows
    cut_rows = []
    chunk_h_full = h / n_chunks
    window_full = int(chunk_h_full * search_window_ratio)
    scale = thumb_h / h

    for i in range(1, n_chunks):
        ideal_full = int(round(i * chunk_h_full))
        lo_full = max(1, ideal_full - window_full)
        hi_full = min(h - 1, ideal_full + window_full)
        lo_t = max(0, int(lo_full * scale))
        hi_t = min(thumb_h - 1, int(hi_full * scale))
        if lo_t >= hi_t:
            cut_rows.append(ideal_full)
            continue

        # Pick the thumbnail row with minimum variance in the window.
        # Tie-break by closeness to the ideal — a slight bias toward the
        # exact midpoint keeps chunks roughly even when there's no clear
        # gutter at all.
        best_t = lo_t
        best_score = float("inf")
        ideal_t = int(ideal_full * scale)
        for ty in range(lo_t, hi_t + 1):
            score = row_var[ty] + 0.05 * abs(ty - ideal_t)
            if score < best_score:
                best_score = score
                best_t = ty
        # Map back to full-image row
        cut_rows.append(int(best_t / scale))

    cut_rows = sorted(set(cut_rows))
    # Defensive: drop any cut that's too close to top/bottom or to its neighbour
    min_gap = max(20, int(h / (n_chunks * 4)))
    cleaned = []
    prev = 0
    for r in cut_rows:
        if r - prev >= min_gap and (h - r) >= min_gap:
            cleaned.append(r)
            prev = r
    return cleaned


# ────────────────────────────────────────────────────────────────────
# Per-page split
# ────────────────────────────────────────────────────────────────────

def slice_suffix(i: int) -> str:
    """0 → 'a', 1 → 'b', …, 25 → 'z', 26 → 'aa', …"""
    if i < 26:
        return chr(ord("a") + i)
    out = ""
    n = i
    while True:
        out = chr(ord("a") + (n % 26)) + out
        n = n // 26 - 1
        if n < 0:
            break
    return out


def split_one(image_path: Path, min_aspect: float, target_aspect: float) -> int:
    """
    Returns the number of chunks the page was split into. Returns 0 if the
    page wasn't tall enough to warrant splitting (skipped).
    """
    with Image.open(image_path) as img:
        w, h = img.size
        aspect = h / max(w, 1)
        if aspect < min_aspect:
            return 0  # not a strip

        # How many roughly-square (or `target_aspect`) chunks?
        target_h = int(round(w * target_aspect))
        n_chunks = max(2, int(round(h / target_h)))

        cut_rows = find_cut_rows(img, n_chunks)
        # Build [0, c1, c2, …, h] band edges
        edges = [0] + cut_rows + [h]

        # Save format: prefer JPEG for size; preserve PNG if input is PNG
        fmt = (img.format or "").upper()
        suffix_map = {"JPEG": ".jpg", "JPG": ".jpg", "PNG": ".png", "WEBP": ".webp"}
        out_suffix = suffix_map.get(fmt, image_path.suffix.lower())

        base_stem = image_path.stem
        out_dir = image_path.parent

        chunks_written = 0
        for i in range(len(edges) - 1):
            top, bottom = edges[i], edges[i + 1]
            if bottom - top < 10:
                continue
            chunk = img.crop((0, top, w, bottom))
            out_name = f"{base_stem}_{slice_suffix(i)}{out_suffix}"
            out_path = out_dir / out_name
            save_kwargs = {}
            if fmt in ("JPEG", "JPG"):
                save_kwargs.update(quality=92, optimize=True)
            chunk.save(out_path, **save_kwargs)
            chunks_written += 1

    # Original file is now superseded — remove it (the backup in
    # pages_original/ keeps it safe for Restore).
    try:
        image_path.unlink()
    except Exception as e:
        emit({"type": "warn", "page": image_path.name,
              "message": f"could not remove original after split: {e}"})

    return chunks_written


# ────────────────────────────────────────────────────────────────────
# Main
# ────────────────────────────────────────────────────────────────────

def main() -> int:
    if len(sys.argv) < 2:
        emit({"type": "error", "message": "Missing job.json argument"})
        return 1

    try:
        with open(sys.argv[1], "r", encoding="utf-8") as f:
            job = json.load(f)
    except Exception as e:
        emit({"type": "error", "message": f"Cannot read job.json: {e}"})
        return 1

    pages_dir = Path(job.get("pages_dir", ""))
    min_aspect = float(job.get("min_aspect", 1.4))
    target_aspect = float(job.get("target_aspect", 1.0))

    if not pages_dir.is_dir():
        emit({"type": "error", "message": f"pages_dir not found: {pages_dir}"})
        return 1

    exts = {".jpg", ".jpeg", ".png", ".webp"}
    pages = sorted(p for p in pages_dir.iterdir()
                   if p.is_file() and p.suffix.lower() in exts)
    if not pages:
        emit({"type": "error", "message": "No image files found in pages_dir"})
        return 1

    # Backup originals to pages_original/ so the existing Restore flow works
    backup_dir = pages_dir.parent / "pages_original"
    backup_dir.mkdir(exist_ok=True)
    for p in pages:
        backup = backup_dir / p.name
        if not backup.exists():
            shutil.copy2(p, backup)

    total = len(pages)
    split_pages = 0
    total_chunks = 0
    skipped = 0

    for i, page in enumerate(pages, 1):
        try:
            n = split_one(page, min_aspect, target_aspect)
            if n > 0:
                split_pages += 1
                total_chunks += n
            else:
                skipped += 1
        except Exception as e:
            emit({"type": "warn", "page": page.name, "message": str(e)})
            skipped += 1
        emit({"type": "progress", "current": i, "total": total,
              "percent": round(i / total * 100, 1)})

    emit({"type": "done",
          "split_pages": split_pages,
          "total_chunks": total_chunks,
          "skipped": skipped,
          "total": total})
    return 0


if __name__ == "__main__":
    sys.exit(main())
