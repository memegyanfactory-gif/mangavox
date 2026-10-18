#!/usr/bin/env python3
"""
MangaVox Text Enhancement

Two flavors:
  • creative / exaggerated  — stateless single-page rewrite (existing behavior)
  • detailed                — chapter-aware "manga recap" narration with
                              rolling memory across pages

The detailed mode emulates the cadence of YouTube manga recap channels: past
tense third-person prose that knows what already happened, names characters
consistently, and keeps the chapter feeling like one continuous story rather
than N disconnected paragraphs.

Communicates via JSON lines on stdout.
Usage: python3 text_enhance.py <job.json>
"""
import sys
import json
import os
import re
import urllib.request
import urllib.error


def emit(obj):
    print(json.dumps(obj), flush=True)


def emit_progress(current, total):
    emit({"type": "progress", "current": current, "total": total,
          "percent": round(current / max(total, 1) * 100, 1)})


def emit_error(message):
    emit({"type": "error", "message": message})


# =============================================================================
# Stateless prompts (creative / exaggerated)
# =============================================================================

_ENGLISH_RULE = (
    "Output STRICTLY in English. Translate any Japanese, Chinese, Korean, or other "
    "non-English source text into natural English before quoting. "
)

STATELESS_PROMPTS = {
    "creative": (
        "You are a skilled creative writer adapting manga text for audiobook narration. "
        + _ENGLISH_RULE +
        "The input is structured panel-by-panel text from a manga page with [DIALOGUE], [NARRATION], "
        "[ON-SCREEN], and [SFX] tags. Convert this into flowing, vivid prose narration. "
        "Keep all dialogue in quotation marks with speaker attribution. "
        "Weave narration boxes and action naturally between dialogue. "
        "Add emotional depth and vivid language while preserving meaning. "
        "Maintain distinct character voices. Output ONLY the rewritten narration, nothing else."
    ),
    "exaggerated": (
        "You are a dramatic anime narrator. "
        + _ENGLISH_RULE +
        "The input is structured panel-by-panel text from a manga page. "
        "Convert it into an exaggerated, over-the-top dramatic narration. "
        "Amplify every emotion to its extreme. Add dramatic pauses (using ellipses and dashes), "
        "intense descriptions, and theatrical flair. Make it feel like the most epic anime moment ever. "
        "Keep dialogue in quotes with speaker names. Output ONLY the rewritten text, nothing else."
    ),
}


# =============================================================================
# Recap-creator system prompt (detailed mode)
# =============================================================================

RECAP_SYSTEM_PROMPT = """You are a manga recap narrator — the kind that voices popular YouTube manga recap videos. Your job is to retell the story so a listener who can't see the panels feels every beat.

LANGUAGE
- Output STRICTLY in English. If the source dialogue or text is in Japanese, Chinese, Korean, or any other language, translate it into natural English before quoting. Never emit non-English script.

VOICE
- Past tense, third-person omniscient.
- Confident, cinematic, but never cheesy. No "Behold!" or "Lo and behold."
- Lean into specifics: the character's expression, the weight of a silence, the *thing* that made the moment land.
- Smooth transitions between panels — "Meanwhile,", "Just then,", "Without warning,", "Across the room,". Vary them.

DIALOGUE
- Keep every line of original dialogue intact, in quotes, with the speaker named (e.g. Naruto smirked. "Watch this."). If the speaker is unknown, write a voice from the shadows said "...".
- Don't paraphrase dialogue. The lines are sacred.

CONTINUITY
- You are mid-chapter. The reader already knows the characters. Don't re-introduce them.
- Keep names consistent with the STORY SO FAR section.
- If a new character appears, name them once and move on.

LENGTH
- 2 to 4 paragraphs per page. No headers. No bullet points. Just prose.
- If a page has no story text (transition / pure art), write ONE short sentence describing the beat (e.g. "A long silence stretched across the rooftop.") — don't pad.

TONE GUARDRAILS
- Match the tone of the source. Action pages → tense, fast. Comedy pages → light, dry. Quiet pages → still, observational.
- Never break the fourth wall. Never reference "the panel" or "the page."

PACING — YOUTUBE-RECAP HOOKS (ONLY when asked in the user prompt's POSITION block)
- FIRST page of the chapter: open with ONE punchy hook line that sets up the chapter — something a recap creator would say at the top of the video ("This is where it all started to fall apart.", "After everything we'd seen, no one expected what came next."). Then go into the page content normally.
- LAST page of the chapter: close with ONE cliffhanger / lead-out line that leaves the listener wanting the next chapter ("…and that was just the beginning.", "By the time the dust settled, nothing would ever be the same."). Don't fabricate plot points — keep it grounded in what the page actually shows.
- INTERSTITIAL pages (every 3rd middle page): you may open with ONE brief re-grounding line ("By now, …", "With everything that had happened, …") — but only if it adds momentum. Otherwise dive straight in.

OUTPUT FORMAT
End your response with this exact block on a new line, nothing after it:
---MEMORY---
Characters: <comma-separated, key characters present this page>
Setting: <one short clause>
Last event: <one sentence — the most important thing that just happened>
Tension: <one short clause — the unresolved question / threat / stake>
"""


# Combine-pass prompt — runs ONCE over the whole chapter to merge per-page
# narration into a single flowing script with no repetition or "Page N" seams.
COMBINE_SYSTEM_PROMPT = """You are an editor producing the final voice-over script for a YouTube manga recap video. Your input is per-page recap narration that was generated one page at a time. Your job is to weave it into ONE continuous script.

LANGUAGE
- Output STRICTLY in English. Any non-English fragments that slipped through must be translated into natural English.

RULES
- Output PURE prose. No "Page 1", "Page 2", no headers, no bullet points.
- Read each segment in order. If a line of dialogue or a beat is repeated across consecutive segments (the model often re-quotes a line that spanned panels), KEEP IT ONLY ONCE in its strongest form.
- If two consecutive segments both open with "Meanwhile," or "Just then,", drop the second transition and rephrase to keep the rhythm varied.
- Smooth scene-to-scene transitions between segments. Use connective tissue ("From there, …", "As that played out, …", "But the real shift came when …") rather than abrupt cuts.
- Preserve EVERY line of original dialogue verbatim and in quotes. Speaker names stay.
- Keep the hook line at the top and the cliffhanger line at the bottom intact — those are the chapter's bookends.
- Don't add facts that weren't in the source. Don't editorialize. Don't break the fourth wall.

OUTPUT
Just the merged narration. One continuous voice-over script. Nothing else."""



MEMORY_RE = re.compile(r"---MEMORY---\s*\n(.*)", re.DOTALL | re.IGNORECASE)


def build_recap_user_prompt(story_so_far, recent_narration, raw_panel_text,
                            manga_title, chapter_number, page_index, total_pages,
                            cast=None):
    """Assemble the per-page user prompt with rolling context."""
    header_bits = []
    if manga_title:
        header_bits.append(f'Manga: "{manga_title}"')
    if chapter_number is not None:
        header_bits.append(f"Chapter {chapter_number}")
    header_bits.append(f"Page {page_index + 1} of {total_pages}")
    header = " · ".join(header_bits)

    sections = [f"== CHAPTER ==\n{header}"]

    # Position-aware pacing — drives the YouTube-recap hook / cliffhanger /
    # interstitial behaviour described in the system prompt's PACING section.
    position = "first" if page_index == 0 else (
        "last" if page_index == total_pages - 1 else "middle"
    )
    is_interstitial = position == "middle" and page_index > 0 and page_index % 3 == 0

    if position == "first":
        sections.append(
            "== POSITION ==\n"
            "FIRST page of the chapter. Open with ONE punchy hook line "
            "(YouTube recap-style) that sets up the chapter, then narrate "
            "the page normally."
        )
    elif position == "last":
        sections.append(
            "== POSITION ==\n"
            "LAST page of the chapter. After narrating the page, close with "
            "ONE cliffhanger / lead-out line that makes the listener want the "
            "next chapter. Stay grounded in what the page actually shows."
        )
    elif is_interstitial:
        sections.append(
            "== POSITION ==\n"
            "Mid-chapter interstitial. You MAY open with one brief "
            "re-grounding line (e.g. 'By now, …') if it adds momentum. "
            "Otherwise dive straight in."
        )

    # Cast block — known character names + descriptions. The model uses these
    # verbatim instead of inventing generic descriptors ("the boy" → "Akira").
    if cast:
        cast_lines = []
        for c in cast:
            name = (c.get("name") or "").strip()
            if not name:
                continue
            desc = (c.get("description") or "").strip()
            cast_lines.append(f"- {name}{f' ({desc})' if desc else ''}")
        if cast_lines:
            sections.append(
                "== CAST (use these names verbatim when the panel matches) ==\n"
                + "\n".join(cast_lines)
            )

    if story_so_far:
        sections.append(f"== STORY SO FAR ==\n{story_so_far.strip()}")
    elif page_index == 0:
        sections.append(
            "== STORY SO FAR ==\n"
            "(This is the first page of the chapter — start clean. Don't recap "
            "earlier chapters.)"
        )

    if recent_narration:
        sections.append(f"== JUST HAPPENED (your prior narration, do NOT repeat) ==\n{recent_narration.strip()}")

    sections.append(f"== THIS PAGE'S RAW PANEL TEXT ==\n{raw_panel_text.strip()}")

    sections.append(
        "== INSTRUCTIONS ==\n"
        "Continue the recap. 2–4 paragraphs. Past tense, third-person. "
        "Use the same character names you used before. Keep dialogue in quotes. "
        "Don't recap the previous pages — *continue* from them.\n\n"
        "End your response with the ---MEMORY--- block exactly as specified."
    )

    return "\n\n".join(sections)


# ─────────────────────────────────────────────────────────────────────────────
# Combine pass — merges per-page recap into one flowing voice-over script
# ─────────────────────────────────────────────────────────────────────────────

def _deterministic_combine_merge(job, output_dir, reason):
    """
    Mechanical merge fallback used when NO Ollama text model is responding.
    Reads enhanced_detailed.json, joins per-page narration with light
    cleanup, dedupes consecutive duplicate sentences, and writes the
    combined output. The result isn't as polished as a real LLM merge but
    it's coherent enough to send to TTS — we'd rather give the user
    *something* than fail outright.
    """
    detailed_path = os.path.join(output_dir, "enhanced_detailed.json")
    if not os.path.exists(detailed_path):
        emit_error("No enhanced_detailed.json found — generate the script first.")
        return
    with open(detailed_path, "r", encoding="utf-8") as f:
        segments = json.load(f)

    pieces = []
    for s in segments:
        text = (s.get("text") or "").strip()
        if text:
            pieces.append(text)

    if not pieces:
        emit_error("No narration found to combine — every page is empty.")
        return

    # Light de-duplication: split each segment into sentences, drop a
    # sentence if it's identical (case-insensitive) to the immediately
    # preceding one across all segments.
    sentence_re = re.compile(r'(?<=[.!?…])\s+(?=[A-Z"\'"])')
    merged_sentences = []
    last = ""
    for piece in pieces:
        for sent in sentence_re.split(piece):
            sent = sent.strip()
            if not sent:
                continue
            if sent.lower() == last.lower():
                continue  # consecutive duplicate
            merged_sentences.append(sent)
            last = sent

    merged = " ".join(merged_sentences).strip()

    out_path = os.path.join(output_dir, "enhanced_combined.json")
    payload = {
        "model": "deterministic-fallback",
        "source_pages": len(pieces),
        "chunks": 0,
        "fallback_reason": reason[:300],
        "text": merged,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)

    emit({"type": "info",
          "message": f"Deterministic merge complete — {len(merged_sentences)} sentences, {len(merged):,} chars."})
    emit({"type": "done",
          "output_file": "enhanced_combined.json",
          "char_count": len(merged),
          "source_pages": len(pieces),
          "chunks": 0,
          "model": "deterministic-fallback"})


def run_combine_pass(job):
    """
    Read per-page recap segments and produce one continuous voice-over script
    with no repetition and smooth scene transitions. Saves to
    enhanced_combined.json so the renderer can show it on a Combine tab.
    """
    output_dir = job["output_dir"]
    ollama_url = job.get("ollama_url", "http://127.0.0.1:11434")
    model = job.get("model", "llama3.1:8b")
    temperature = job.get("temperature", 0.6)
    fallback_models = list(job.get("fallback_models") or [])

    # Pre-flight probe — same crash-protection we have on detailed mode.
    candidates = [model] + [m for m in fallback_models if m != model]
    chosen, probe_err = pick_working_model(ollama_url, candidates)
    if chosen is None:
        # No LLM works — but for COMBINE we can still produce a usable
        # output by mechanically merging the per-page narration. The user
        # gets *something* instead of an error message.
        emit({"type": "info",
              "message": f"No LLM responded ({probe_err[:80]}) — falling back to deterministic merge."})
        try:
            return _deterministic_combine_merge(job, output_dir, probe_err)
        except Exception as e:
            emit_error(
                f"No LLM responded ({probe_err[:120]}) and deterministic "
                f"fallback also failed: {e}"
            )
            return
    if chosen != model:
        emit({"type": "info",
              "message": f"Switched to {chosen} — {model} wasn't responding ({probe_err[:60]})."})
        model = chosen

    detailed_path = os.path.join(output_dir, "enhanced_detailed.json")
    if not os.path.exists(detailed_path):
        emit_error("No enhanced_detailed.json found — generate the script first.")
        return

    try:
        with open(detailed_path, "r", encoding="utf-8") as f:
            segments = json.load(f)
    except Exception as e:
        emit_error(f"Cannot read enhanced_detailed.json: {e}")
        return

    # Collect non-empty narration segments
    pieces = []
    for s in segments:
        text = (s.get("text") or "").strip()
        if text:
            pieces.append(text)

    if not pieces:
        emit_error("No narration found to combine — every page is empty.")
        return

    max_chars = int(job.get("max_combine_chars", 24000))
    total_chars = sum(len(p) for p in pieces) + len(pieces) * 20  # account for separators

    # ── Build chunks ────────────────────────────────────────────────────
    # If the chapter is small enough, one pass merges everything. Otherwise
    # we group consecutive pages so each chunk stays under `max_chars`, then
    # merge each chunk separately, then merge the chunk-merges in a final
    # pass. Three-stage funnel keeps every prompt within the model's window.
    chunks = []  # list of list[str]
    if total_chars <= max_chars:
        chunks = [pieces]
    else:
        cur = []  # list[str]
        cur_chars = 0
        for p in pieces:
            extra = len(p) + 20
            if cur and (cur_chars + extra) > max_chars:
                chunks.append(cur)
                cur = [p]
                cur_chars = extra
            else:
                cur.append(p)
                cur_chars += extra
        if cur:
            chunks.append(cur)

    emit({"type": "info",
          "message": f"Combining {len(pieces)} pages "
                     f"({total_chars:,} chars) → "
                     f"{len(chunks)} chunk(s) using {model}…"})

    def merge_one(segments_chunk, label="chunk"):
        joined = "\n\n---NEXT PAGE---\n\n".join(segments_chunk)
        user_prompt = (
            "== PER-PAGE NARRATION SEGMENTS (in order, separated by ---NEXT PAGE---) ==\n"
            f"{joined}\n\n"
            "== TASK ==\n"
            "Merge these into ONE continuous voice-over script. "
            "De-duplicate repeated dialogue. Smooth transitions between pages. "
            "Keep the hook at the start and the cliffhanger at the end. "
            "Output prose only — no Page-N headers."
        )
        return call_ollama_chat(
            ollama_url, model, COMBINE_SYSTEM_PROMPT, user_prompt, temperature
        )

    # ── Pass 1: merge each chunk ────────────────────────────────────────
    chunk_results = []  # list[str]
    total_steps = len(chunks) + (1 if len(chunks) > 1 else 0)
    for i, chunk in enumerate(chunks):
        emit_progress(i + 1, total_steps)
        try:
            merged_chunk = merge_one(chunk, label=f"chunk {i+1}/{len(chunks)}")
        except Exception as e:
            err_str = str(e)
            # Same auto-fallback as the detailed flow — if Ollama crashed,
            # walk the fallback list and retry this chunk with the next
            # working model. Subsequent chunks also use the new model.
            if is_ollama_crash(err_str):
                remaining = [m for m in fallback_models if m != model and m]
                next_model, _ = pick_working_model(ollama_url, remaining)
                if next_model:
                    emit({"type": "info",
                          "message": f"{model} crashed during chunk {i+1} — falling back to {next_model}."})
                    model = next_model
                    try:
                        merged_chunk = merge_one(chunk, label=f"chunk {i+1}/{len(chunks)} (retry)")
                        chunk_results.append(merged_chunk)
                        continue
                    except Exception as e2:
                        err_str = f"{err_str} (fallback {next_model} also failed: {e2})"
            emit({"type": "info",
                  "message": f"Combine chunk {i+1} failed ({err_str[:80]}) — using deterministic merge instead."})
            try:
                return _deterministic_combine_merge(job, output_dir, err_str)
            except Exception as e3:
                emit_error(f"Combine chunk {i+1}/{len(chunks)} failed: {err_str} (fallback also failed: {e3})")
                return
        chunk_results.append(merged_chunk)

    # ── Pass 2: merge the chunk-merges (only if we had >1 chunk) ────────
    if len(chunk_results) == 1:
        merged = chunk_results[0]
    else:
        emit_progress(total_steps, total_steps)
        try:
            merged = merge_one(chunk_results, label="final pass")
        except Exception as e:
            # If the final merge fails, fall back to concatenation with smooth
            # connectives so the user still gets *something* usable.
            emit({"type": "info",
                  "message": f"Final merge failed ({e}); concatenating chunks instead."})
            merged = "\n\n".join(chunk_results)

    # Defensive cleanup — strip any residual page markers / preambles
    merged = re.sub(r"(?im)^\s*page\s*\d+\s*[:\-]?\s*\n", "", merged).strip()
    merged = re.sub(r"^(here['']?s|here is)[^.\n]*[.:]?\s*", "", merged, count=1, flags=re.IGNORECASE).strip()

    out_path = os.path.join(output_dir, "enhanced_combined.json")
    payload = {
        "model": model,
        "source_pages": len(pieces),
        "chunks": len(chunks),
        "text": merged,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)

    emit({"type": "done", "output_file": "enhanced_combined.json",
          "char_count": len(merged), "source_pages": len(pieces),
          "chunks": len(chunks), "model": model})


def parse_memory_block(model_output):
    """Split the model output into (narration, memory_dict)."""
    match = MEMORY_RE.search(model_output)
    if not match:
        return model_output.strip(), None

    narration = model_output[:match.start()].rstrip()
    raw = match.group(1).strip()

    memory = {}
    for line in raw.splitlines():
        line = line.strip()
        if not line or ":" not in line:
            continue
        key, _, value = line.partition(":")
        key = key.strip().lower()
        value = value.strip()
        if key in {"characters", "setting", "last event", "tension"} and value:
            memory[key] = value

    if not memory:
        return narration, None
    return narration, memory


def merge_story_digest(prev_digest, new_memory):
    """
    Build a compact rolling story-so-far digest from the latest memory block.
    Keeps it under ~500 chars so the model isn't drowning in context.
    """
    if not new_memory:
        return prev_digest

    # Carry forward character names — they accumulate across the chapter
    prev_chars = set()
    if prev_digest:
        m = re.search(r"Characters?:\s*(.+)", prev_digest, re.IGNORECASE)
        if m:
            prev_chars = {c.strip() for c in m.group(1).split(",") if c.strip()}

    new_chars = {c.strip() for c in new_memory.get("characters", "").split(",") if c.strip()}
    all_chars = sorted(prev_chars | new_chars)
    chars_line = ", ".join(all_chars[:12])  # cap at 12 to keep prompt tight

    digest_lines = [
        f"Characters: {chars_line}" if chars_line else None,
        f"Setting: {new_memory.get('setting', '')}".strip(": "),
        f"Last event: {new_memory.get('last event', '')}".strip(": "),
        f"Tension: {new_memory.get('tension', '')}".strip(": "),
    ]
    digest = "\n".join(line for line in digest_lines if line and not line.endswith(":"))
    # Hard cap
    if len(digest) > 600:
        digest = digest[:600] + "…"
    return digest


# =============================================================================
# Ollama call
# =============================================================================

# ─────────────────────────────────────────────────────────────────────────────
# Crash detection + model health probe — same pattern we used for vision.
# Local Ollama text models on Apple Silicon sometimes crash with
# "llama runner process has terminated" (Metal allocator issue, OOM, or a
# bad quantization). When that happens we auto-fall-back to the next
# installed text model rather than failing the whole run.
# ─────────────────────────────────────────────────────────────────────────────

def is_ollama_crash(err_str: str) -> bool:
    s = err_str.lower()
    return (
        "http 500" in s
        or "llama runner" in s
        or "process has terminated" in s
        or "connection refused" in s
        or "timed out" in s
    )


def text_model_health_check(ollama_url, model, timeout=120):
    """One tiny call to confirm the model loads + responds.

    Returns (ok, error_str). When False, error_str describes WHY so the
    caller can surface it to the user. The default 120s timeout is generous
    on purpose — cold-loading a 7B model on a slow disk can easily take
    60-90s before Ollama even starts producing the first token.
    """
    payload = json.dumps({
        "model": model,
        "stream": False,
        "options": {"temperature": 0.1, "num_predict": 16},
        "messages": [{"role": "user", "content": "Reply with: OK"}],
    }).encode("utf-8")
    url = f"{ollama_url.rstrip('/')}/api/chat"
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if "message" in data or "response" in data:
                return True, ""
            return False, "empty response"
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:200]
        msg = f"HTTP {e.code}: {body}"
        print(f"[enhance] {model} health probe {msg}", file=sys.stderr)
        return False, msg
    except urllib.error.URLError as e:
        msg = f"connection error: {e.reason}"
        print(f"[enhance] {model} health probe {msg}", file=sys.stderr)
        return False, msg
    except Exception as e:
        print(f"[enhance] {model} health probe failed: {e}", file=sys.stderr)
        return False, str(e)


def pick_working_model(ollama_url, candidates):
    """Return (model_name, last_error) — `model_name` is None if every
    candidate failed; `last_error` describes why the last one didn't work
    (used to build a clear user-facing diagnostic)."""
    seen = set()
    last_err = ""
    for m in candidates:
        if not m or m in seen:
            continue
        seen.add(m)
        emit({"type": "info", "message": f"Probing text model: {m}…"})
        ok, err = text_model_health_check(ollama_url, m)
        if ok:
            return m, ""
        last_err = err or "unknown"
        emit({"type": "info", "message": f"{m} unavailable ({last_err[:60]}) — trying next"})
    return None, last_err


def call_ollama_chat(ollama_url, model, system_prompt, user_text, temperature=0.7):
    """Use the chat endpoint so we can have a clear system/user split.

    `num_predict` is intentionally modest (800) — Apple Silicon Metal
    allocator crashes ("llama runner terminated") get more frequent as the
    output buffer grows. 800 tokens is plenty for 2-4 narration paragraphs."""
    payload = {
        "model": model,
        "stream": False,
        "options": {"temperature": temperature, "num_predict": 800, "top_p": 0.9},
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_text},
        ],
    }

    url = f"{ollama_url.rstrip('/')}/api/chat"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})

    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            result = json.loads(resp.read().decode("utf-8"))
            return result.get("message", {}).get("content", "").strip()
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:500]
        raise RuntimeError(f"Ollama HTTP {e.code}: {body}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Cannot connect to Ollama at {ollama_url}: {e.reason}")


def call_ollama_generate(ollama_url, model, system_prompt, text, temperature=0.7):
    """Legacy single-prompt call — used for stateless modes."""
    payload = {
        "model": model,
        "stream": False,
        "system": system_prompt,
        "prompt": text,
        "options": {"temperature": temperature},
    }

    url = f"{ollama_url.rstrip('/')}/api/generate"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})

    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            result = json.loads(resp.read().decode("utf-8"))
            return result.get("response", "").strip()
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:500]
        raise RuntimeError(f"Ollama HTTP {e.code}: {body}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Cannot connect to Ollama at {ollama_url}: {e.reason}")


# =============================================================================
# Detailed (recap) flow with rolling memory
# =============================================================================

def run_detailed_with_memory(job):
    segments = job["segments"]
    output_dir = job["output_dir"]
    ollama_url = job.get("ollama_url", "http://127.0.0.1:11434")
    model = job.get("model", "llama3.1:8b")
    temperature = job.get("temperature", 0.7)
    context_pages = int(job.get("context_pages", 2))
    manga_title = job.get("manga_title", "") or ""
    chapter_number = job.get("chapter_number")
    cast = job.get("cast") or []  # [{name, description}]
    # Ordered list of fallback models — when the current one crashes mid-loop,
    # the next one is tried. The renderer builds this from installed Ollama
    # text models, prioritising smaller / more reliable ones.
    fallback_models = list(job.get("fallback_models") or [])

    os.makedirs(output_dir, exist_ok=True)

    # ── Pre-flight: probe the chosen model ──────────────────────────────
    # Same fix we made for vision. If the chosen model crashes the probe,
    # walk the fallback list and pick the first one that responds.
    candidates = [model] + [m for m in fallback_models if m != model]
    chosen, probe_err = pick_working_model(ollama_url, candidates)
    if chosen is None:
        emit_error(
            f"No installed Ollama text model is responding "
            f"(last error: {probe_err[:200]}). "
            f"Try: `ollama serve` in a terminal, then confirm with `ollama list`. "
            f"If a model is installed but crashing, try a smaller one "
            f"(e.g. `ollama pull llama3.2:3b`)."
        )
        return
    if chosen != model:
        emit({"type": "info",
              "message": f"Switched to {chosen} — {model} wasn't responding ({probe_err[:60]})."})
        model = chosen

    total = len(segments)
    emit({"type": "info",
          "message": f"Recap-narrating {total} pages of "
                     f"{manga_title or 'this chapter'} using {model}"})

    results = []
    memories = []          # parallel array of per-page memory dicts
    story_digest = ""      # rolling story-so-far summary
    recent_outputs = []    # last K enhanced narrations (verbatim)

    for i, seg in enumerate(segments):
        emit_progress(i + 1, total)

        original_text = seg.get("text", "") or ""
        if not original_text.strip():
            results.append({"page": seg.get("page", i), "text": "", "original": ""})
            memories.append(None)
            # don't add empty pages to recent_outputs — they'd dilute context
            continue

        recent_blob = "\n\n".join(
            f"[Page {p}]\n{t}" for (p, t) in recent_outputs[-context_pages:]
        )

        user_prompt = build_recap_user_prompt(
            story_so_far=story_digest,
            recent_narration=recent_blob,
            raw_panel_text=original_text,
            manga_title=manga_title,
            chapter_number=chapter_number,
            page_index=i,
            total_pages=total,
            cast=cast,
        )

        try:
            raw_output = call_ollama_chat(
                ollama_url, model, RECAP_SYSTEM_PROMPT, user_prompt, temperature
            )
            narration, memory = parse_memory_block(raw_output)

            # Defensive cleanup — strip any stray ---MEMORY--- echoes,
            # leading "Page N:" labels, or "Here's the recap:" preambles.
            narration = re.sub(r"^(Here['']?s.*?:|Page\s*\d+\s*:|Recap:)\s*",
                               "", narration, flags=re.IGNORECASE).strip()

            results.append({
                "page": seg.get("page", i),
                "text": narration,
                "original": original_text,
            })
            memories.append(memory)
            recent_outputs.append((seg.get("page", i) + 1, narration))
            # Cap recent_outputs to the configured window so the prompt stays small
            if len(recent_outputs) > context_pages:
                recent_outputs = recent_outputs[-context_pages:]

            if memory:
                story_digest = merge_story_digest(story_digest, memory)

        except Exception as e:
            err_str = str(e)
            print(f"[enhance] Failed on page {i}: {err_str}", file=sys.stderr)

            # If the model crashed (Ollama "llama runner terminated"),
            # walk the fallback list and switch to the first one that probes
            # OK, then retry the current page with that model. Subsequent
            # pages also use the new model.
            if is_ollama_crash(err_str):
                remaining = [m for m in fallback_models if m != model and m not in (None, "")]
                next_model, _ = pick_working_model(ollama_url, remaining)
                if next_model:
                    emit({"type": "info",
                          "message": f"{model} crashed — falling back to {next_model} for the rest of the chapter."})
                    model = next_model
                    # Retry this page once with the new model
                    try:
                        raw_output = call_ollama_chat(
                            ollama_url, model, RECAP_SYSTEM_PROMPT, user_prompt, temperature
                        )
                        narration, memory = parse_memory_block(raw_output)
                        narration = re.sub(r"^(Here['']?s.*?:|Page\s*\d+\s*:|Recap:)\s*",
                                           "", narration, flags=re.IGNORECASE).strip()
                        results.append({
                            "page": seg.get("page", i),
                            "text": narration,
                            "original": original_text,
                        })
                        memories.append(memory)
                        recent_outputs.append((seg.get("page", i) + 1, narration))
                        if len(recent_outputs) > context_pages:
                            recent_outputs = recent_outputs[-context_pages:]
                        if memory:
                            story_digest = merge_story_digest(story_digest, memory)
                        continue
                    except Exception as e2:
                        err_str = f"{err_str} (fallback {next_model} also failed: {e2})"

            results.append({
                "page": seg.get("page", i),
                "text": original_text,
                "original": original_text,
                "error": err_str,
            })
            memories.append(None)

    # Save narration
    output_path = os.path.join(output_dir, "enhanced_detailed.json")
    with open(output_path, "w") as f:
        json.dump(results, f, indent=2)

    # Save memory chain alongside — useful for debugging / future resume
    memory_path = os.path.join(output_dir, "enhanced_detailed_memory.json")
    memory_payload = {
        "manga_title": manga_title,
        "chapter_number": chapter_number,
        "model": model,
        "final_digest": story_digest,
        "per_page": [
            {"page": (segments[i].get("page", i)), "memory": memories[i]}
            for i in range(len(memories))
        ],
    }
    with open(memory_path, "w") as f:
        json.dump(memory_payload, f, indent=2)

    successful = sum(1 for r in results if r.get("text") and not r.get("error"))
    emit({"type": "info", "message": f"Recap-narrated {successful}/{total} pages"})
    emit({"type": "done", "output_file": "enhanced_detailed.json",
          "successful": successful, "total": total})


# =============================================================================
# Stateless flow (creative / exaggerated)
# =============================================================================

def run_stateless(job, mode):
    segments = job["segments"]
    output_dir = job["output_dir"]
    ollama_url = job.get("ollama_url", "http://127.0.0.1:11434")
    model = job.get("model", "llama3.1:8b")
    temperature = job.get("temperature", 0.7)

    os.makedirs(output_dir, exist_ok=True)
    system_prompt = STATELESS_PROMPTS[mode]
    emit({"type": "info",
          "message": f"Enhancing {len(segments)} segments in '{mode}' mode using {model}"})

    results = []
    for i, seg in enumerate(segments):
        emit_progress(i + 1, len(segments))

        original_text = seg.get("text", "")
        if not original_text or not original_text.strip():
            results.append({"page": seg.get("page", i), "text": "", "original": ""})
            continue

        try:
            enhanced = call_ollama_generate(
                ollama_url, model, system_prompt, original_text, temperature
            )
            results.append({
                "page": seg.get("page", i),
                "text": enhanced,
                "original": original_text,
            })
        except Exception as e:
            print(f"[enhance] Failed on segment {i}: {e}", file=sys.stderr)
            results.append({
                "page": seg.get("page", i),
                "text": original_text,
                "original": original_text,
                "error": str(e),
            })

    output_path = os.path.join(output_dir, f"enhanced_{mode}.json")
    with open(output_path, "w") as f:
        json.dump(results, f, indent=2)

    successful = sum(1 for r in results if r.get("text") and not r.get("error"))
    emit({"type": "info", "message": f"Enhanced {successful}/{len(results)} segments"})
    emit({"type": "done", "output_file": f"enhanced_{mode}.json",
          "successful": successful, "total": len(results)})


# =============================================================================
# Entry
# =============================================================================

def run_enhancement(job):
    mode = job.get("mode", "creative")
    if mode == "detailed":
        run_detailed_with_memory(job)
    elif mode == "combine":
        run_combine_pass(job)
    elif mode in STATELESS_PROMPTS:
        run_stateless(job, mode)
    else:
        emit_error(f"Unknown enhancement mode: {mode}. Use: creative, exaggerated, detailed, combine")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 text_enhance.py <job.json>", file=sys.stderr)
        sys.exit(1)

    try:
        with open(sys.argv[1], "r") as f:
            job = json.load(f)
    except Exception as e:
        emit_error(f"Failed to read job file: {e}")
        sys.exit(1)

    try:
        run_enhancement(job)
    except Exception as e:
        emit_error(f"Fatal: {e}")
        import traceback
        traceback.print_exc(file=sys.stderr)
        sys.exit(1)
