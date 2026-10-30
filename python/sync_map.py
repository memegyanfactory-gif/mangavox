"""
Sync Map Generator
Creates timing map from page texts + audio durations.
"""
import os
import json

def generate_sync_map(text_segments, audio_files, output_dir, page_count=0, beats=None):
    """
    Generate a sync map linking pages to audio timing.

    When ``beats`` (panel-level beats from panel_beats.py) are supplied, each
    page segment is subdivided across that page's panels proportional to the
    narration text length, producing ``beat_segments`` with a ``panel_image``
    per entry so the video can swap panels in sync with the narration. The
    page-level ``segments`` are kept unchanged for backward compatibility.

    Returns the sync map dict.
    """
    audio_by_page = {}
    text_by_page = {}
    for audio in audio_files:
        page = int(audio.get("page", 0))
        text_by_page[page] = audio.get("text") or text_by_page.get(page, "")
        path = audio.get("path")
        if path and os.path.exists(path) and os.path.getsize(path) > 100:
            audio_by_page[page] = path

    for seg in text_segments or []:
        page = int(seg.get("page", 0))
        t = (seg.get("text") or "").strip()
        if t:
            text_by_page[page] = t

    pages = sorted(set(list(audio_by_page.keys()) + list(text_by_page.keys())))
    if page_count and page_count > 0:
        pages = list(range(page_count))
    elif not pages:
        pages = [0]

    segments = []
    current_ms = 0
    audio_dir = os.path.join(output_dir, "audio")

    for page in pages:
        text = text_by_page.get(page, "")
        audio_path = audio_by_page.get(page)
        if not audio_path:
            candidate = os.path.join(audio_dir, f"page_{page:04d}.wav")
            if os.path.isfile(candidate) and os.path.getsize(candidate) > 100:
                audio_path = candidate

        if not audio_path or not os.path.exists(audio_path):
            continue

        duration_ms = _get_audio_duration_ms(audio_path)

        segments.append({
            "page": page,
            "text": text,
            "audio_file": os.path.basename(audio_path),
            "start_ms": current_ms,
            "end_ms": current_ms + duration_ms,
        })

        current_ms += duration_ms

    # Pages the story left out (every beat of the page marked "skip") are cut: no picture, no
    # silence. Kept when nothing would be left.
    offset = _beat_offset(segments, beats)
    skip = _skipped_pages(beats, offset)
    if skip and any(s["page"] not in skip for s in segments):
        current_ms = 0
        kept = []
        for s in segments:
            if s["page"] in skip:
                continue
            span = s["end_ms"] - s["start_ms"]
            s["start_ms"], s["end_ms"] = current_ms, current_ms + span
            current_ms += span
            kept.append(s)
        segments = kept

    sync_map = {
        "segments": segments,
        "total_duration_ms": current_ms,
    }

    # ── Panel-level beats ───────────────────────────────────────────────────
    # Subdivide each page segment across that page's panels so the video can
    # advance panel-by-panel in step with the narration.
    beat_segments = _build_beat_segments(segments, beats, offset)
    if beat_segments:
        sync_map["beat_segments"] = beat_segments
        sync_map["has_beats"] = True

    # Captions: SRT cues driven by the same timing the video uses, so on-screen
    # text stays perfectly synced with the panel + audio.
    try:
        from captions import write_captions
        srt_path = write_captions(beat_segments or segments, output_dir)
        if srt_path:
            sync_map["captions"] = os.path.basename(srt_path)
    except Exception:
        pass  # captions are optional — never block the sync map

    # Save to file
    sync_path = os.path.join(output_dir, "sync_map.json")
    with open(sync_path, 'w') as f:
        json.dump(sync_map, f, indent=2)

    return sync_map


def _beat_offset(segments, beats):
    """Beats may be 1-based while audio pages are 0-based (or vice versa): the uniform offset
    that lines them up, found before any page is cut."""
    seg_pages = [int(s["page"]) for s in segments]
    beat_pages = [int(b.get("page", 0)) for b in beats or []]
    return (min(seg_pages) - min(beat_pages)) if (seg_pages and beat_pages) else 0


def _skipped_pages(beats, offset):
    """Audio pages whose every beat is marked "skip" (pages the story leaves out)."""
    flags = {}
    for b in beats or []:
        page = int(b.get("page", 0)) + offset
        flags[page] = flags.get(page, True) and bool(b.get("skip"))
    return {p for p, all_skip in flags.items() if all_skip}


def _build_beat_segments(segments, beats, offset=None):
    """Split each page segment's time window across its panels by text length."""
    if not beats or not segments:
        return []
    if offset is None:
        offset = _beat_offset(segments, beats)

    beats_by_page = {}
    for b in beats:
        beats_by_page.setdefault(int(b.get("page", 0)) + offset, []).append(b)

    out = []
    matched_any = False
    for seg in segments:
        page = int(seg["page"])
        page_beats = beats_by_page.get(page) or []
        start, end = int(seg["start_ms"]), int(seg["end_ms"])
        span = max(0, end - start)

        if not page_beats:
            # No panel data for this page — keep it as a single full-page beat.
            fp = None
            if beats:
                # Find if any beat for this page has a focus point
                for pb in beats:
                    if int(pb.get("page", 0)) + offset == page and pb.get("focus_point"):
                        fp = pb["focus_point"]
                        break
            beat_seg = {
                "page": page, "panel": None, "panel_image": None,
                "text": seg.get("text", ""), "audio_file": seg.get("audio_file"),
                "start_ms": start, "end_ms": end,
                "audio_offset_ms": 0, "duration_ms": span,
            }
            if fp:
                beat_seg["focus_point"] = fp
            out.append(beat_seg)
            continue

        matched_any = True
        # Weight panels by their narration length so talky panels linger and
        # silent ones pass quickly; give every panel a small minimum hold.
        weights = [max(len((pb.get("text") or "").strip()), 8) for pb in page_beats]
        total_w = sum(weights) or 1
        cursor = start
        for i, pb in enumerate(page_beats):
            if i == len(page_beats) - 1:
                seg_end = end  # last panel absorbs rounding to hit the page end exactly
            else:
                seg_end = cursor + int(span * weights[i] / total_w)
            seg_end = max(cursor, seg_end)
            beat_seg = {
                "page": page,
                "panel": pb.get("panel"),
                "panel_image": pb.get("panel_image"),
                "text": pb.get("text", ""),
                "audio_file": seg.get("audio_file"),
                "start_ms": cursor,
                "end_ms": seg_end,
                # Offset + length within THIS page's single audio file, so the
                # renderer can play just this panel's slice of the page audio.
                "audio_offset_ms": cursor - start,
                "duration_ms": seg_end - cursor,
            }
            if pb.get("focus_point"):
                beat_seg["focus_point"] = pb["focus_point"]
            out.append(beat_seg)
            cursor = seg_end

    return out if matched_any else []

def _get_audio_duration_ms(audio_path):
    """Get audio duration in milliseconds using wave module."""
    try:
        import wave
        with wave.open(audio_path, 'r') as wf:
            frames = wf.getnframes()
            rate = wf.getframerate()
            return int(frames / rate * 1000)
    except Exception:
        # Fallback: estimate from file size (rough for WAV)
        try:
            size = os.path.getsize(audio_path)
            # Assume 16-bit mono 22050Hz WAV
            return int(size / (22050 * 2) * 1000)
        except:
            return 5000  # Default 5 seconds
