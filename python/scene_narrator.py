#!/usr/bin/env python3
"""Whole-chapter scene narrator.

Reads every page of a chapter, batches them into vision API calls, and produces
one continuous English prose narration. After every batch, the model also
emits a structured memory delta (new characters, aliases, facts, running
summary). That delta merges into the per-manga memory file so character names
and continuity carry across chapters.

Designed to handle chapters of any length (we've seen 163-page raw uploads):
- Gemini: ~12 images per request, up to ~120 image quota per call.
- Ollama: 4 images per request to keep response time / context size sane.

Memory schema (managed by src/main/services/memory-store.ts but read here):
  {
    "mangaId": "...",
    "characters": [{ "name": "...", "aliases": [...], "description": "...",
                     "firstSeenChapter": 12 }],
    "facts":      [{ "text": "...", "chapter": 12 }],
    "storySoFar": "...",
    "chaptersNarrated": [1, 2, ...]
  }

Job payload (written by text.ipc.ts):
  {
    "pages_dir":        "...",          # absolute path to chapter pages
    "output_dir":       "...",          # where to write narration.json
    "memory_path":      "...",          # absolute path to memory JSON
    "manga_id":         "...",
    "chapter_number":   12,             # nullable
    "provider":         "gemini" | "ollama" | "g0i",
    "gemini_api_key":   "...",          # required for provider=gemini
    "gemini_model":     "gemini-2.0-flash",
    "g0i_api_key":      "...",          # required for provider=g0i
    "g0i_model":        "gemini-2.5-flash" | "claude-sonnet-4-6" | "gpt-4o" | ...,
    "g0i_family":       "gemini" | "claude" | "openai",
    "g0i_base_url":     "https://api.g0i.ai/v1",
    "ollama_url":       "http://127.0.0.1:11434",
    "ollama_model":     "qwen2.5vl:7b",
    "batch_size":       12,             # optional
    "temperature":      0.55
  }

Output (written to {output_dir}/narration.json):
  {
    "chapter": <int or null>,
    "narration": "<continuous English prose>",
    "batches":  [{ "from": 1, "to": 12, "text": "..." }, ...],
    "model":    "gemini-2.0-flash",
    "pages":    <int>
  }
"""

from __future__ import annotations

import base64
import io
import json
import os
import re
import sys
import time
import traceback
import urllib.error
import urllib.request
from typing import Any

try:
    from PIL import Image  # type: ignore
except Exception:                                       # pragma: no cover
    Image = None  # PIL only needed for downscaling — proceed without it if missing

try:
    # Sibling module — builds panel-level beats so audio/video sync to panels.
    from panel_beats import build_beats  # type: ignore
except Exception:                                       # pragma: no cover
    build_beats = None


# ────────────────────────────────────────────────────────────────────────────
# stdout protocol — every line is one JSON message consumed by text.ipc.ts
# ────────────────────────────────────────────────────────────────────────────

TOTAL_TOKENS_USED = 0

def emit(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()

START_TIME = None
START_INDEX = 0

def init_progress_timer(start_index: int) -> None:
    global START_TIME, START_INDEX
    START_TIME = time.time()
    START_INDEX = start_index

def emit_progress(current: int, total: int, label: str = "") -> None:
    global START_TIME, START_INDEX, TOTAL_TOKENS_USED
    pct = int(round(100.0 * current / total)) if total else 0
    payload = {
        "type": "progress",
        "current": current,
        "total": total,
        "percent": pct,
        "label": label,
        "token_usage": TOTAL_TOKENS_USED,
        "tokenUsage": TOTAL_TOKENS_USED
    }
    
    # Calculate estimated remaining seconds
    if START_TIME is not None and current > START_INDEX:
        elapsed = time.time() - START_TIME
        done_steps = current - START_INDEX
        steps_left = total - current
        if done_steps > 0:
            sec_per_step = elapsed / done_steps
            payload["estimated_remaining_seconds"] = int(sec_per_step * steps_left)
            
    emit(payload)

def emit_log(msg: str) -> None:
    emit({"type": "log", "message": msg})

def emit_batch(batch_index: int, page_from: int, page_to: int, text: str, provider: str | None = None) -> None:
    global TOTAL_TOKENS_USED
    emit({
        "type": "batch",
        "batch_index": batch_index,
        "page_from": page_from,
        "page_to": page_to,
        "text": text,
        "token_usage": TOTAL_TOKENS_USED,
        "tokenUsage": TOTAL_TOKENS_USED,
        **({"provider": provider} if provider else {}),
    })

def emit_error(msg: str) -> None:
    emit({"type": "error", "message": msg})

def emit_done(payload: dict) -> None:
    global TOTAL_TOKENS_USED
    emit({"type": "done", "token_usage": TOTAL_TOKENS_USED, "tokenUsage": TOTAL_TOKENS_USED, **payload})


# ────────────────────────────────────────────────────────────────────────────
# Image helpers
# ────────────────────────────────────────────────────────────────────────────

PAGE_EXTS = (".jpg", ".jpeg", ".png", ".webp")
# Gemini happily takes large images; local Ollama vision models are memory
# constrained and crash ("llama runner process has terminated") on big inputs,
# so we downscale much harder for Ollama.
MAX_DIM_GEMINI = 1280
MAX_DIM_OLLAMA = 896
# g0i.ai charges per pixel for Claude/GPT, so cap a bit smaller than Gemini.
MAX_DIM_G0I = 1152

def list_pages(pages_dir: str, use_raw_pages: bool = True) -> list[str]:
    # If the chapter was run through the strip-splitter, `pages/` holds only the
    # crops (0001_a.jpg, 0001_b.jpg, …) and the FULL originals live in a sibling
    # `pages_original/` (0001.jpg, …). Reading the crops fragments the story, so
    # prefer the full pages when they exist.
    chapter_dir = os.path.dirname(pages_dir.rstrip("/"))
    original_dir = os.path.join(chapter_dir, "pages_original")
    full_re = re.compile(r"^\d+\.(jpg|jpeg|png|webp)$", re.IGNORECASE)

    def collect(dir_path: str, full_only: bool) -> list[str]:
        if not os.path.isdir(dir_path):
            return []
        names = [
            f for f in os.listdir(dir_path)
            if f.lower().endswith(PAGE_EXTS) and not f.startswith(".")
        ]
        if full_only:
            # Keep only base-numbered full pages (0001.jpg), drop crops (0001_a.jpg).
            full = [f for f in names if full_re.match(f)]
            if full:  # only use the filter if it actually found full pages
                names = full
        names.sort()
        return [os.path.join(dir_path, f) for f in names]

    if use_raw_pages:
        pages = collect(original_dir, full_only=True)
        if pages:
            return pages
        return collect(pages_dir, full_only=True)
    else:
        return collect(pages_dir, full_only=False)

def encode_page(path: str, max_dim: int = 1280) -> tuple[str, str]:
    """Return (base64 string, mime). Downscale if PIL is available."""
    mime = "image/jpeg"
    if path.lower().endswith(".png"): mime = "image/png"
    elif path.lower().endswith(".webp"): mime = "image/webp"

    if Image is not None:
        try:
            img = Image.open(path)
            if img.mode in ("RGBA", "P", "LA"):
                img = img.convert("RGB")
                mime = "image/jpeg"
            w, h = img.size
            scale = max_dim / max(w, h)
            if scale < 1.0:
                img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)
            buf = io.BytesIO()
            if mime == "image/png":
                img.save(buf, format="PNG", optimize=True)
            else:
                img.save(buf, format="JPEG", quality=85, optimize=True)
            return base64.b64encode(buf.getvalue()).decode("ascii"), mime
        except Exception:
            pass  # fall through to raw read

    with open(path, "rb") as fh:
        return base64.b64encode(fh.read()).decode("ascii"), mime


# ────────────────────────────────────────────────────────────────────────────
# Memory I/O (same shape as src/main/services/memory-store.ts)
# ────────────────────────────────────────────────────────────────────────────

def load_memory(memory_path: str, manga_id: str) -> dict:
    if not os.path.exists(memory_path):
        return _empty_memory(manga_id)
    try:
        with open(memory_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
        return {
            "mangaId":          manga_id,
            "characters":       data.get("characters") or [],
            "facts":            data.get("facts") or [],
            "storySoFar":       data.get("storySoFar") or "",
            "chaptersNarrated": data.get("chaptersNarrated") or [],
            "updatedAt":        data.get("updatedAt") or "",
        }
    except Exception:
        return _empty_memory(manga_id)

def _empty_memory(manga_id: str) -> dict:
    return {
        "mangaId": manga_id, "characters": [], "facts": [],
        "storySoFar": "", "chaptersNarrated": [], "updatedAt": "",
    }

def save_memory(memory_path: str, mem: dict) -> None:
    os.makedirs(os.path.dirname(memory_path), exist_ok=True)
    with open(memory_path, "w", encoding="utf-8") as fh:
        json.dump(mem, fh, ensure_ascii=False, indent=2)

def attach_beats(payload: dict, pages_dir: str, batches: list) -> None:
    """Compute panel-level beats and add them to the narration payload."""
    if build_beats is None:
        return
    try:
        beats = build_beats(pages_dir, batches)
        payload["beats"] = beats
        emit_log(f"narrator: built {len(beats)} panel beats for sync")
    except Exception as e:
        emit_log(f"warn: panel beat build failed: {e}")


def merge_delta(mem: dict, delta: dict, chapter_number: int | None) -> dict:
    """Same merge semantics as memory-store.ts mergeMemoryDelta."""
    mem = {**mem, "characters": list(mem.get("characters", [])), "facts": list(mem.get("facts", []))}

    for incoming in delta.get("characters") or []:
        name = (incoming.get("name") or "").strip()
        if not name: continue
        key = name.lower()
        match = None
        for c in mem["characters"]:
            if c.get("name", "").lower() == key:
                match = c; break
            for a in c.get("aliases") or []:
                if a.lower() == key:
                    match = c; break
            if match: break
        if match:
            existing_aliases = {a.lower() for a in match.get("aliases") or []}
            existing_aliases.add(match.get("name", "").lower())
            for a in incoming.get("aliases") or []:
                if a and a.lower() not in existing_aliases:
                    match.setdefault("aliases", []).append(a)
                    existing_aliases.add(a.lower())
            inc_desc = (incoming.get("description") or "").strip()
            if inc_desc and len(inc_desc) > len(match.get("description", "")):
                match["description"] = inc_desc
        else:
            mem["characters"].append({
                "name": name,
                "aliases": [a for a in (incoming.get("aliases") or []) if a],
                "description": (incoming.get("description") or "").strip(),
                "firstSeenChapter": chapter_number,
            })

    for f in delta.get("facts") or []:
        text = (f.get("text") or "").strip()
        if not text: continue
        if any((existing.get("text") or "").strip() == text for existing in mem["facts"]):
            continue
        mem["facts"].append({"text": text, "chapter": chapter_number})

    story = delta.get("storySoFar") or ""
    if isinstance(story, str) and story.strip():
        mem["storySoFar"] = story.strip()

    if chapter_number is not None:
        if chapter_number not in mem.get("chaptersNarrated", []):
            mem.setdefault("chaptersNarrated", []).append(chapter_number)
            mem["chaptersNarrated"].sort()
    return mem


def load_ocr_for_page(output_dir: str, page_num: int) -> str | None:
    """Load pre-extracted OCR text for a page from text_edited.json or vision_results.json."""
    for filename in ["text_edited.json", "vision_results.json"]:
        path = os.path.join(output_dir, filename)
        if os.path.exists(path):
            try:
                with open(path, "r", encoding="utf-8") as fh:
                    data = json.load(fh)
                segments = data.get("segments") if isinstance(data, dict) else data
                if isinstance(segments, list):
                    page_texts = []
                    for seg in segments:
                        seg_page = seg.get("page")
                        if seg_page is not None:
                            try:
                                if int(seg_page) == page_num:
                                    txt = seg.get("text") or ""
                                    if txt.strip():
                                        page_texts.append(txt.strip())
                            except (ValueError, TypeError):
                                pass
                    if page_texts:
                        return "\n".join(page_texts)
            except Exception as e:
                emit_log(f"warn: failed to load OCR from {path}: {e}")
    return None


def parse_focus_points(text: str) -> list[list[int]]:
    """Look for focus coordinates in the format [ymin, xmin, ymax, xmax] inside ---FOCUS--- block."""
    focus_points = []
    if not text:
        return focus_points
    focus_part = ""
    if "---FOCUS---" in text:
        focus_part = text.split("---FOCUS---")[-1]
    else:
        focus_part = text
    matches = re.findall(r"\[\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\]", focus_part)
    for m in matches:
        try:
            coords = [int(x) for x in m]
            if all(0 <= val <= 1000 for val in coords):
                if coords[0] < coords[2] and coords[1] < coords[3]:
                    focus_points.append(coords)
        except Exception:
            pass
    return focus_points


def parse_multi_page_focus_points(text: str) -> dict[str, list[list[int]]]:
    """Parse focus points for multiple pages, e.g. from Gemini's output."""
    out = {}
    if not text:
        return out
    focus_part = ""
    if "---FOCUS---" in text:
        focus_part = text.split("---FOCUS---")[-1]
    else:
        focus_part = text
    for line in focus_part.splitlines():
        m_page = re.search(r"(?:Page\s+)?(\d+)\s*:", line)
        if m_page:
            pg = m_page.group(1)
            coords_list = []
            matches = re.findall(r"\[\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\]", line)
            for m in matches:
                try:
                    coords = [int(x) for x in m]
                    if all(0 <= val <= 1000 for val in coords):
                        if coords[0] < coords[2] and coords[1] < coords[3]:
                            coords_list.append(coords)
                except Exception:
                    pass
            if coords_list:
                out[pg] = coords_list
    return out


# ────────────────────────────────────────────────────────────────────────────
# Prompt construction
# ────────────────────────────────────────────────────────────────────────────

SYSTEM_PROMPT = """You are a manga scene narrator producing one continuous English prose narration of a chapter as a human reader would experience it.

LANGUAGE
- Output STRICTLY in English. Translate any Japanese, Korean, Chinese or other source text to natural English before quoting.

NARRATION STYLE
- Past tense, cinematic third-person. No "Behold!" / "Lo!" / "Witness!"
- Describe action AND visual mood — facial expressions, posture, weather, environment — even on pages with no dialogue. Silent panels are scenes too; narrate the beat.
- Smooth panel-to-panel transitions ("Then,", "Across the room,", "A heartbeat passed,"). Vary them.
- Keep every line of original dialogue intact, in quotes, with the speaker named when known (e.g. Sora smirked. "Watch this."). Use voice-from-the-shadows phrasing if the speaker is hidden.
- When pages have NO text: narrate what's happening visually anyway. Pure visual scenes are not skipped.

PACING & BREVITY
- If a page or panel is not very interesting or is just a transition (e.g., landscapes, simple walking, characters standing in silence without key expressions/events, panel backgrounds), make it a very small, brief narration for that page/panel. Do not over-explain or add unnecessary filler text. Linger only on important visual or emotional narrative beats.

CHARACTER CONSISTENCY (CRITICAL)
- The KNOWN CHARACTERS block below lists names already established in this series. ALWAYS use those exact names when you recognize a character — never invent a new name for an existing one.
- If you can't tell who a character is, say "a tall figure" / "the silver-haired boy" / "the masked stranger" rather than guessing a new name.
- If a NEW named character appears (their name is shown in dialogue / on-screen text), introduce them once by name and add them to the memory delta.

STRUCTURE
- One flowing prose narration for the whole batch — NOT page headers, NOT bullet points.
- Each batch picks up where the previous batch's STORY SO FAR left off. Don't re-introduce known characters.

MEMORY DELTA (REQUIRED)
After your narration, on a new line, output exactly:
---MEMORY---
{ "characters": [{ "name": "Name", "aliases": [...], "description": "1-sentence trait" }],
  "facts":      [{ "text": "important plot fact"} ],
  "storySoFar": "One updated paragraph summarising the story through the end of THIS batch." }

Only include characters / facts that are NEW this batch. `storySoFar` is always required.
The JSON must be valid. Nothing after the closing brace.

FOCUS POINTS (REQUIRED)
At the very end of your response, after the MEMORY block, output exactly:
---FOCUS---
Page <number>: [ymin, xmin, ymax, xmax], [ymin, xmin, ymax, xmax]
Page <number>: [ymin, xmin, ymax, xmax]

Use normalized coordinates [ymin, xmin, ymax, xmax] from 0 to 1000 where [0,0,1000,1000] is the whole page. Bounding boxes should cover active characters, text bubbles, or visual focus areas. List coordinates for each page in the batch in order."""

HINDI_LANGUAGE_NOTE = """

LANGUAGE OVERRIDE
- Output ONLY natural Hindi written in Devanagari.
- Never repeat an English sentence with a Hindi translation in parentheses, or a Hindi sentence with an English translation.
- Latin text is allowed only for established character names, proper nouns, acronyms, and special attack names that cannot be translated naturally.
- Keep character names stable.
- The MEMORY JSON keys must stay in English exactly as requested. Write storySoFar, descriptions, and facts in Hindi except for allowed proper nouns."""

def prompt_for_language(base: str, language: str) -> str:
    """Create one non-contradictory language prompt."""
    if language != "hindi":
        return base
    prompt = base
    prompt = prompt.replace(
        "producing one continuous English prose narration",
        "producing one continuous Hindi prose narration",
    )
    prompt = prompt.replace(
        "Retell it as one continuous, immersive English narration",
        "Retell it as one continuous, immersive Hindi narration",
    )
    prompt = re.sub(
        r"LANGUAGE\n- Output STRICTLY in English\.[^\n]*\n",
        "LANGUAGE\n- Output ONLY natural Hindi in Devanagari. Translate all source dialogue into natural Hindi.\n",
        prompt,
    )
    return prompt + HINDI_LANGUAGE_NOTE

# Tolerant: matches ---MEMORY---, --- MEMORY ---, and the common
# "---\n\nMEMORY---" the model emits (dashes, whitespace/newlines, MEMORY).
MEMORY_RE = re.compile(r"-{2,}\s*MEMORY\s*-{2,}?\s*(\{.*\})", re.DOTALL | re.IGNORECASE)
# Fallback: a bare "MEMORY---" or "MEMORY" line directly before a JSON object.
MEMORY_RE_LOOSE = re.compile(r"\bMEMORY\b[\s\-]*?(\{[\s\S]*\})", re.IGNORECASE)

# Page/panel references that break immersion — stripped post-hoc since models
# leak them no matter how firmly the prompt forbids it.
_OF_PAGE = r"(?:\s+of\s+(?:this|the)?\s*pages?(?:\s+\d+)?)?"
_PAGE_REF_PATTERNS = [
    re.compile(r"\bon page \d+[,:]?\s*", re.IGNORECASE),
    # "in (the next) panel (2) (of this/the page (75))"
    re.compile(r"\bin (?:the (?:next|final|first|last|previous|same|second|third|top|bottom)\s+)?panel(?:\s+\d+)?" + _OF_PAGE + r"[,:]?\s*", re.IGNORECASE),
    re.compile(r"\bthe (?:next|final|first|last|previous|top|bottom) panel" + _OF_PAGE + r"[,:]?\s*", re.IGNORECASE),
    re.compile(r"\bin (?:this|the) (?:page|panel)[,:]?\s*", re.IGNORECASE),
    re.compile(r"\b(?:a |the )?(?:closer )?inset panel\b[,:]?\s*", re.IGNORECASE),
    re.compile(r"\bof (?:this|the) page[,:]?\s*", re.IGNORECASE),
    re.compile(r"\bpages?\s+\d+(?:\s*[-–]\s*\d+)?[,:]?\s*", re.IGNORECASE),
]

def clean_narration(text: str) -> str:
    """Strip any leaked memory block + page/panel references from narration."""
    if not text:
        return text
    # Remove a leaked focus section entirely (everything from a FOCUS marker on).
    m_focus = re.search(r"-{2,}\s*FOCUS\b|\bFOCUS\s*-{2,}|\n\s*FOCUS\s*\n", text, re.IGNORECASE)
    if m_focus:
        text = text[:m_focus.start()]
    # Remove a leaked memory section entirely (everything from a MEMORY marker on).
    m = re.search(r"-{2,}\s*MEMORY\b|\bMEMORY\s*-{2,}|\n\s*MEMORY\s*\n*\s*\{", text, re.IGNORECASE)
    if m:
        text = text[:m.start()]
    # Drop standalone "---" separator lines.
    text = re.sub(r"\n\s*-{3,}\s*\n", "\n\n", text)
    # Strip page/panel references, then tidy the leftover capitalisation/space.
    for pat in _PAGE_REF_PATTERNS:
        text = pat.sub("", text)
    # Collapse artefacts like ", the" / double spaces / orphaned leading punctuation.
    text = re.sub(r"[ \t]{2,}", " ", text)
    text = re.sub(r"\n{3,}", "\n\n", text)
    # Re-capitalise sentences that lost their leading word to a stripped phrase.
    text = re.sub(r"(^|\n|\. )([a-z])", lambda mm: mm.group(1) + mm.group(2).upper(), text)
    return text.strip()

def validate_narration(text: str, language: str) -> list[str]:
    """Reject empty, repetitive, generic, or bilingual output before saving."""
    issues: list[str] = []
    clean = clean_narration(text)
    if len(clean) < 80:
        issues.append("narration is empty or too short")
    lower = clean.lower()
    generic = (
        "i can't see the image", "i cannot see the image", "as an ai",
        "please provide the manga", "unable to analyze", "here is the narration",
    )
    if any(marker in lower for marker in generic):
        issues.append("response contains generic non-manga filler")

    sentences = [s.strip().lower() for s in re.split(r"[.!?।]+", clean) if len(s.strip()) > 24]
    if sentences:
        unique_ratio = len(set(sentences)) / len(sentences)
        if len(sentences) >= 4 and unique_ratio < 0.65:
            issues.append("response repeats the same narration")

    devanagari = len(re.findall(r"[\u0900-\u097F]", clean))
    latin_words = re.findall(r"\b[A-Za-z]{3,}\b", clean)
    if language == "hindi":
        if devanagari < max(20, len(clean) * 0.18):
            issues.append("Hindi output is not primarily Devanagari")
        if re.search(r"\([^)]{12,}[A-Za-z]{4,}[^)]*\)", clean):
            issues.append("Hindi output includes an English translation in parentheses")
        if len(latin_words) > max(18, devanagari // 8):
            issues.append("Hindi output contains too much English")
    elif devanagari > max(12, len(clean) * 0.04):
        issues.append("English output contains Hindi duplication")
    return issues

def correction_prompt(language: str, issues: list[str]) -> str:
    target = "natural Hindi in Devanagari only" if language == "hindi" else "English only"
    return (
        "\n\nCORRECTION REQUIRED\n"
        f"The previous answer was rejected because: {', '.join(issues)}. "
        f"Write the complete grounded narration again in {target}. "
        "Use only facts visible in the supplied page descriptions. Do not invent names, "
        "events, motives, or dialogue. Do not include translations in parentheses."
    )

# Stage-1 prompt for Ollama: the vision model ONLY describes the page. Small
# vision models (moondream, llava) are poor narrative writers but decent at
# plain description, so we keep their job simple and let a text model write.
DESCRIBE_SYSTEM = (
    "You are the eyes for a manga narrator. Describe ONE manga/comic page in rich, "
    "concrete detail so a writer who cannot see it can retell the scene faithfully. "
    "Output STRICTLY in English; translate any Japanese/Korean/Chinese text."
)
DESCRIBE_USER = (
    "Describe this page panel by panel, top to bottom (right-to-left within a panel):\n"
    "• CHARACTERS: appearance, hair, clothing, age — enough to recognise them again.\n"
    "• ACTION: what each person is doing, body language, gestures, movement.\n"
    "• EXPRESSIONS & MOOD: facial expressions, emotions, the tone of the moment.\n"
    "• SETTING: location, time of day, weather, background details.\n"
    "• DIALOGUE: every speech bubble and caption, transcribed and translated to English, "
    "with the speaker noted when it's clear.\n"
    "• SOUND/FX: any visible sound effects (and what they imply).\n"
    "NAMES: only state a character's NAME if it is literally written on the page (in a "
    "speech bubble or caption). Otherwise refer to them ONLY by appearance "
    "(e.g. 'the black-haired young man', 'the woman with the ponytail'). NEVER guess or "
    "make up a name.\n"
    "Be specific and literal. Do NOT invent events. Do NOT write a story — just report "
    "what is on the page so the writer has everything they need.\n"
    "• PACING & BREVITY: If the page/panel is not interesting or represents a transition (e.g., landscapes, simple walking, characters standing in silence without key expressions/events, background art), keep the description extremely brief and short (1 small sentence).\n"
    "• FOCUS POINTS: At the end of your response, output a '---FOCUS---' line and then list the coordinate box of the 1 to 3 main visual focus points on the page (active characters, speech bubbles, or focal items). "
    "Use normalized coordinates [ymin, xmin, ymax, xmax] from 0 to 1000 where [0,0,1000,1000] is the whole page. Format exactly like this:\n"
    "---FOCUS---\n"
    "[ymin, xmin, ymax, xmax]\n"
    "[ymin, xmin, ymax, xmax]"
)

# Phase-2 prompt: turn the full set of page descriptions into one in-depth,
# human-like manga narration — the depth a good YouTube manga-recap channel
# delivers. This runs over MANY pages at once for whole-chapter coherence.
NARRATE_SYSTEM = """You are a master manga narrator — the voice of a top manga-recap channel. You are given factual PAGE DESCRIPTIONS for a stretch of a chapter (multiple pages at once). Retell it as one continuous, immersive English narration, exactly as a human reader experiences the story.

LANGUAGE
- Output STRICTLY in English. Translate any non-English dialogue naturally.

DEPTH & STYLE (this is the whole point)
- Narrate scene by scene with real depth: set each scene, describe what characters do, how they look, what their expressions reveal, and the emotional undercurrent.
- Past tense, cinematic third-person. Confident and vivid, never cheesy ("Behold!" is banned).
- Don't just list panels — connect them into flowing story beats with transitions ("Then,", "Across the room,", "A beat passed,"). Convey pacing: linger on big moments, move quickly through filler.
- Bring out subtext: tension, motives, what's left unsaid. Read the scene like a human who understands the story, not a camera.

PACING & BREVITY
- If a page/panel description indicates it is not very interesting or represents a transition (e.g., landscapes, simple walking, characters standing in silence without key expressions/events, background scenery), make it a very small, brief narration for that page/panel. Do not over-explain or add unnecessary filler text. Linger only on important visual or emotional narrative beats.

DIALOGUE
- Weave in the actual dialogue, in quotes, attributed to the speaker ("Watch this," Sora smirked.). Keep the important lines intact; you may trim redundant ones.

CHARACTER CONSISTENCY (CRITICAL — read carefully)
- NEVER invent or guess a character's name. Made-up names that change page to page (Kenji, then Sora, then Kaito for the same person) destroy the narration.
- Use a real name ONLY if it appears in KNOWN CHARACTERS or is explicitly quoted in the dialogue/description.
- For anyone without an established name, refer to them by a CONSISTENT appearance-based label and reuse the SAME label every time ("the black-haired youth", "the woman in the purple coat", "the boy with the rabbit"). Pick the label from KNOWN CHARACTERS if one exists.
- In the MEMORY delta, give unnamed characters a descriptive label as their "name" (e.g. "name": "the black-haired youth") so it stays stable across the chapter — do NOT assign them a fabricated personal name.

STRUCTURE
- One flowing narration covering all the provided pages. Pure prose — no headers, no bullet points.
- NEVER mention "page", "panel", "the page", "the next panel", or any page/panel numbers. The listener experiences a continuous story, not a comic. Convert "in the final panel of page 31" into a story beat ("Then, in the stillness that followed, …"). This is critical — page/panel references break the immersion.
- Pick up smoothly from STORY SO FAR; don't re-introduce known characters.

MEMORY DELTA (REQUIRED)
After the narration, on a new line output exactly:
---MEMORY---
{ "characters": [{ "name": "Name", "aliases": [...], "description": "1-sentence trait" }],
  "facts": [{ "text": "important plot fact" }],
  "storySoFar": "One updated paragraph summarising the story through the end of these pages." }
Only include NEW characters/facts. storySoFar is always required. Valid JSON, nothing after the closing brace."""

def build_user_prompt(
    *, batch_index: int, batch_count: int,
    page_from: int, page_to: int, total_pages: int,
    chapter_number: int | None, memory: dict,
) -> str:
    char_lines = []
    for c in memory.get("characters") or []:
        aliases = c.get("aliases") or []
        alias_part = f" (also: {', '.join(aliases)})" if aliases else ""
        desc = c.get("description") or ""
        char_lines.append(f"- {c.get('name', '')}{alias_part} — {desc}")
    known_block = "\n".join(char_lines) if char_lines else "(none yet — establish names as they appear in dialogue)"

    story_so_far = (memory.get("storySoFar") or "").strip() or "(none yet — this may be the first chapter narrated)"

    chapter_label = f"Chapter {chapter_number}" if chapter_number is not None else "this chapter"
    position = "the OPENING" if batch_index == 0 else ("the FINAL" if batch_index == batch_count - 1 else f"MIDDLE batch {batch_index + 1} of {batch_count}")

    return f"""STORY SO FAR
{story_so_far}

KNOWN CHARACTERS
{known_block}

BATCH POSITION
This is {position} batch of {chapter_label}. It covers pages {page_from}–{page_to} of {total_pages}.
{'Open with a hook line that frames the chapter.' if batch_index == 0 else ''}
{'Close with one cliffhanger / lead-out line that leaves the listener wanting the next chapter.' if batch_index == batch_count - 1 else ''}

Narrate this batch as one continuous prose passage, then emit the ---MEMORY--- delta."""


# ────────────────────────────────────────────────────────────────────────────
# Provider calls
# ────────────────────────────────────────────────────────────────────────────

def token_caps(budget: str | None, reasoning_model: bool = False) -> tuple[int, int]:
    """Return (normal_cap, reasoning_cap) for the chosen spend/length profile."""
    b = (budget or "balanced").lower()
    if b == "low":
        return 4096, 8192
    if b == "high":
        return 8192, 24576 if reasoning_model else 8192
    return 6144, 12288 if reasoning_model else 6144


def call_gemini(api_key: str, model: str, system: str, user: str,
                images: list[tuple[str, str]], temperature: float,
                token_budget: str | None = None) -> str:
    """Returns the model's text response. Images are (base64, mime) pairs."""
    global TOTAL_TOKENS_USED
    model_id = model or "gemini-2.0-flash"
    parts: list[dict] = [{"text": user}]
    for b64, mime in images:
        parts.append({"inlineData": {"mimeType": mime, "data": b64}})
    body = {
        "systemInstruction": {"parts": [{"text": system}]},
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": {
            "temperature": temperature,
            "maxOutputTokens": token_caps(token_budget)[0],
            "topP": 0.9,
        },
    }
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent?key={api_key}"
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="ignore")
        raise RuntimeError(f"Gemini HTTP {e.code}: {detail[:600]}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Gemini network error: {e.reason}")
    data = json.loads(raw)
    
    # Track Gemini tokens
    usage = data.get("usageMetadata") or {}
    if usage:
        prompt = usage.get("promptTokenCount", 0)
        output = usage.get("candidatesTokenCount", 0)
        TOTAL_TOKENS_USED += (prompt + output)

    try:
        return data["candidates"][0]["content"]["parts"][0]["text"]
    except (KeyError, IndexError, TypeError):
        # Surface what the API actually returned so the user can act on it.
        raise RuntimeError(f"Gemini returned no text: {json.dumps(data)[:600]}")

# Module-level holder so the batch loop can read usage telemetry from the
# last g0i call without us threading it through every helper signature.
LAST_G0I_USAGE: dict | None = None
LAST_G0I_FINISH: str | None = None
LAST_OPENROUTER_USAGE: dict | None = None
LAST_OPENROUTER_FINISH: str | None = None
LAST_OPENROUTER_MODEL: str | None = None

def call_openrouter(api_key: str, model: str, base_url: str, system: str, user: str,
                    images: list[tuple[str, str]], temperature: float,
                    token_budget: str | None = None) -> str:
    """Call OpenRouter's OpenAI-compatible /chat/completions endpoint."""
    global LAST_OPENROUTER_USAGE, LAST_OPENROUTER_FINISH, TOTAL_TOKENS_USED, LAST_OPENROUTER_MODEL
    LAST_OPENROUTER_USAGE = None
    LAST_OPENROUTER_FINISH = None
    LAST_OPENROUTER_MODEL = model or "google/gemini-2.5-flash"
    if not api_key:
        raise RuntimeError("OpenRouter: missing API key. Add it under Settings → AI Models → OpenRouter.")

    model_id = model or "google/gemini-2.5-flash"
    base = (base_url or "https://openrouter.ai/api/v1").rstrip("/")

    user_parts: list[dict] = [{"type": "text", "text": user}]
    for b64, mime in images:
        user_parts.append({
            "type": "image_url",
            "image_url": {"url": f"data:{mime};base64,{b64}"},
        })

    is_reasoning_model = any(
        marker in model_id.lower()
        for marker in ("gemini", "gpt-5", "kimi-k2-thinking", "claude-opus")
    )
    normal_cap, reasoning_cap = token_caps(token_budget, is_reasoning_model)

    body = {
        "model": model_id,
        "temperature": temperature,
        "max_tokens": reasoning_cap if is_reasoning_model else normal_cap,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user_parts},
        ],
    }

    url = f"{base}/chat/completions"
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
            "HTTP-Referer": "https://github.com/mangavox/mangavox",
            "X-Title": "MangaVox",
            "User-Agent": "MangaVox/6.1",
            "Accept": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=240) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="ignore")
        try:
            parsed = json.loads(detail)
            err_msg = parsed.get("error", {}).get("message") or detail
        except Exception:
            err_msg = detail
        
        # Check if the OpenRouter key has insufficient credits or model lacks image input support, and fallback
        msg_lower = err_msg.lower()
        is_credit_or_endpoint_err = (
            "no endpoints found that support image" in msg_lower
            or "credit" in msg_lower
            or "payment" in msg_lower
            or "402" in msg_lower
            or "insufficient" in msg_lower
        )
        if is_credit_or_endpoint_err and model_id != "nvidia/nemotron-nano-12b-v2-vl:free":
            emit_log(f"warn: OpenRouter key lacks credits or model {model_id} is unsupported. Automatically falling back to free vision model: nvidia/nemotron-nano-12b-v2-vl:free")
            return call_openrouter(
                api_key, "nvidia/nemotron-nano-12b-v2-vl:free", base_url, system, user,
                images, temperature, token_budget
            )
            
        raise RuntimeError(f"OpenRouter: {err_msg}")
    except Exception as e:
        # Check for credit or endpoint errors in generic exceptions
        msg_lower = str(e).lower()
        is_credit_or_endpoint_err = (
            "no endpoints found that support image" in msg_lower
            or "credit" in msg_lower
            or "payment" in msg_lower
            or "402" in msg_lower
            or "insufficient" in msg_lower
        )
        if is_credit_or_endpoint_err and model_id != "nvidia/nemotron-nano-12b-v2-vl:free":
            emit_log(f"warn: OpenRouter key lacks credits or model {model_id} is unsupported. Automatically falling back to free vision model: nvidia/nemotron-nano-12b-v2-vl:free")
            return call_openrouter(
                api_key, "nvidia/nemotron-nano-12b-v2-vl:free", base_url, system, user,
                images, temperature, token_budget
            )
            
        raise RuntimeError(f"OpenRouter network error: {e}")

    try:
        data = json.loads(raw)
        choice = data["choices"][0]
        text = choice["message"]["content"]
        LAST_OPENROUTER_FINISH = choice.get("finish_reason")
        usage = data.get("usage") or {}
        LAST_OPENROUTER_USAGE = usage
        if usage:
            prompt = usage.get("prompt_tokens", 0)
            output = usage.get("completion_tokens", 0)
            TOTAL_TOKENS_USED += (prompt + output)
        return text
    except Exception as e:
        raise RuntimeError(f"OpenRouter returned invalid JSON or format: {e} (raw: {raw[:200]})")



def call_g0i(api_key: str, model: str, base_url: str, system: str, user: str,
             images: list[tuple[str, str]], temperature: float,
             token_budget: str | None = None) -> str:
    """Call g0i.ai's OpenAI-compatible /chat/completions endpoint.

    g0i.ai proxies Gemini, Claude (Sonnet/Opus/Haiku) and OpenAI models behind a
    single key + the standard OpenAI chat format. Vision is sent as `image_url`
    parts using data URIs — every supported family understands this shape.

    On success populates `LAST_G0I_USAGE` and `LAST_G0I_FINISH` so the caller
    can report token consumption and detect "ran out of budget on reasoning
    before answering" cases.
    """
    global LAST_G0I_USAGE, LAST_G0I_FINISH
    LAST_G0I_USAGE = None
    LAST_G0I_FINISH = None
    if not api_key:
        raise RuntimeError("g0i: missing API key. Add it under Settings → AI Models → g0i.ai.")

    # Hard belt-and-braces guard against the bundled sample key. Earlier
    # MangaVox releases auto-saved it to config; if that file hasn't been
    # migrated yet, refuse the call locally instead of burning HTTP latency on
    # a response we KNOW is going to be "Trial expired".
    DEAD_SAMPLE_KEY = "sk-43484e549f578382daf4453b70887abff1b98ba0b1e9295d196c4871c217240b"
    if api_key.strip() == DEAD_SAMPLE_KEY:
        raise RuntimeError(
            "g0i.ai: the bundled demo key's trial is dead upstream — no point sending the request. "
            "Open Settings → AI Models → g0i.ai and either paste your own key from "
            "https://g0i.ai/api-keys, or clear the field and switch the Text Studio engine to "
            "Google Gemini (free, no trial) or local Ollama."
        )

    model_id = model or "gemini-2.5-flash"
    base = (base_url or "https://api.g0i.ai/v1").rstrip("/")

    user_parts: list[dict] = [{"type": "text", "text": user}]
    for b64, mime in images:
        user_parts.append({
            "type": "image_url",
            "image_url": {"url": f"data:{mime};base64,{b64}"},
        })

    # Reasoning-style models on g0i (Gemini 2.5/3.x Flash, GPT-5 family) burn
    # hidden chain-of-thought tokens BEFORE the visible answer. With max_tokens
    # too low the model exhausts the budget on reasoning and returns "".
    # Bumping the ceiling so even verbose reasoning still leaves output room.
    #
    # NOTE: I empirically tested `reasoning_effort: minimal` against g0i's
    # proxy and it actually INCREASED reasoning_tokens 5x on Gemini (139 vs 29
    # without). The proxy doesn't forward that hint usefully today, so we just
    # rely on the larger budget instead.
    is_reasoning_model = any(
        marker in model_id.lower()
        for marker in ("gemini", "gpt-5", "kimi-k2-thinking", "claude-opus")
    )
    normal_cap, reasoning_cap = token_caps(token_budget, is_reasoning_model)
    body = {
        "model": model_id,
        "temperature": temperature,
        "max_tokens": reasoning_cap if is_reasoning_model else normal_cap,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user_parts},
        ],
    }

    url = f"{base}/chat/completions"
    # Cloudflare's bot rules in front of api.g0i.ai 403 anything that looks
    # like `Python-urllib/3.x` (error code 1010). Send a real browser UA so the
    # request passes their fingerprint check — same shape as the Electron
    # renderer's fetch that we use in the Settings test-key flow.
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
            "User-Agent": (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/537.36 "
                "(KHTML, like Gecko) MangaVox/6.1 Chrome/124.0.0.0 Safari/537.36"
            ),
            "Accept": "application/json",
            "Accept-Language": "en-US,en;q=0.9",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=240) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="ignore")
        # Cloudflare error 1010 = "browser signature banned" — fires when
        # urllib's default UA leaks through. We already set a real UA above,
        # so if this still hits it means CF tightened the rule. Give the user
        # an actionable hint instead of "bad key".
        is_cf_1010 = (
            "error code: 1010" in detail
            or "Error 1010" in detail
            or ("1010" in detail and "Cloudflare" in detail)
        )
        if is_cf_1010:
            raise RuntimeError(
                "g0i.ai: Cloudflare blocked the request (error 1010 — browser fingerprint). "
                "This usually means the User-Agent header was rejected. Try again in a minute; "
                "if it persists, your IP may be temporarily flagged — switch network/VPN and retry."
            )
        # The gateway often returns 200/400/403 with a JSON `detail` field that
        # tells the user exactly what's wrong (plan-locked, not-a-VLM, bad
        # model name). Surface that text directly — generic "HTTP 403" hides it.
        plan_locked = (
            "require an Opus-tier plan" in detail
            or "Upgrade at https://g0i.ai/pricing" in detail
        )
        not_a_vlm = "not a VLM" in detail or "Vision Language Model" in detail
        unknown_model = "not found. Available models:" in detail
        trial_expired = (
            "free trial has expired" in detail
            or "Buy a plan in Billing" in detail
            or "g0i.ai/billing" in detail
        )
        quota_exceeded = (
            "Daily free-tier limit reached" in detail
            or "Cap resets at 00:00 UTC" in detail
            or "tokens). This cap" in detail
        )
        if trial_expired:
            raise RuntimeError(
                "g0i.ai: the API key's free trial has expired. The shared demo key shipped "
                "with MangaVox is no longer usable — either sign up for your own key at "
                "https://g0i.ai/api-keys (a fresh signup may give you a new trial), or buy "
                "a paid plan at https://g0i.ai/billing. In the meantime, switch the Text "
                "Studio engine to Google Gemini (free) or Ollama (local) in Settings → AI Models."
            )
        if quota_exceeded:
            raise RuntimeError(
                f"g0i.ai daily free-tier cap hit (this happens with the shared demo key). "
                f"Two fixes: (1) get your OWN key at https://g0i.ai/api-keys and paste it in "
                f"Settings → AI Models → g0i.ai — fresh keys get a fresh daily allowance; "
                f"or (2) wait until 00:00 UTC for the cap to reset. Gateway said: {detail[:300]}"
            )
        if plan_locked:
            raise RuntimeError(
                f"g0i.ai: '{model_id}' requires the Opus-tier plan. "
                f"Pick a GPT-5.4 / Kimi model in Settings → AI Models, or upgrade at "
                f"https://g0i.ai/pricing."
            )
        if not_a_vlm:
            raise RuntimeError(
                f"g0i.ai: '{model_id}' is text-only and can't read manga pages. "
                f"Switch to a vision model (GPT-5.4, GPT-5.4-mini, Kimi K2.6) in Settings → AI Models."
            )
        if unknown_model:
            # The gateway's error body already lists the valid models — pass it
            # straight through so the user can copy/paste the right ID.
            raise RuntimeError(f"g0i.ai: {detail.strip()[:600]}")
        if e.code in (401, 403):
            raise RuntimeError(
                f"g0i.ai rejected the API key (HTTP {e.code}). Get a fresh one from "
                f"https://g0i.ai/api-keys and paste it in Settings → AI Models. Detail: {detail[:300]}"
            )
        if e.code == 404:
            raise RuntimeError(
                f"g0i.ai: model '{model_id}' is not available on your plan. "
                f"Pick a different model in Settings → AI Models. Detail: {detail[:300]}"
            )
        if e.code == 429:
            raise RuntimeError(
                f"g0i.ai rate-limited the request (HTTP 429). Slow down or upgrade your plan."
            )
        raise RuntimeError(f"g0i.ai HTTP {e.code} ({model_id}): {detail[:500]}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"g0i.ai network error: {e.reason}")

    data = json.loads(raw)
    # Some upstreams (DeepSeek) return HTTP 200 with an error envelope instead
    # of `choices` — surface that as a real exception, not "no text".
    if isinstance(data, dict) and "choices" not in data:
        msg = (
            (isinstance(data.get("error"), dict) and data["error"].get("message"))
            or data.get("detail")
            or data.get("message")
            or json.dumps(data)[:400]
        )
        msg_s = str(msg)
        if "not a VLM" in msg_s or "Vision Language Model" in msg_s:
            raise RuntimeError(
                f"g0i.ai: '{model_id}' is text-only and can't read manga pages. "
                f"Switch to a vision model (GPT-5.4, GPT-5.4-mini, Kimi K2.6) in Settings → AI Models."
            )
        if (
            "free trial has expired" in msg_s
            or "Buy a plan in Billing" in msg_s
            or "g0i.ai/billing" in msg_s
        ):
            raise RuntimeError(
                "g0i.ai: the API key's free trial has expired. The shared demo key shipped "
                "with MangaVox is no longer usable — sign up for your own at "
                "https://g0i.ai/api-keys, buy a plan at https://g0i.ai/billing, or switch "
                "the Text Studio engine to Google Gemini (free) or Ollama (local) in "
                "Settings → AI Models."
            )
        if (
            "Daily free-tier limit reached" in msg_s
            or "Cap resets at 00:00 UTC" in msg_s
            or "tokens). This cap" in msg_s
        ):
            raise RuntimeError(
                f"g0i.ai daily free-tier cap hit (this happens with the shared demo key). "
                f"Two fixes: (1) get your OWN key at https://g0i.ai/api-keys and paste it in "
                f"Settings → AI Models → g0i.ai — fresh keys get a fresh daily allowance; "
                f"or (2) wait until 00:00 UTC for the cap to reset."
            )
        raise RuntimeError(f"g0i.ai ({model_id}): {msg}")
    try:
        choice_obj = data["choices"][0]
        finish = choice_obj.get("finish_reason") or choice_obj.get("native_finish_reason")
        choice = choice_obj["message"]
        content = choice.get("content")
        # Anthropic-via-OpenAI sometimes returns a list of {type:'text', text:...}.
        if isinstance(content, list):
            content = "".join(p.get("text", "") for p in content if isinstance(p, dict))
        content = content or ""
    except (KeyError, IndexError, TypeError):
        raise RuntimeError(f"g0i.ai returned no text: {json.dumps(data)[:500]}")

    # Capture usage so the batch loop can log it.
    usage = data.get("usage") or {}
    LAST_G0I_USAGE = usage if isinstance(usage, dict) else None
    LAST_G0I_FINISH = finish
    global TOTAL_TOKENS_USED
    if usage:
        prompt = usage.get("prompt_tokens", 0)
        output = usage.get("completion_tokens", 0)
        TOTAL_TOKENS_USED += (prompt + output)

    # Empty-content guard. When reasoning models eat the whole budget on hidden
    # CoT, the gateway returns finish_reason="length" with content="". Bubble a
    # specific error so the user stops bleeding tokens instead of seeing batch
    # after batch silently appending empty strings.
    if not content.strip():
        reasoning_tokens = 0
        try:
            details = usage.get("completion_tokens_details") or {}
            reasoning_tokens = int(details.get("reasoning_tokens") or 0)
        except Exception:
            reasoning_tokens = 0
        completion_tokens = 0
        try:
            completion_tokens = int(usage.get("completion_tokens") or 0)
        except Exception:
            completion_tokens = 0
        hint = ""
        if finish == "length" or reasoning_tokens > completion_tokens * 4:
            hint = (
                " The model burned all its output budget on hidden reasoning. "
                "Pick a non-reasoning model (gpt-5.4-mini, kimi-k2.6) in Settings → AI Models → g0i.ai."
            )
        raise RuntimeError(
            f"g0i.ai ({model_id}): empty response (finish={finish}, "
            f"reasoning_tokens={reasoning_tokens}, output_tokens={completion_tokens}).{hint}"
        )

    return content


def call_ollama(ollama_url: str, model: str, system: str, user: str,
                images: list[tuple[str, str]], temperature: float,
                num_gpu: int | None = None, is_openai: bool = False) -> str:
    # Check if this is an OpenAI compatible endpoint like LM Studio
    is_openai_compat = is_openai or "1234" in ollama_url or "/v1" in ollama_url
    
    if is_openai_compat:
        messages = [{"role": "system", "content": system}]
        content_parts = [{"type": "text", "text": user}]
        for b64, mime in images:
            content_parts.append({
                "type": "image_url",
                "image_url": {
                    "url": f"data:{mime};base64,{b64}"
                }
            })
        messages.append({"role": "user", "content": content_parts})
        
        body = {
            "model": model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": 2048,
            "stream": False
        }
        
        endpoint = ollama_url.rstrip("/")
        if not endpoint.endswith("/v1/chat/completions") and not endpoint.endswith("/chat/completions"):
            if "/v1" in endpoint:
                endpoint = f"{endpoint}/chat/completions"
            else:
                endpoint = f"{endpoint}/v1/chat/completions"
    else:
        user_msg = {"role": "user", "content": user, "images": [b64 for b64, _ in images]}
        options: dict = {"temperature": temperature, "num_predict": 2048, "top_p": 0.9}
        if num_gpu is not None:
            options["num_gpu"] = num_gpu
        body = {
            "model": model,
            "messages": [{"role": "system", "content": system}, user_msg],
            "stream": False,
            "options": options,
        }
        endpoint = f"{ollama_url.rstrip('/')}/api/chat"

    req = urllib.request.Request(
        endpoint,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=600) as resp:  # vision is slow
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="ignore")
        if e.code == 404:
            raise RuntimeError(
                f"Ollama model '{model}' not installed. Run: ollama pull {model}"
            )
        if "runner process has terminated" in detail or "failed to initialize" in detail.lower():
            raise RuntimeError(
                "OLLAMA_RUNTIME_BROKEN: Ollama's model runner crashed (Metal backend "
                "failed to initialise). This is an Ollama/macOS compatibility bug, not a "
                "model or memory problem — it crashes every model. Fix: update Ollama to "
                "the latest version from https://ollama.com/download (your version is too "
                "old for this macOS). Then quit & reopen Ollama and retry. Until then, use Gemini."
            )
        if "out of memory" in detail.lower():
            raise RuntimeError(
                f"'{model}' ran out of memory. Try a smaller model (moondream) or close other apps."
            )
        raise RuntimeError(f"Ollama HTTP {e.code} ({model}): {detail[:400]}")
    except urllib.error.URLError as e:
        raise RuntimeError(
            f"Cannot reach Ollama at {ollama_url}: {e.reason}. Is `ollama serve` running?"
        )
        
    if is_openai_compat:
        content = data.get("choices", [{}])[0].get("message", {}).get("content", "")
    else:
        content = (data.get("message") or {}).get("content") or ""
        
    global TOTAL_TOKENS_USED
    if is_openai_compat:
        usage = data.get("usage") or {}
        prompt = usage.get("prompt_tokens", 0)
        output = usage.get("completion_tokens", 0)
    else:
        prompt = data.get("prompt_eval_count", 0)
        output = data.get("eval_count", 0)
    TOTAL_TOKENS_USED += (prompt + output)

    if not content:
        raise RuntimeError(
            f"Ollama returned no narration ({model}). The selected model may not "
            f"support image input. Try: qwen2.5vl:7b, llava:13b, or minicpm-v."
        )
    return content


def call_ollama_text(ollama_url: str, model: str, system: str, user: str,
                     temperature: float, num_ctx: int = 8192,
                     num_predict: int = 3072, is_openai: bool = False) -> str:
    # Check if this is an OpenAI compatible endpoint like LM Studio
    is_openai_compat = is_openai or "1234" in ollama_url or "/v1" in ollama_url
    
    if is_openai_compat:
        body = {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": temperature,
            "max_tokens": num_predict,
            "stream": False
        }
        endpoint = ollama_url.rstrip("/")
        if not endpoint.endswith("/v1/chat/completions") and not endpoint.endswith("/chat/completions"):
            if "/v1" in endpoint:
                endpoint = f"{endpoint}/chat/completions"
            else:
                endpoint = f"{endpoint}/v1/chat/completions"
    else:
        body = {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": False,
            "options": {"temperature": temperature, "num_predict": num_predict,
                        "top_p": 0.9, "num_ctx": num_ctx},
        }
        endpoint = f"{ollama_url.rstrip('/')}/api/chat"

    req = urllib.request.Request(
        endpoint,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="ignore")
        if "runner process has terminated" in detail or "failed to initialize" in detail.lower():
            raise RuntimeError(
                "OLLAMA_RUNTIME_BROKEN: Ollama's runner crashed (Metal init failed). "
                "Update Ollama from https://ollama.com/download, then restart it."
            )
        raise RuntimeError(f"Ollama text HTTP {e.code} ({model}): {detail[:300]}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Cannot reach Ollama at {ollama_url}: {e.reason}")
        
    global TOTAL_TOKENS_USED
    if is_openai_compat:
        usage = data.get("usage") or {}
        prompt = usage.get("prompt_tokens", 0)
        output = usage.get("completion_tokens", 0)
    else:
        prompt = data.get("prompt_eval_count", 0)
        output = data.get("eval_count", 0)
    TOTAL_TOKENS_USED += (prompt + output)

    if is_openai_compat:
        return data.get("choices", [{}])[0].get("message", {}).get("content", "") or ""
    else:
        return (data.get("message") or {}).get("content") or ""


# ────────────────────────────────────────────────────────────────────────────
# Main loop
# ────────────────────────────────────────────────────────────────────────────

def split_narration_and_delta(raw: str) -> tuple[str, dict]:
    m = MEMORY_RE.search(raw) or MEMORY_RE_LOOSE.search(raw)
    if not m:
        return clean_narration(raw), {}
    narration = raw[:m.start()]
    delta_str = _trim_to_balanced_json(m.group(1).strip())
    try:
        delta = json.loads(delta_str)
    except json.JSONDecodeError:
        delta = {}
    return clean_narration(narration), delta

def _trim_to_balanced_json(s: str) -> str:
    depth = 0
    end = -1
    in_str = False
    esc = False
    for i, ch in enumerate(s):
        if in_str:
            if esc: esc = False
            elif ch == "\\": esc = True
            elif ch == '"': in_str = False
            continue
        if ch == '"': in_str = True
        elif ch == "{": depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    return s[:end] if end > 0 else s

def _est_tokens(text: str) -> int:
    # Cheap heuristic — good enough to decide chunking. ~4 chars/token.
    return len(text) // 4

def run_ollama_two_phase(job: dict, pages: list[str], memory: dict,
                         final_path: str, md_path: str, partial_path: str,
                         chapter_number, temperature: float, resume: bool, use_raw_pages: bool = True) -> None:
    """Ollama narration in two phases for whole-chapter depth:
      Phase 1 — vision model DESCRIBES every page (1 image/call).
      Phase 2 — text model NARRATES large chunks of descriptions at once, so
                the writing has full-chapter context instead of per-page myopia.
    Both phases are resumable via the partial file."""
    ourl = job.get("ollama_url") or "http://127.0.0.1:11434"
    vision_model = job["ollama_model"]
    text_model = job.get("ollama_text_model") or vision_model
    memory_path = job["memory_path"]
    provider = "ollama"
    is_openai = bool(job.get("is_openai", False))
    output_dir = job["output_dir"]
    total = len(pages)
    page_offset = int(job.get("page_offset") or 0)
    total_pages_all = int(job.get("total_pages_all") or total)
    language = (job.get("language") or "english").lower()
    prefix_batches = list(job.get("prefix_batches") or [])
    narrate_system = prompt_for_language(NARRATE_SYSTEM, language)

    # Resumable state across both phases.
    phase = "describe"
    descriptions: list[dict] = []     # [{page, text, focus_points}]
    next_page = 0
    narration_chunks: list[dict] = [] # [{from,to,text}]
    next_chunk = 0
    cpu_mode = False

    if resume and os.path.exists(partial_path):
        try:
            with open(partial_path, "r", encoding="utf-8") as fh:
                prev = json.load(fh)
            phase = prev.get("phase") or "describe"
            descriptions = list(prev.get("descriptions") or [])
            next_page = int(prev.get("next_page") or len(descriptions))
            narration_chunks = list(prev.get("local_batches") or prev.get("batches") or [])
            next_chunk = int(prev.get("next_chunk") or len(narration_chunks))
            emit_log(f"narrator(ollama): resuming phase={phase} page={next_page}/{total} chunk={next_chunk}")
        except Exception as e:
            emit_log(f"warn: failed to load partial, starting fresh: {e}")

    model_label = f"{vision_model}+{text_model}" if text_model != vision_model else vision_model

    def save_partial():
        try:
            with open(partial_path, "w", encoding="utf-8") as fh:
                json.dump({
                    "chapter": chapter_number, "provider": provider,
                    "phase": phase, "descriptions": descriptions, "next_page": next_page,
                    "batches": prefix_batches + narration_chunks,
                    "local_batches": narration_chunks,
                    "next_chunk": next_chunk,
                    "total_batches": max(1, len(narration_chunks)),
                    "next_batch_index": next_chunk,
                    "model": model_label, "pages": total_pages_all,
                    "language": language,
                    "page_from": page_offset + 1,
                    "page_to": page_offset + total,
                    "tokens_used": TOTAL_TOKENS_USED,
                }, fh, ensure_ascii=False, indent=2)
        except Exception as e:
            emit_log(f"warn: partial save failed: {e}")

    def parse_combined_descriptions(combined_text: str, page_numbers: list[int]) -> dict[int, str]:
        import re
        result = {}
        # Match pattern for page titles like: [Page 5], Page 5:, ### Page 5, etc.
        pattern = re.compile(r'(?:\[?\b[pP]age\s+(\d+)\b\]?:?|###\s*[pP]age\s+(\d+))')
        matches = list(pattern.finditer(combined_text))
        
        if not matches:
            if len(page_numbers) == 1:
                result[page_numbers[0]] = combined_text
            else:
                paragraphs = [p.strip() for p in combined_text.split("\n\n") if p.strip()]
                if len(paragraphs) >= len(page_numbers):
                    chunk_sz = len(paragraphs) // len(page_numbers)
                    for idx, p_num in enumerate(page_numbers):
                        start_p = idx * chunk_sz
                        end_p = start_p + chunk_sz if idx < len(page_numbers) - 1 else len(paragraphs)
                        result[p_num] = "\n\n".join(paragraphs[start_p:end_p])
                else:
                    text_len = len(combined_text)
                    chunk_len = text_len // len(page_numbers)
                    for idx, p_num in enumerate(page_numbers):
                        result[p_num] = combined_text[idx*chunk_len:(idx+1)*chunk_len]
            return result

        for idx, match in enumerate(matches):
            p_num_str = match.group(1) or match.group(2)
            if not p_num_str:
                continue
            p_num = int(p_num_str)
            start_idx = match.end()
            end_idx = matches[idx+1].start() if idx + 1 < len(matches) else len(combined_text)
            result[p_num] = combined_text[start_idx:end_idx].strip()
            
        for p_num in page_numbers:
            if p_num not in result:
                result[p_num] = ""
        return result

    def ollama_describe(images: list[tuple[str, str]], user_prompt: str) -> str:
        nonlocal cpu_mode
        for attempt in range(2):
            try:
                return call_ollama(ourl, vision_model, DESCRIBE_SYSTEM, user_prompt,
                                   images, 0.3, num_gpu=0 if cpu_mode else None, is_openai=is_openai)
            except Exception as e:
                msg = str(e)
                if "OLLAMA_RUNTIME_BROKEN" in msg:
                    raise
                if not cpu_mode and "out of memory" in msg.lower():
                    cpu_mode = True
                    emit_log("vision OOM — switching to CPU for describe phase")
                    continue
                if attempt == 0:
                    time.sleep(1); continue
                raise
        return ""

    # ── PHASE 1: describe pages in batches ─────────────────────────────────
    if phase == "describe":
        init_progress_timer(next_page)
        batch_size_val = int(job.get("batch_size") or 4)
        
        i = next_page
        while i < total:
            limit = min(i + batch_size_val, total)
            emit_progress(i + 1, total, f"Reading pages {i + 1}-{limit}/{total} (describing)")
            
            batch_imgs = []
            valid_indices = []
            for j in range(i, limit):
                p_idx = page_offset + j + 1
                try:
                    img = encode_page(pages[j], MAX_DIM_OLLAMA)
                    batch_imgs.append(img)
                    valid_indices.append(j)
                except Exception as e:
                    emit_log(f"warn: encode failed page {j+1}: {e}")
                    descriptions.append({"page": p_idx, "text": "", "focus_points": []})
            
            if not batch_imgs:
                i = limit
                next_page = i
                save_partial()
                continue
            
            pages_list_str = ", ".join(str(page_offset + idx + 1) for idx in valid_indices)
            custom_user = (
                f"You are describing the following manga pages: {pages_list_str}.\n"
                f"Describe each page separately and in order. You MUST start the description of each page "
                f"with a clear marker on its own line: '[Page X]' (where X is the page number, e.g. [Page {page_offset + valid_indices[0] + 1}]).\n"
                f"Be precise and extremely brief in your descriptions.\n\n"
                f"{DESCRIBE_USER}"
            )
            
            ocr_refs = []
            for idx in valid_indices:
                p_num = page_offset + idx + 1
                ocr_txt = load_ocr_for_page(output_dir, p_num)
                if ocr_txt:
                    ocr_refs.append(f"--- PAGE {p_num} DIALOGUE/TEXT ---\n{ocr_txt}")
            if ocr_refs:
                custom_user += "\n\nPRE-EXTRACTED DIALOGUE/TEXT (CRITICAL REFERENCE — USE THESE EXACT TEXTS AND SPEAKERS):\n" + "\n\n".join(ocr_refs)
            
            try:
                combined_desc = ollama_describe(batch_imgs, custom_user)
            except Exception as e:
                if "OLLAMA_RUNTIME_BROKEN" in str(e):
                    emit_error(str(e).replace("OLLAMA_RUNTIME_BROKEN: ", ""))
                    return
                emit_error(f"Vision describe failed on pages {pages_list_str}: {e}")
                return
            
            parsed_descs = parse_combined_descriptions(combined_desc, [page_offset + idx + 1 for idx in valid_indices])
            
            for idx in valid_indices:
                p_num = page_offset + idx + 1
                desc_text = parsed_descs.get(p_num, "").strip()
                fps = parse_focus_points(desc_text)
                descriptions.append({"page": p_num, "text": desc_text, "focus_points": fps})
            
            i = limit
            next_page = i
            save_partial()
            
        phase = "narrate"
        next_chunk = 0
        save_partial()

    # ── PHASE 2: narrate large chunks (whole-chapter context) ──────────────
    # Pack as many page descriptions as fit a generous token budget into each
    # chunk so the writer sees big spans at once. qwen-class models handle 32k
    # context; we target ~12k tokens of descriptions per chunk and size num_ctx
    # to fit prompt+output.
    DESC_TOKEN_BUDGET = 12000
    chunks: list[tuple[int, int]] = []
    cur_start = 0
    cur_tokens = 0
    for idx, d in enumerate(descriptions):
        t = _est_tokens(d.get("text", "")) + 20
        if cur_tokens + t > DESC_TOKEN_BUDGET and idx > cur_start:
            chunks.append((cur_start, idx))
            cur_start = idx
            cur_tokens = 0
        cur_tokens += t
    if cur_start < len(descriptions):
        chunks.append((cur_start, len(descriptions)))
    if not chunks:
        chunks = [(0, len(descriptions))]

    emit_log(f"narrator(ollama): {total} pages described → {len(chunks)} narration chunk(s) of ~{DESC_TOKEN_BUDGET} tokens")

    # Re-emit any already-written chunks so the UI repopulates on resume.
    for idx, b in enumerate(narration_chunks):
        emit_batch(idx, b.get("from", 0), b.get("to", 0), b.get("text", ""), provider)

    init_progress_timer(next_chunk)
    for ci in range(next_chunk, len(chunks)):
        c_start, c_end = chunks[ci]
        page_from = descriptions[c_start]["page"]
        page_to = descriptions[c_end - 1]["page"]
        emit_progress(ci + 1, len(chunks), f"Narrating pages {page_from}-{page_to} ({ci+1}/{len(chunks)})")

        # Known characters + story so far from memory.
        char_lines = []
        for ch in memory.get("characters") or []:
            al = ch.get("aliases") or []
            char_lines.append(f"- {ch.get('name','')}{' (also: '+', '.join(al)+')' if al else ''} — {ch.get('description','')}")
        known = "\n".join(char_lines) if char_lines else "(none yet)"
        story_so_far = (memory.get("storySoFar") or "").strip() or "(start of what we've narrated)"

        desc_block = "\n\n".join(
            f"[Page {descriptions[j]['page']}]\n{descriptions[j]['text']}"
            for j in range(c_start, c_end) if descriptions[j].get("text")
        )
        position = "the OPENING of" if ci == 0 else ("the FINAL part of" if ci == len(chunks) - 1 else "a middle part of")
        chap_lbl = f"Chapter {chapter_number}" if chapter_number is not None else "this chapter"
        user = (
            f"STORY SO FAR\n{story_so_far}\n\n"
            f"KNOWN CHARACTERS\n{known}\n\n"
            f"This is {position} {chap_lbl}, covering pages {page_from}-{page_to}.\n"
            f"{'Open with a hook line that frames the chapter.' if ci == 0 else ''}\n"
            f"{'End with a cliffhanger/lead-out line.' if ci == len(chunks)-1 else ''}\n\n"
            f"PAGE DESCRIPTIONS (narrate these into one flowing, in-depth passage):\n{desc_block}\n\n"
            f"Write the narration, then the ---MEMORY--- delta."
        )
        # Size context to hold the whole prompt + output comfortably.
        need = _est_tokens(narrate_system) + _est_tokens(user) + 3072 + 1024
        num_ctx = 8192
        for cap in (8192, 16384, 32768):
            if need <= cap:
                num_ctx = cap; break
        else:
            num_ctx = 32768

        try:
            raw = call_ollama_text(ourl, text_model, narrate_system, user,
                                   min(temperature, 0.4), num_ctx=num_ctx, num_predict=3072, is_openai=is_openai)
            preview, _ = split_narration_and_delta(raw)
            issues = validate_narration(preview, language)
            if issues:
                emit_log(f"narrator(ollama): retrying rejected chunk {ci+1}: {', '.join(issues)}")
                raw = call_ollama_text(
                    ourl, text_model, narrate_system,
                    user + correction_prompt(language, issues),
                    0.2, num_ctx=num_ctx, num_predict=3072, is_openai=is_openai
                )
        except Exception as e:
            if "OLLAMA_RUNTIME_BROKEN" in str(e):
                emit_error(str(e).replace("OLLAMA_RUNTIME_BROKEN: ", ""))
                return
            emit_error(f"Narration chunk {ci+1} failed: {e}")
            return

        narration, delta = split_narration_and_delta(raw)
        issues = validate_narration(narration, language)
        if issues:
            emit_error(
                f"Narration chunk {ci+1} was rejected after retry: {', '.join(issues)}. "
                "Your previous valid progress was preserved."
            )
            return
        if delta:
            memory = merge_delta(memory, delta, chapter_number)
            save_memory(memory_path, memory)
        narration_chunks.append({"from": page_from, "to": page_to, "text": narration})
        next_chunk = ci + 1
        emit_batch(ci, page_from, page_to, narration, provider)
        save_partial()

    final_batches = prefix_batches + narration_chunks
    final = "\n\n".join(b["text"] for b in final_batches).strip()
    
    # Collect focus points from Phase 1 descriptions
    fps_map = {}
    for d in descriptions:
        pg = str(d["page"])
        if d.get("focus_points"):
            fps_map[pg] = d["focus_points"]
            
    payload = {
        "chapter": chapter_number, "narration": final, "batches": final_batches,
        "model": model_label, "pages": total_pages_all, "provider": provider,
        "language": language, "page_from": page_offset + 1,
        "page_to": page_offset + total,
        "focus_points": fps_map,
        "tokens_used": TOTAL_TOKENS_USED,
    }
    attach_beats(payload, job["pages_dir"], final_batches)
    with open(final_path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, ensure_ascii=False, indent=2)
    with open(md_path, "w", encoding="utf-8") as fh:
        fh.write(final + "\n")
    try:
        if os.path.exists(partial_path):
            os.remove(partial_path)
    except Exception:
        pass
    emit_done({"output_path": final_path, "pages": total_pages_all, "batches": len(narration_chunks),
               "model": model_label, "provider": provider})


def run_job(job: dict) -> None:
    pages_dir = job["pages_dir"]
    output_dir = job["output_dir"]
    memory_path = job["memory_path"]
    manga_id = job["manga_id"]
    chapter_number = job.get("chapter_number")
    provider = (job.get("provider") or "gemini").lower()
    use_raw_pages = bool(job.get("use_raw_pages", True))
    
    # Most Ollama vision models accept multiple images if prompted properly.
    # Allow grouping by batch_size for Ollama (default 4).
    batch_size = int(job.get("batch_size") or (4 if provider == "ollama" else 12))
    temperature = float(job.get("temperature") or 0.55)
    resume = bool(job.get("resume") or False)

    os.makedirs(output_dir, exist_ok=True)
    pages = list_pages(pages_dir, use_raw_pages=use_raw_pages)
    if not pages:
        emit_error(f"No image pages found in {pages_dir}")
        return
    total_pages_all = len(pages)
    page_offset = 0
    if job.get("page_from") or job.get("page_to"):
        start_page = min(total_pages_all, max(1, int(job.get("page_from") or 1)))
        end_page = min(total_pages_all, int(job.get("page_to") or total_pages_all))
        if start_page > end_page:
            start_page, end_page = end_page, start_page
        pages = pages[start_page - 1:end_page]
        page_offset = start_page - 1
        job["page_offset"] = page_offset
        job["total_pages_all"] = total_pages_all
        emit_log(f"narrator: page range {start_page}-{end_page} of {total_pages_all}")
    language = (job.get("language") or "english").lower()
    system_prompt = prompt_for_language(SYSTEM_PROMPT, language)

    memory = load_memory(memory_path, manga_id)
    total_pages = len(pages)
    batches: list[tuple[int, int]] = []
    for i in range(0, total_pages, batch_size):
        batches.append((i, min(i + batch_size, total_pages)))
    batch_count = len(batches)

    # ── Per-provider output paths ─────────────────────────────────────────
    # Each provider gets its own output file so running Ollama after Gemini
    # (or vice versa) does NOT overwrite the existing narration. The
    # renderer reads both and shows whichever the user selects.
    final_path  = os.path.join(output_dir, f"narration.{provider}.json")
    md_path     = os.path.join(output_dir, f"narration.{provider}.md")
    partial_path = os.path.join(output_dir, f"narration.{provider}.partial.json")

    # Ollama uses the two-phase (describe-all → narrate-in-large-chunks) flow
    # so the writing has whole-chapter context. Gemini keeps its multimodal
    # batch loop below (it reads images + writes in one good pass).
    if provider == "ollama":
        run_ollama_two_phase(job, pages, memory, final_path, md_path, partial_path,
                             chapter_number, temperature, resume, use_raw_pages)
        return

    prefix_batches = list(job.get("prefix_batches") or [])
    all_narration: list[str] = [str(b.get("text") or "") for b in prefix_batches]
    saved_batches: list[dict] = []
    model_label = ""
    start_batch_index = 0
    all_focus_points: dict = {}
    if resume and os.path.exists(partial_path):
        try:
            with open(partial_path, "r", encoding="utf-8") as fh:
                prev = json.load(fh)
            saved_batches = list(prev.get("local_batches") or prev.get("batches") or [])
            all_narration = [b.get("text", "") for b in prefix_batches + saved_batches]
            start_batch_index = int(prev.get("next_batch_index") or len(saved_batches))
            model_label = prev.get("model") or ""
            all_focus_points = prev.get("focus_points") or {}
            emit_log(f"narrator: resuming at batch {start_batch_index + 1}/{batch_count} ({len(saved_batches)} batches recovered)")
        except Exception as e:
            emit_log(f"warn: failed to load partial state, starting fresh: {e}")
            start_batch_index = 0
            saved_batches = []
            all_narration = [str(b.get("text") or "") for b in prefix_batches]
            all_focus_points = {}

    emit_log(f"narrator: {len(pages)} pages, provider={provider}, batch_size={batch_size}, start_batch={start_batch_index}, language={language}")

    def write_partial(next_index: int) -> None:
        try:
            with open(partial_path, "w", encoding="utf-8") as fh:
                json.dump({
                    "chapter": chapter_number,
                    "batches": prefix_batches + saved_batches,
                    "local_batches": saved_batches,
                    "next_batch_index": next_index,
                    "total_batches": batch_count,
                    "model": model_label,
                    "provider": provider,
                    "pages": total_pages_all,
                    "language": language,
                    "page_from": page_offset + 1,
                    "page_to": page_offset + total_pages,
                    "focus_points": all_focus_points,
                    "tokens_used": TOTAL_TOKENS_USED,
                }, fh, ensure_ascii=False, indent=2)
        except Exception as e:
            emit_log(f"warn: failed to write partial state: {e}")

    # Once a GPU OOM is seen we flip this and stay on CPU for the whole run.
    ollama_cpu_mode = False

    # Surface already-completed batches to the renderer up front so the UI
    # repopulates immediately on resume (instead of flashing empty).
    for idx, b in enumerate(saved_batches):
        emit_batch(idx, b.get("from", 0), b.get("to", 0), b.get("text", ""), provider)

    init_progress_timer(start_batch_index)
    for bi, (start, end) in enumerate(batches):
        if bi < start_batch_index:
            continue
        page_from = page_offset + start + 1
        page_to = page_offset + end
        emit_progress(bi + 1, batch_count, f"batch {bi + 1}/{batch_count} (pages {page_from}-{page_to})")

        # Encode images for this batch (smaller for Ollama to avoid OOM crashes)
        if provider == "ollama":
            max_dim = MAX_DIM_OLLAMA
        elif provider == "g0i":
            max_dim = MAX_DIM_G0I
        else:
            max_dim = MAX_DIM_GEMINI
        encoded: list[tuple[str, str]] = []
        for p in pages[start:end]:
            try:
                encoded.append(encode_page(p, max_dim))
            except Exception as e:
                emit_log(f"warn: failed to encode {p}: {e}")

        if not encoded:
            emit_log(f"warn: batch {bi + 1} produced no encodable images, skipping")
            continue

        # Collect OCR text for the pages in this batch
        ocr_lines = []
        for j, p_path in enumerate(pages[start:end]):
            actual_page_num = page_from + j
            ocr_txt = load_ocr_for_page(output_dir, actual_page_num)
            if ocr_txt:
                ocr_lines.append(f"[Page {actual_page_num}]\nPre-extracted Dialogue/Text:\n{ocr_txt}")
        
        ocr_reference = ""
        if ocr_lines:
            ocr_reference = "\n\nPRE-EXTRACTED DIALOGUE/TEXT FOR THE PAGES IN THIS BATCH (CRITICAL REFERENCE — USE THESE EXACT DIALOGUE LINES/BUBBLES FOR ACCURACY, DO NOT FABRICATE DIALOGUE):\n" + "\n\n".join(ocr_lines)

        user_prompt = build_user_prompt(
            batch_index=bi, batch_count=batch_count,
            page_from=page_from, page_to=page_to, total_pages=total_pages_all,
            chapter_number=chapter_number, memory=memory,
        ) + ocr_reference

        attempts = 0
        last_err: Exception | None = None
        raw_response = ""
        # Cloud providers (Gemini / g0i) hit transient 503/429 under load —
        # bump the cap so a single capacity spike doesn't kill a 9-batch run
        # the user already invested tokens in. Ollama keeps its smaller cap
        # since failures there are usually configuration-level, not load.
        max_attempts = 3 if provider == "ollama" else 6
        # Track whether the previous failure was a transient overload — drives
        # the exponential backoff schedule below.
        last_was_transient = False
        while attempts < max_attempts:
            attempts += 1
            try:
                if provider == "gemini":
                    model_label = job.get("gemini_model") or "gemini-2.0-flash"
                    raw_response = call_gemini(
                        job["gemini_api_key"], model_label,
                        system_prompt, user_prompt, encoded, temperature,
                        job.get("token_budget"),
                    )
                elif provider == "g0i":
                    # g0i.ai — OpenAI-compatible proxy. The configured model can be
                    # a Gemini, Claude or GPT id; we just forward whatever was saved.
                    model_label = job.get("g0i_model") or "gemini-2.5-flash-new"
                    raw_response = call_g0i(
                        job.get("g0i_api_key") or "",
                        model_label,
                        job.get("g0i_base_url") or "https://api.g0i.ai/v1",
                        system_prompt, user_prompt, encoded, temperature,
                        job.get("token_budget"),
                    )
                    # Surface token usage so the UI can show what's being spent
                    # per batch — the "wasting tokens with no output" symptom
                    # was invisible before because we only logged at done-time.
                    if LAST_G0I_USAGE:
                        u = LAST_G0I_USAGE
                        details = u.get("completion_tokens_details") or {}
                        emit_log(
                            f"g0i batch {bi + 1}/{batch_count} usage — "
                            f"prompt={u.get('prompt_tokens')} "
                            f"output={u.get('completion_tokens')} "
                            f"reasoning={details.get('reasoning_tokens', 0)} "
                            f"total={u.get('total_tokens')} "
                            f"finish={LAST_G0I_FINISH}"
                        )
                elif provider == "openrouter":
                    model_label = job.get("openrouter_model") or "google/gemini-2.5-flash"
                    raw_response = call_openrouter(
                        job.get("openrouter_api_key") or "",
                        model_label,
                        job.get("openrouter_base_url") or "https://openrouter.ai/api/v1",
                        system_prompt, user_prompt, encoded, temperature,
                        job.get("token_budget"),
                    )
                    # Update model_label in case a fallback occurred
                    if LAST_OPENROUTER_MODEL:
                        model_label = LAST_OPENROUTER_MODEL

                    if LAST_OPENROUTER_USAGE:
                        u = LAST_OPENROUTER_USAGE
                        details = u.get("completion_tokens_details") or {}
                        emit_log(
                            f"openrouter batch {bi + 1}/{batch_count} usage — "
                            f"prompt={u.get('prompt_tokens')} "
                            f"output={u.get('completion_tokens')} "
                            f"reasoning={details.get('reasoning_tokens', 0)} "
                            f"total={u.get('total_tokens')} "
                            f"finish={LAST_OPENROUTER_FINISH}"
                        )
                        
                    # Rate limit safety sleep for free tier / free models
                    if ":free" in model_label.lower():
                        emit_log(f"narrator(openrouter): rate limit cooldown sleep (5s) for free model {model_label}...")
                        time.sleep(5)
                elif provider == "ollama":
                    ourl = job.get("ollama_url") or "http://127.0.0.1:11434"
                    vision_model = job["ollama_model"]
                    text_model = job.get("ollama_text_model") or vision_model
                    ngpu = 0 if ollama_cpu_mode else None
                    # ── Stage 1: vision model DESCRIBES each page plainly ──
                    descriptions = []
                    for (b64, mime) in encoded:
                        desc = call_ollama(
                            ourl, vision_model,
                            DESCRIBE_SYSTEM, DESCRIBE_USER, [(b64, mime)], 0.3,
                            num_gpu=ngpu,
                        )
                        descriptions.append(desc.strip())
                    combined_desc = "\n\n".join(d for d in descriptions if d)
                    # ── Stage 2: text model WRITES the narration from the
                    # description + rolling memory. A real text model writes far
                    # better prose than a small vision model. If no separate text
                    # model is available, fall back to the vision model's text.
                    text_user = (
                        user_prompt
                        + "\n\nRAW PAGE DESCRIPTION (rewrite this into the narration; "
                        + "do not add events that aren't described):\n"
                        + combined_desc
                    )
                    if text_model and text_model != vision_model:
                        raw_response = call_ollama_text(
                            ourl, text_model, system_prompt, text_user, temperature,
                        )
                        model_label = f"{vision_model}+{text_model}"
                    else:
                        raw_response = call_ollama_text(
                            ourl, vision_model, system_prompt, text_user, temperature,
                        )
                        model_label = vision_model
                else:
                    raise RuntimeError(f"Unknown provider: {provider}")
                preview, _ = split_narration_and_delta(raw_response)
                issues = validate_narration(preview, language)
                if issues:
                    if attempts < max_attempts:
                        emit_log(f"batch {bi + 1}: rejected output, retrying: {', '.join(issues)}")
                        user_prompt += correction_prompt(language, issues)
                        raw_response = ""
                        continue
                    raise RuntimeError(f"invalid narration output: {', '.join(issues)}")
                break
            except Exception as e:
                last_err = e
                msg = str(e)
                # A broken Ollama runtime (Metal init failure) can't be fixed by
                # retrying or switching to CPU — fail immediately with the clear
                # "update Ollama" guidance so the user isn't left waiting.
                if "OLLAMA_RUNTIME_BROKEN" in msg:
                    emit_error(msg.replace("OLLAMA_RUNTIME_BROKEN: ", ""))
                    return
                # Genuine GPU OOM on other machines → try CPU once.
                if provider == "ollama" and not ollama_cpu_mode and "out of memory" in msg.lower():
                    ollama_cpu_mode = True
                    emit_log(f"batch {bi + 1}: GPU OOM — retrying on CPU (num_gpu=0). Slower but reliable.")
                    emit_progress(bi + 1, batch_count, f"batch {bi + 1}/{batch_count} — switched to CPU mode (slower)")
                    continue
                # Transient overload from cloud providers — Gemini returns 503
                # "high demand" / "UNAVAILABLE" or 429 when too many people are
                # hitting Flash. The fix is just to wait longer, not give up at
                # 2 attempts. Exponential backoff so a brief spike doesn't kill
                # a long narration the user has already paid tokens for.
                is_transient = (
                    "HTTP 503" in msg
                    or "HTTP 429" in msg
                    or "UNAVAILABLE" in msg
                    or "high demand" in msg.lower()
                    or "overloaded" in msg.lower()
                    or "rate" in msg.lower() and "limit" in msg.lower()
                    or "no endpoints" in msg.lower()
                    or "no active endpoints" in msg.lower()
                )
                last_was_transient = is_transient
                if is_transient and attempts < max_attempts:
                    # 5s, 15s, 30s, 60s, 120s — capacity usually recovers
                    # within the first minute, this gives us 4+ minutes total.
                    backoff = [5, 15, 30, 60, 120][min(attempts - 1, 4)]
                    emit_log(
                        f"batch {bi + 1}: transient {provider} overload "
                        f"(attempt {attempts}/{max_attempts}) — waiting {backoff}s before retry"
                    )
                    emit_progress(
                        bi + 1, batch_count,
                        f"batch {bi + 1}/{batch_count} — provider overloaded, retrying in {backoff}s "
                        f"(attempt {attempts + 1}/{max_attempts})"
                    )
                    time.sleep(backoff)
                    continue
                emit_log(f"warn: batch {bi + 1} attempt {attempts} failed: {e}")
                time.sleep(2 if attempts == 1 else 0)
        # End-of-loop: if all attempts failed, surface the error
        if not raw_response:
            if last_was_transient:
                emit_error(
                    f"Batch {bi + 1} could not complete after {attempts} retries — "
                    f"{provider} is under heavy load right now. Your progress is saved "
                    f"(batches 1-{bi} complete). Wait a couple of minutes, then click "
                    f"Resume to pick up at batch {bi + 1}."
                )
            else:
                emit_error(
                    f"Batch {bi + 1} failed: {last_err or 'unknown error'}. "
                    "Ensure your key has sufficient credits and the selected model is supported."
                )
            return
        narration, delta = split_narration_and_delta(raw_response)
        
        # Parse focus points for this batch
        batch_fps = parse_multi_page_focus_points(raw_response)
        if not batch_fps:
            fps_list = parse_focus_points(raw_response)
            if fps_list:
                batch_fps = {str(page_from): fps_list}
        all_focus_points.update(batch_fps)

        issues = validate_narration(narration, language)
        if issues:
            emit_error(
                f"Batch {bi + 1} was rejected: {', '.join(issues)}. "
                "Earlier valid batches remain saved; use Resume to try again."
            )
            return
        all_narration.append(narration)
        saved_batches.append({"from": page_from, "to": page_to, "text": narration})

        if delta:
            memory = merge_delta(memory, delta, chapter_number)
            save_memory(memory_path, memory)

        emit_batch(bi, page_from, page_to, narration, provider)
        # Persist progress so an error on the next batch doesn't cost us this one.
        write_partial(bi + 1)

    final = "\n\n".join(all_narration).strip()
    final_batches = prefix_batches + saved_batches
    payload = {
        "chapter": chapter_number,
        "narration": final,
        "batches": final_batches,
        "model": model_label,
        "pages": total_pages_all,
        "provider": provider,
        "language": language,
        "page_from": page_offset + 1,
        "page_to": page_offset + total_pages,
        "focus_points": all_focus_points,
        "tokens_used": TOTAL_TOKENS_USED,
    }
    attach_beats(payload, pages_dir, final_batches)
    with open(final_path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, ensure_ascii=False, indent=2)
    with open(md_path, "w", encoding="utf-8") as fh:
        fh.write(final + "\n")

    # Successful completion — drop the partial state file.
    try:
        if os.path.exists(partial_path):
            os.remove(partial_path)
    except Exception:
        pass

    emit_done({"output_path": final_path, "pages": total_pages_all, "batches": len(saved_batches), "model": model_label, "provider": provider})


def main() -> None:
    if len(sys.argv) < 2:
        emit_error("Usage: scene_narrator.py <job.json>")
        sys.exit(1)
    job_path = sys.argv[1]
    try:
        with open(job_path, "r", encoding="utf-8") as fh:
            job = json.load(fh)
        run_job(job)
    except Exception as e:
        emit_error(f"{e}\n{traceback.format_exc()}")
        sys.exit(1)


if __name__ == "__main__":
    main()
