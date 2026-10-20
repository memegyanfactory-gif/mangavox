#!/usr/bin/env python3
"""Caption (subtitle) generation from the sync map.

Produces an SRT whose cues are driven by the same beat timing the video uses, so
the on-screen text is *guaranteed* in sync with both the panel and the audio
("whatever is in the image is written beside it"). Each beat is further split
into short word-chunked lines whose time is distributed across the beat, giving
a karaoke-like line-level reveal without needing a heavy forced-aligner.

If `stable-ts` / `whisperx` happen to be installed, refine_word_timings() can be
swapped in for true word-level timestamps — but the default path has no extra
dependency and never breaks the pipeline.
"""

from __future__ import annotations

import os

MAX_WORDS_PER_LINE = 8
MIN_CUE_MS = 600


def _fmt_ts(ms: int) -> str:
    ms = max(0, int(ms))
    h = ms // 3_600_000
    m = (ms % 3_600_000) // 60_000
    s = (ms % 60_000) // 1000
    msec = ms % 1000
    return f"{h:02d}:{m:02d}:{s:02d},{msec:03d}"


def _chunk_words(text: str, max_words: int = MAX_WORDS_PER_LINE) -> list[str]:
    words = text.split()
    if not words:
        return []
    return [" ".join(words[i:i + max_words]) for i in range(0, len(words), max_words)]


def build_cues(segments: list[dict]) -> list[dict]:
    """Turn timed segments (beats or pages) into short caption cues."""
    cues: list[dict] = []
    for seg in segments or []:
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        start = int(seg.get("start_ms", 0))
        end = int(seg.get("end_ms", start))
        span = max(MIN_CUE_MS, end - start)
        lines = _chunk_words(text)
        if not lines:
            continue
        # Distribute the segment's time across its word-chunked lines by length.
        weights = [max(len(l), 1) for l in lines]
        total_w = sum(weights) or 1
        cursor = start
        for i, line in enumerate(lines):
            cue_end = end if i == len(lines) - 1 else cursor + int(span * weights[i] / total_w)
            cue_end = max(cursor + 1, cue_end)
            cues.append({"start_ms": cursor, "end_ms": cue_end, "text": line})
            cursor = cue_end
    return cues


def write_captions(segments: list[dict], output_dir: str, filename: str = "captions.srt") -> str | None:
    """Write an SRT file from the timed segments. Returns the path, or None."""
    cues = build_cues(segments)
    if not cues:
        return None
    path = os.path.join(output_dir, filename)
    with open(path, "w", encoding="utf-8") as fh:
        for i, cue in enumerate(cues, start=1):
            fh.write(f"{i}\n{_fmt_ts(cue['start_ms'])} --> {_fmt_ts(cue['end_ms'])}\n{cue['text']}\n\n")
    return path


def write_styled_captions(segments: list[dict], output_dir: str, width: int, height: int,
                          config: dict | None = None, filename: str = "captions.ass") -> str | None:
    """Burn-in caption track in the chosen Watchfiwn caption template (caption_engine.py).

    `segments` are the clips on the rendered timeline ({text, start_ms, end_ms, audio_path,
    audio_offset_ms}); words are timed against their audio. The template, size, position and
    colours come from `config["subtitles"]`, the chapter's subtitles.json or the Studio default.
    Returns the .ass path, or None when captions are off or there is no text.
    """
    import caption_engine

    settings = caption_engine.resolve_settings(config, output_dir)
    path = os.path.join(output_dir, filename)
    if not settings["enabled"]:
        if os.path.exists(path):
            try:
                os.unlink(path)
            except OSError:
                pass
        return None
    return caption_engine.write_ass(segments, path, width, height, settings)


# Legacy colour themes / animations for write_ass_captions (kept for older callers).
COLOR_THEMES = {
    "Neon Cyan":        ("&H00FFFF00", "&H0000FFFF", "&H00000000", "&H00111111"),
    "Neon Yellow":      ("&H0000FFFF", "&H000000FF", "&H00000000", "&H00000000"),
    "Hot Pink":         ("&H00FF00FF", "&H00FFFFFF", "&H001A001A", "&H00000000"),
    "Vibrant Orange":   ("&H0000A5FF", "&H000000FF", "&H00000000", "&H00000000"),
    "Electric Lime":    ("&H0000FF00", "&H0000FFFF", "&H00000000", "&H00000000"),
    "Classic White":    ("&H00FFFFFF", "&H0000FFFF", "&H00000000", "&H00000000"),
    "Sleek Gold":       ("&H0000D7FF", "&H000000FF", "&H00000000", "&H00000000"),
    "Cherry Red":       ("&H000000FF", "&H00FFFFFF", "&H00000000", "&H00000000"),
    "Sky Blue":         ("&H00FFCC66", "&H00000000", "&H00000000", "&H00000000"),
    "Mint Green":       ("&H0099FF99", "&H00000000", "&H00000000", "&H00000000"),
    "Soft Lavender":    ("&H00FFCCFF", "&H00000000", "&H00000000", "&H00000000"),
    "Sunset Orange":    ("&H0033CCFF", "&H00000000", "&H00000000", "&H00000000"),
    "Retro Cyberpunk":  ("&H00FF0099", "&H00FFFF00", "&H00000000", "&H00000000"),
    "Toxic Waste":      ("&H0033FF33", "&H000000FF", "&H00000000", "&H00000000"),
    "Ice Blue":         ("&H00FFFF99", "&H00000000", "&H00000000", "&H00000000"),
    "Flaming Amber":    ("&H000066FF", "&H00000000", "&H00000000", "&H00000000"),
    "Deep Purple":      ("&H00CC0099", "&H00000000", "&H00000000", "&H00000000"),
    "Royal Emerald":    ("&H0066CC00", "&H00000000", "&H00000000", "&H00000000"),
    "Dark Charcoal":    ("&H00CCCCCC", "&H00000000", "&H00000000", "&H00000000"),
    "Rose Quartz":      ("&H00B3B3FF", "&H00000000", "&H00000000", "&H00000000"),
}


def _fmt_ass_ts(ms: int) -> str:
    ms = max(0, int(ms))
    h = ms // 3_600_000
    m = (ms % 3_600_000) // 60_000
    s = (ms % 60_000) // 1000
    cs = (ms % 1000) // 10
    return f"{h}:{m:02d}:{s:02d}.{cs:02d}"


def write_ass_captions(segments: list[dict], output_dir: str, theme_name: str = "Classic White", anim_style: str = "Karaoke Sweep", filename: str = "captions.ass", width: int = 1920, height: int = 1080) -> str | None:
    """Write an ASS file from the timed segments with advanced styles/animations.

    An upright frame (height > width, i.e. reels) uses a 1080x1920 canvas, bigger
    text and sits the captions higher, clear of the platforms' buttons and titles."""
    cues = build_cues(segments)
    if not cues:
        return None

    theme = COLOR_THEMES.get(theme_name, COLOR_THEMES["Classic White"])
    primary, secondary, outline, back = theme

    upright = height > width
    res_x, res_y = (1080, 1920) if upright else (1920, 1080)
    cx = res_x // 2
    # Baseline of the bottom-centred text.
    by = int(res_y * 0.72) if upright else res_y - 70
    font_size = 58 if upright else 28

    path = os.path.join(output_dir, filename)
    with open(path, "w", encoding="utf-8") as fh:
        # Script Info
        fh.write("[Script Info]\n")
        fh.write("; Script generated by MangaVox\n")
        fh.write("Title: MangaVox Styled Captions\n")
        fh.write("ScriptType: v4+\n")
        fh.write(f"PlayResX: {res_x}\n")
        fh.write(f"PlayResY: {res_y}\n")
        fh.write("WrapStyle: 0\n\n")

        # Styles
        fh.write("[V4+ Styles]\n")
        fh.write("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n")
        # Alignment=2 is bottom center
        fh.write(f"Style: Default,Arial,{font_size},{primary},{secondary},{outline},{back},-1,0,0,0,100,100,0,0,1,3,1,2,40,40,{res_y - by},1\n\n")

        # Events
        fh.write("[Events]\n")
        fh.write("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n")

        for idx, cue in enumerate(cues):
            start_ms = cue["start_ms"]
            end_ms = cue["end_ms"]
            text = cue["text"]
            span = max(10, end_ms - start_ms)

            # Apply selected animation style tags
            if anim_style in ("Karaoke Sweep", "Karaoke Pop"):
                tag = "\\kf" if anim_style == "Karaoke Sweep" else "\\k"
                words = text.split()
                if not words:
                    formatted_text = text
                else:
                    total_chars = sum(max(1, len(w)) for w in words)
                    word_parts = []
                    for w in words:
                        # calculate duration in centiseconds
                        w_len = max(1, len(w))
                        w_csec = max(1, int((span * w_len / total_chars) / 10))
                        word_parts.append(f"{{{tag}{w_csec}}}{w}")
                    formatted_text = " ".join(word_parts)
            elif anim_style == "Fade In":
                formatted_text = f"{{\\fad(150,150)}}{text}"
            elif anim_style == "Bounce Pop":
                formatted_text = f"{{\\t(0,150,\\fscx115\\fscy115)\\t(150,300,\\fscx100\\fscy100)}}{text}"
            elif anim_style == "Slide Up":
                formatted_text = f"{{\\an2\\move({cx},{by + 40},{cx},{by},0,250)}}{text}"
            elif anim_style == "Slide Down":
                formatted_text = f"{{\\an2\\move({cx},{by - 50},{cx},{by},0,250)}}{text}"
            elif anim_style == "Zoom Blur":
                formatted_text = f"{{\\fscx50\\fscy50\\blur2\\t(0,250,\\fscx100\\fscy100\\blur0)}}{text}"
            elif anim_style == "Elastic Bounce":
                formatted_text = f"{{\\fscx20\\fscy20\\t(0,150,\\fscx120\\fscy120)\\t(150,250,\\fscx90\\fscy90)\\t(250,350,\\fscx100\\fscy100)}}{text}"
            elif anim_style == "Rotate Spin":
                formatted_text = f"{{\\frz-15\\t(0,250,\\frz0)}}{text}"
            elif anim_style == "Fade Zoom":
                formatted_text = f"{{\\fad(200,0)\\fscx80\\fscy80\\t(0,200,\\fscx100\\fscy100)}}{text}"
            elif anim_style == "Glitch Flash":
                formatted_text = f"{{\\c&H00FFFF00\\t(0,100,\\c&H00FF00FF)\\t(100,200,\\c&H00FFFFFF)}}{text}"
            elif anim_style == "Letter Spacing Stretch":
                formatted_text = f"{{\\fsp10\\t(0,300,\\fsp0)}}{text}"
            elif anim_style == "Shake":
                formatted_text = f"{{\\t(0,100,\\frz-2)\\t(100,200,\\frz2)\\t(200,300,\\frz-1)\\t(300,400,\\frz1)\\t(400,500,\\frz0)}}{text}"
            elif anim_style == "Slide Left":
                formatted_text = f"{{\\an2\\move(0,{by},{cx},{by},0,250)}}{text}"
            elif anim_style == "Slide Right":
                formatted_text = f"{{\\an2\\move({res_x},{by},{cx},{by},0,250)}}{text}"
            elif anim_style == "Blur Fade":
                formatted_text = f"{{\\fad(150,150)\\blur0\\t({span-200},{span},\\blur4)}}{text}"
            elif anim_style == "Double Outline Pulse":
                formatted_text = f"{{\\t(0,200,\\bord5)\\t(200,400,\\bord3)}}{text}"
            elif anim_style == "Slow Rise":
                formatted_text = f"{{\\an2\\move({cx},{by + 10},{cx},{by - 30},0,{span})}}{text}"
            elif anim_style == "Fade Pulse":
                mid_span = int(span / 2)
                formatted_text = f"{{\\fad(100,100)\\t(0,{mid_span},\\alpha&H80)\\t({mid_span},{span},\\alpha&H00)}}{text}"
            elif anim_style == "Word-by-word Reveal":
                # uses standard karaoke sweep but acts as simple word highlights
                words = text.split()
                if not words:
                    formatted_text = text
                else:
                    total_chars = sum(max(1, len(w)) for w in words)
                    word_parts = []
                    for w in words:
                        w_len = max(1, len(w))
                        w_csec = max(1, int((span * w_len / total_chars) / 10))
                        word_parts.append(f"{{\\k{w_csec}}}{w}")
                    formatted_text = " ".join(word_parts)
            else:
                formatted_text = text

            start_str = _fmt_ass_ts(start_ms)
            end_str = _fmt_ass_ts(end_ms)
            fh.write(f"Dialogue: 0,{start_str},{end_str},Default,,0,0,0,,{formatted_text}\n")

    return path

