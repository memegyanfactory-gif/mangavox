"""Normalize script segments for per-page TTS (one WAV per page)."""
from __future__ import annotations

import re


def split_text_evenly(text: str, page_count: int) -> list[dict]:
    n = max(1, page_count)
    trimmed = (text or "").strip()
    if not trimmed:
        return [{"page": p, "text": ""} for p in range(n)]
    if n == 1:
        return [{"page": 0, "text": trimmed}]

    sentences = [s.strip() for s in re.split(r"(?<=[.!?।॥])\s+|\n+", trimmed) if s.strip()]
    units = sentences if len(sentences) >= n else trimmed.split()
    if not units:
        return [{"page": 0, "text": trimmed}] + [{"page": p, "text": ""} for p in range(1, n)]

    per = max(1, (len(units) + n - 1) // n)
    out: list[dict] = []
    cursor = 0
    joiner = " " if sentences else " "
    for p in range(n):
        if p == n - 1:
            chunk = joiner.join(units[cursor:]).strip()
        else:
            chunk = joiner.join(units[cursor : cursor + per]).strip()
            cursor += per
        out.append({"page": p, "text": chunk})
    return out


def normalize_script_for_pages(segments: list[dict], page_count: int) -> list[dict]:
    n = max(0, int(page_count or 0))
    if n <= 0:
        return segments

    by_page: dict[int, str] = {}
    for seg in segments:
        p = max(0, int(seg.get("page", 0)))
        t = (seg.get("text") or "").strip()
        if not t:
            continue
        by_page[p] = f"{by_page[p]}\n\n{t}" if p in by_page else t

    filled = [(p, t) for p, t in by_page.items() if t]
    if len(filled) == 1 and n > 1:
        return split_text_evenly(filled[0][1], n)

    return [{"page": p, "text": by_page.get(p, "")} for p in range(n)]
