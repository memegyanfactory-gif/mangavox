#!/usr/bin/env python3
"""Panel-level beat builder.

Turns the narrator's page-batch prose into an ordered list of *beats*, each tied
to a specific panel image and the exact narration sentence(s) that play over it.
This is what lets the audio/video sync to what's actually visible on screen
("whatever is in the image is written beside it").

It reuses the panel crops produced by split_strip.py (``0001_a.jpg``,
``0001_b.jpg`` …). When a chapter wasn't strip-split (ordinary manga with whole
pages) each full page is treated as a single panel, so the result is still a
clean per-page beat list — finer than the old one-blob-per-page sync.

A beat:
  { "index": 0, "page": 1, "panel": "0001_a.jpg",
    "panel_image": "/abs/path/to/0001_a.jpg",
    "text": "The narration sentence(s) spoken over this panel." }
"""

from __future__ import annotations

import os
import re

PAGE_EXTS = (".jpg", ".jpeg", ".png", ".webp")
# base stem + panel suffix, e.g. 0001_a.jpg
_CROP_RE = re.compile(r"^(?P<base>.+?)_(?P<suffix>[a-z]{1,3})\.(?:jpg|jpeg|png|webp)$", re.IGNORECASE)
_FULL_RE = re.compile(r"^(?P<base>\d+)\.(?:jpg|jpeg|png|webp)$", re.IGNORECASE)
# Sentence boundary covering Latin + Devanagari danda.
_SENT_RE = re.compile(r"[^.!?।]+[.!?।]+|\S+$")


def _list_image_dir(dir_path: str) -> list[str]:
    if not os.path.isdir(dir_path):
        return []
    return sorted(
        f for f in os.listdir(dir_path)
        if f.lower().endswith(PAGE_EXTS) and not f.startswith(".")
    )


def panel_groups(pages_dir: str) -> list[dict]:
    """Ordered list of pages, each with its panel image paths in reading order.

    Returns ``[{ "page": 1, "panels": [(name, abs_path), ...] }, ...]``. If panel
    crops exist they're used; otherwise the full pages become single panels.
    """
    chapter_dir = os.path.dirname(pages_dir.rstrip("/"))
    original_dir = os.path.join(chapter_dir, "pages_original")

    # Group whatever crops live in pages_dir by their base stem.
    crops_by_base: dict[str, list[str]] = {}
    for name in _list_image_dir(pages_dir):
        m = _CROP_RE.match(name)
        if m:
            crops_by_base.setdefault(m.group("base"), []).append(name)

    groups: list[dict] = []

    if crops_by_base:
        # Strip-split chapter: one group per original page, its crops in order.
        for page_index, base in enumerate(sorted(crops_by_base.keys()), start=1):
            panels = [(n, os.path.join(pages_dir, n)) for n in sorted(crops_by_base[base])]
            groups.append({"page": page_index, "panels": panels})
        return groups

    # No crops — fall back to whole pages (prefer originals, then pages_dir).
    src_dir = original_dir if _list_image_dir(original_dir) else pages_dir
    for page_index, name in enumerate(_list_image_dir(src_dir), start=1):
        groups.append({"page": page_index, "panels": [(name, os.path.join(src_dir, name))]})
    return groups


def split_sentences(text: str) -> list[str]:
    if not text:
        return []
    out = [s.strip() for s in _SENT_RE.findall(text)]
    return [s for s in out if len(s) >= 2]


def _distribute(sentences: list[str], n_panels: int) -> list[str]:
    """Spread sentences across n_panels in order (monotonic, no reordering)."""
    if n_panels <= 0:
        return []
    if not sentences:
        return [""] * n_panels
    buckets: list[list[str]] = [[] for _ in range(n_panels)]
    if len(sentences) <= n_panels:
        # One sentence per panel, front-loaded; trailing panels hold silently.
        for i, s in enumerate(sentences):
            buckets[i].append(s)
    else:
        # Proportionally assign each sentence to a panel slice.
        for i, s in enumerate(sentences):
            idx = min(n_panels - 1, (i * n_panels) // len(sentences))
            buckets[idx].append(s)
    return [" ".join(b).strip() for b in buckets]


def build_beats(pages_dir: str, batches: list[dict], focus_points: dict = None) -> list[dict]:
    """Build ordered panel beats from per-batch narration prose.

    ``batches`` is the narrator's ``[{ "from": pageNo, "to": pageNo, "text": ... }]``.
    Narration stays within its own page range, so beats keep local accuracy.
    """
    groups = panel_groups(pages_dir)
    if not groups:
        return []
    by_page = {g["page"]: g["panels"] for g in groups}
    max_page = max(by_page) if by_page else 0

    beats: list[dict] = []
    index = 0
    for batch in batches or []:
        text = (batch.get("text") or "").strip()
        p_from = int(batch.get("from") or 1)
        p_to = int(batch.get("to") or p_from)
        if p_to < p_from:
            p_from, p_to = p_to, p_from
        p_from = max(1, p_from)
        p_to = min(max_page, p_to) if max_page else p_to

        # All panels covered by this batch, in reading order.
        panels: list[tuple[int, str, str, list[int] | None]] = []
        for pg in range(p_from, p_to + 1):
            pg_panels = by_page.get(pg, [])
            pg_focus = []
            if focus_points:
                pg_focus = focus_points.get(str(pg)) or focus_points.get(pg) or []
            
            if len(pg_panels) == 1 and len(pg_focus) > 1:
                # Unsplit page, but has multiple focus points!
                # Create virtual panels for each focus point to zoom dynamically.
                name, path = pg_panels[0]
                for idx, fp in enumerate(pg_focus):
                    panels.append((pg, f"{os.path.splitext(name)[0]}_fp{idx}{os.path.splitext(name)[1]}", path, fp))
            else:
                for idx, (name, path) in enumerate(pg_panels):
                    fp = None
                    if pg_focus:
                        if idx < len(pg_focus):
                            fp = pg_focus[idx]
                        else:
                            fp = pg_focus[-1] if pg_focus else None
                    panels.append((pg, name, path, fp))
        if not panels:
            continue

        sentences = split_sentences(text)
        spread = _distribute(sentences, len(panels))
        for (pg, name, path, fp), beat_text in zip(panels, spread):
            beat = {
                "index": index,
                "page": pg,
                "panel": name,
                "panel_image": path,
                "text": beat_text,
            }
            if fp:
                beat["focus_point"] = fp
            beats.append(beat)
            index += 1

    return beats
