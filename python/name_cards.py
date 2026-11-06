"""Series mode (beta): the first time a character speaks in a chapter, their name comes up on a
card like a TV series introduces its cast: the name in capitals with a short gold rule beside it,
top left (clear of the captions at the bottom and the logo top right), sliding in and fading out
over about three seconds.

The cards are ASS events. They join the chapter's caption track (`captions.ass`, its style `W`
and inline tags, like every caption piece), or, with captions off, a track of their own, so both
render engines burn them exactly as they burn the captions. A clip carries the card on its
`name_card` (the character's name, set by mv_recap's export on their first line).
"""
import os

# How long a card shows, and at least (a short first line still gets a readable card).
CARD_MS = 3000
MIN_MS = 1800
# Fades and the slide in from the left.
FADE_IN_MS, FADE_OUT_MS, SLIDE_MS = 250, 450, 380
# Gold rule beside the name (ASS colours are &HBBGGRR&).
RULE_COLOR = "&H4AB0E8&"
FONT = "Inter ExtraBold"


def _ts(ms: int) -> str:
    cs = max(0, int(round(ms / 10)))
    return f"{cs // 360000}:{(cs % 360000) // 6000:02d}:{(cs % 6000) // 100:02d}.{cs % 100:02d}"


def _esc(text: str) -> str:
    return text.replace("\\", "\\\\").replace("{", "(").replace("}", ")").replace("\n", " ").strip()


def card_events(segments: list, width: int, height: int) -> list:
    """`Dialogue:` lines (style `W`) of a card for every segment that names one. Each name is
    carded once, at its first segment; a card ends at the next card's start so two never stack."""
    starts = []
    seen = set()
    for seg in segments:
        name = _esc(str(seg.get("name_card") or ""))
        if not name or name.lower() in seen:
            continue
        seen.add(name.lower())
        start = int(seg.get("start_ms") or 0)
        end = max(int(seg.get("end_ms") or start) + 400, start + MIN_MS)
        starts.append((start, min(end, start + CARD_MS), name))
    starts.sort()
    u = height / 1080.0
    x, y = round(width * 0.05), round(height * 0.085)
    size, spacing = round(54 * u), round(6 * u)
    rule_w, rule_h = max(2, round(6 * u)), round(62 * u)
    gap = round(22 * u)
    lines = []
    for k, (a, b, name) in enumerate(starts):
        if k + 1 < len(starts):
            b = min(b, starts[k + 1][0])
        if b - a < 600:
            continue
        fade = f"\\fad({FADE_IN_MS},{FADE_OUT_MS})"
        shadow = f"\\bord0\\shad{max(1, round(2 * u))}\\4c&H000000&\\4a&H60&"
        # The rule: a filled rectangle drawn at the card's left edge.
        rule = f"{{\\an7\\pos({x},{y}){fade}\\bord0\\shad0\\1c{RULE_COLOR}\\p1}}m 0 0 l {rule_w} 0 l {rule_w} {rule_h} l 0 {rule_h}{{\\p0}}"
        # The name, left-aligned, vertically centred on the rule, sliding in beside it.
        tx, ty = x + rule_w + gap, y + rule_h // 2
        text = (f"{{\\an4\\move({tx - round(40 * u)},{ty},{tx},{ty},0,{SLIDE_MS}){fade}\\fn{FONT}\\fs{size}\\fsp{spacing}"
                f"\\1c&HFFFFFF&{shadow}}}{name.upper()}")
        lines.append(f"Dialogue: 7,{_ts(a)},{_ts(b)},W,,0,0,0,,{rule}")
        lines.append(f"Dialogue: 7,{_ts(a)},{_ts(b)},W,,0,0,0,,{text}")
    return lines


HEADER = (
    "[Script Info]\n; MangaVox series name cards\nScriptType: v4+\nWrapStyle: 2\nScaledBorderAndShadow: yes\n"
    "YCbCr Matrix: TV.709\nPlayResX: {w}\nPlayResY: {h}\n\n[V4+ Styles]\n"
    "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, "
    "StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n"
    "Style: W,Arial,48,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,5,0,0,0,1\n\n"
    "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
)


def add_cards(ass_path, segments: list, width: int, height: int, output_dir: str):
    """The chapter's burn-in track with its name cards: added to the caption track at `ass_path`
    (whose `[Events]` come last), or written to `namecards.ass` in `output_dir` when there are no
    captions. Returns the track to burn (`ass_path` unchanged when no segment names a card)."""
    lines = card_events(segments, width, height)
    if not lines:
        return ass_path
    if ass_path and os.path.isfile(ass_path):
        with open(ass_path, "a", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
        return ass_path
    path = os.path.join(output_dir, "namecards.ass")
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(HEADER.format(w=width, h=height))
        fh.write("\n".join(lines) + "\n")
    return path
