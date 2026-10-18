#!/usr/bin/env python3
"""
MangaVox Vision LLM Text Extraction
Uses local Ollama vision models to read manga/comic pages in a narration style.
Recommended models: minicpm-v:8b, gemma3:12b, llava-llama3:8b
100% free — runs on your Mac, no API key needed.
Communicates via JSON lines on stdout.
Usage: python3 vision_extract.py <job.json>
"""
import sys
import json
import os
import base64
import urllib.request
import urllib.error
import io

def emit(obj):
    print(json.dumps(obj), flush=True)

def emit_progress(current, total):
    emit({"type": "progress", "current": current, "total": total, "percent": round(current / max(total, 1) * 100, 1)})

def emit_page(page, file, text, error=None):
    """Stream a single page's extracted text to the renderer the moment it's
    ready, so the user sees text appearing live instead of staring at a
    progress bar. The renderer appends this to its segments[] state."""
    msg = {"type": "page", "page": page, "file": file, "text": text or ""}
    if error:
        msg["error"] = str(error)
    emit(msg)

def emit_error(message):
    emit({"type": "error", "message": message})

# Structured manga extraction prompt — the model reads the page like a human manga
# reader, following correct panel/bubble order, and outputs structured text per panel.
SYSTEM_PROMPT = (
    "You are a precise manga page reader. Your ONLY job is to extract the STORY TEXT from this page "
    "in the correct reading order. You reconstruct what a human reader experiences — nothing more.\n\n"

    "== WHAT TO READ ==\n"
    "• DIALOGUE BUBBLES: Round/oval shapes with a tail pointing to a speaker. These are the most important. "
    "The tail tells you who is speaking.\n"
    "• NARRATION BOXES: Hard rectangular boxes (usually top/bottom of panel) with no tail. "
    "These are inner thoughts or narrator voice.\n"
    "• ON-SCREEN TEXT: Text on computer screens, phones, books, signs, or posters INSIDE the story.\n"
    "• EMPHASIS: Larger/bold text or jagged spiky bubbles = shouting. Note the tone.\n\n"

    "== WHAT TO COMPLETELY IGNORE ==\n"
    "• Publisher credits, logos, magazine names in margins\n"
    "• Author/artist names (small text in margins)\n"
    "• Disclaimer text (e.g. 'this manga is fiction...')\n"
    "• Decorative chapter titles or volume info\n"
    "• Sound effects embedded in artwork (large stylized katakana like カチ, ドン, etc.) — "
    "UNLESS they add critical context (explosion, door slam), then note briefly as [SFX: bang]\n"
    "• Page numbers\n"
    "• Advertisements\n\n"

    "== READING ORDER (CRITICAL) ==\n"
    "BETWEEN PANELS: Find the panel furthest to the TOP-RIGHT — that is Panel 1. "
    "Move LEFT across the row. When the row ends, drop to the next row, start from RIGHT again.\n"
    "INSIDE A PANEL: Read the speech bubble furthest to the RIGHT first, then move left. "
    "If a narration box is at the TOP of the panel, read it BEFORE dialogue. "
    "If at the BOTTOM, read it AFTER dialogue.\n\n"

    "== OUTPUT FORMAT ==\n"
    "For each panel, output in this exact structure:\n"
    "PANEL [number]\n"
    "[Type] Speaker: \"Text\" (tone)\n\n"
    "Types:\n"
    "• [DIALOGUE] Character Name or Unknown: \"what they say\" (tone: calm/shouting/whispering/sarcastic/etc.)\n"
    "• [NARRATION] N/A: the narration or inner thought text (tone)\n"
    "• [ON-SCREEN] N/A: text shown on a screen/sign/book\n"
    "• [SFX] N/A: only for critical sound effects\n\n"

    "RULES:\n"
    "- Always write in English. Translate any Japanese, Chinese, Korean text.\n"
    "- If a bubble has no tail and no clear owner, use 'Unknown' as speaker.\n"
    "- If text overlaps two panels, assign to whichever it overlaps more.\n"
    "- For flashback panels (darker, rough border), add [FLASHBACK] before the panel number.\n"
    "- If the page is purely visual with no story text, write: [VISUAL ONLY] Brief scene description.\n"
    "- Do NOT describe the art. Do NOT write 'In this panel we see...'. ONLY extract story text.\n"
    "- Do NOT add commentary or analysis. Just the structured text extraction."
)

USER_PROMPT = "Read this manga page. Extract all story text in correct right-to-left panel order using the structured format."


def prepare_image(image_path, max_dimension=1536):
    """
    Prepare image for vision model — upscale small images and ensure quality.
    Returns base64-encoded image data.
    """
    try:
        from PIL import Image
        img = Image.open(image_path)
        w, h = img.size

        # Convert to RGB if needed (some models choke on RGBA/P mode)
        if img.mode not in ('RGB', 'L'):
            img = img.convert('RGB')

        # If image is too small, upscale for better OCR
        if max(w, h) < 800:
            scale = 1200 / max(w, h)
            img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)

        # If image is huge, downscale to prevent memory issues
        if max(w, h) > max_dimension:
            scale = max_dimension / max(w, h)
            img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)

        # Convert to high-quality PNG for the model
        buf = io.BytesIO()
        img.save(buf, format="PNG", quality=95)
        return base64.b64encode(buf.getvalue()).decode("utf-8")
    except ImportError:
        # PIL not available — just read the raw file
        with open(image_path, "rb") as f:
            return base64.b64encode(f.read()).decode("utf-8")


def call_ollama_vision(ollama_url, model, image_path, temperature=0.3):
    """Send an image to a local Ollama vision model and extract text."""
    image_b64 = prepare_image(image_path)

    payload = {
        "model": model,
        "stream": False,
        "options": {
            "temperature": temperature,
            "num_predict": 4096,
            "top_p": 0.9,
        },
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {
                "role": "user",
                "content": USER_PROMPT,
                "images": [image_b64],
            },
        ],
    }

    url = f"{ollama_url.rstrip('/')}/api/chat"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})

    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            result = json.loads(resp.read().decode("utf-8"))
            text = result.get("message", {}).get("content", "").strip()

            # Post-process: remove common model preamble artifacts
            preambles = [
                "Here is the text from the manga page:",
                "Here's the transcribed text:",
                "The text on this page reads:",
                "Text from this page:",
                "Here is the extracted text:",
                "Here's the text on this page:",
                "The text reads:",
                "I can see the following text:",
                "Reading the page:",
                "Here's the narration:",
                "Here is the narration for this page:",
                "Here's my narration of this manga page:",
                "Narrating this page:",
                "Here is the structured extraction:",
                "Here's the structured text extraction:",
                "Here is the text extraction:",
                "Reading this manga page:",
            ]
            lower = text.lower()
            for prefix in preambles:
                if lower.startswith(prefix.lower()):
                    text = text[len(prefix):].strip()
                    break

            # Remove any trailing meta-commentary the model adds
            for suffix_marker in ["\n\nNote:", "\n\n(Note:", "\n\nThis page", "\n\nThe page",
                                  "\n\n---", "\n\n*This", "\n\n(This", "\n\nI hope",
                                  "\n\nLet me know"]:
                idx = text.find(suffix_marker)
                if idx > 0:
                    text = text[:idx].strip()

            return text
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:500]
        raise RuntimeError(f"Ollama HTTP {e.code}: {body}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Cannot connect to Ollama at {ollama_url}: {e.reason}")


def call_gemini_vision(api_key, image_path, model=None):
    """Use Google Gemini API (free tier) — excellent quality, fast."""
    import time as _time

    # Use the specified model, defaulting to gemini-2.0-flash if not set
    model_name = model if model else 'gemini-2.0-flash'

    with open(image_path, "rb") as f:
        image_data = base64.b64encode(f.read()).decode("utf-8")

    ext = os.path.splitext(image_path)[1].lower()
    mime_map = {'.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.png': 'image/png',
                '.webp': 'image/webp', '.gif': 'image/gif'}
    mime_type = mime_map.get(ext, 'image/jpeg')

    payload = json.dumps({
        "contents": [{
            "parts": [
                {"text": "Narrate this manga/comic page. Tell me what's happening and what everyone is saying."},
                {"inline_data": {"mime_type": mime_type, "data": image_data}},
            ]
        }],
        "systemInstruction": {"parts": [{"text": SYSTEM_PROMPT}]},
        "generationConfig": {"temperature": 0.4, "maxOutputTokens": 4096, "topP": 0.9}
    }).encode("utf-8")

    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model_name}:generateContent?key={api_key}"
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})

    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                result = json.loads(resp.read().decode("utf-8"))
                candidates = result.get("candidates", [])
                if not candidates:
                    return ""
                parts = candidates[0].get("content", {}).get("parts", [])
                return "".join(p.get("text", "") for p in parts).strip()
        except urllib.error.HTTPError as e:
            if e.code == 429:
                wait = 5 * (attempt + 1)
                print(f"[vision] Gemini rate limited, waiting {wait}s...", file=sys.stderr)
                _time.sleep(wait)
                continue
            body = e.read().decode("utf-8", errors="replace")[:300]
            raise RuntimeError(f"Gemini API {e.code}: {body}")
    raise RuntimeError("Gemini API: too many retries")


# =============================================================================
# Ollama health probe — single-page test call to detect model-crash failures
# (the "llama runner process has terminated" pattern) before we run a whole
# chapter through it.
# =============================================================================

def ollama_health_check(ollama_url, model, sample_image_path, timeout=45):
    """
    Try one minimal call to the Ollama vision model. Returns True if it
    responds successfully (any non-error response counts), False otherwise.
    On False the caller should switch to PaddleOCR.
    """
    try:
        image_b64 = prepare_image(sample_image_path, max_dimension=512)  # tiny payload
    except Exception as e:
        print(f"[vision] health probe: cannot prepare image: {e}", file=sys.stderr)
        return False

    payload = json.dumps({
        "model": model,
        "stream": False,
        "options": {"temperature": 0.1, "num_predict": 32},
        "messages": [
            {"role": "user", "content": "Reply with: OK", "images": [image_b64]},
        ],
    }).encode("utf-8")

    url = f"{ollama_url.rstrip('/')}/api/chat"
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            result = json.loads(resp.read().decode("utf-8"))
            # Even an empty content is fine — what matters is that the runner
            # didn't terminate. Errors bubble up as HTTPError below.
            return "message" in result or "response" in result
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:300]
        print(f"[vision] health probe HTTP {e.code}: {body}", file=sys.stderr)
        return False
    except Exception as e:
        print(f"[vision] health probe failed: {e}", file=sys.stderr)
        return False


# =============================================================================
# PaddleOCR engine — same approach AI Manga Reader uses (PP-OCRv4 in their
# case as ONNX, here as native PaddleOCR). Stable, no LLM crashes, fast.
# =============================================================================

_paddle_ocr_singleton = [None]  # lazy-init holder


def get_paddle_ocr(lang="en"):
    """Lazily load and reuse a single PaddleOCR instance."""
    if _paddle_ocr_singleton[0] is not None:
        return _paddle_ocr_singleton[0]

    # Skip the slow PaddleX model-source connectivity check — models are already
    # cached locally on second run, and the check fires before that lookup.
    os.environ.setdefault("PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK", "True")

    try:
        from paddleocr import PaddleOCR
    except ImportError as e:
        raise RuntimeError(
            "PaddleOCR is not installed. Install with: "
            "pip install paddleocr paddlepaddle"
        ) from e

    # PaddleOCR's API has shifted across versions. Newer releases (v3+) renamed
    # `use_angle_cls` → `use_textline_orientation` and removed `show_log`. Try
    # the modern signature first, fall back to the legacy one for older installs.
    last_err = None
    for kwargs in (
        {"use_textline_orientation": True, "lang": lang},
        {"use_angle_cls": True, "lang": lang, "show_log": False},
        {"lang": lang},  # bare-minimum fallback
    ):
        try:
            ocr = PaddleOCR(**kwargs)
            _paddle_ocr_singleton[0] = ocr
            return ocr
        except TypeError as e:
            last_err = e
            continue
    raise RuntimeError(f"Could not initialize PaddleOCR: {last_err}")


def call_paddle_ocr(image_path, lang="en", confidence_threshold=0.5):
    """
    Run PaddleOCR on a single image and return the recognized text joined by
    newlines, plus the structured boxes for downstream overlay rendering.

    PaddleOCR has TWO output shapes depending on version:
      • Legacy (<=2.x): `[[ [box, (text, conf)], ... ]]` — list of pages
      • Modern (3.x):   `[{rec_texts: [...], rec_scores: [...], rec_polys: [...]}]`
    We handle both.
    """
    ocr = get_paddle_ocr(lang)

    # Try new API first (just `ocr.ocr(path)` — no `cls=` kwarg in v3)
    try:
        result = ocr.ocr(image_path)
    except TypeError:
        result = ocr.ocr(image_path, cls=True)

    lines, boxes = [], []
    if not result:
        return "", []

    first = result[0]

    # Modern dict-based output
    if isinstance(first, dict):
        texts = first.get("rec_texts") or first.get("texts") or []
        scores = first.get("rec_scores") or first.get("scores") or []
        polys = first.get("rec_polys") or first.get("dt_polys") or first.get("polys") or []
        for i, text in enumerate(texts):
            conf = float(scores[i]) if i < len(scores) else 1.0
            if conf < confidence_threshold or not text or not text.strip():
                continue
            poly = polys[i] if i < len(polys) else None
            lines.append(text.strip())
            box_points = []
            if poly is not None:
                try:
                    box_points = [[int(p[0]), int(p[1])] for p in poly]
                except Exception:
                    box_points = []
            boxes.append({"points": box_points, "confidence": round(conf, 3)})
        return "\n".join(lines), boxes

    # Legacy list-of-tuples output
    if isinstance(first, list):
        for line in first:
            try:
                box, (text, conf) = line[0], line[1]
            except Exception:
                continue
            if conf >= confidence_threshold and text and text.strip():
                lines.append(text.strip())
                boxes.append({
                    "points": [[int(p[0]), int(p[1])] for p in box],
                    "confidence": round(float(conf), 3),
                })
    return "\n".join(lines), boxes


# =============================================================================
# HuggingFace moondream2 — offline vision model (~3.5 GB once downloaded).
# Loaded lazily on first use; cached in memory across pages.
# =============================================================================

def load_hf_moondream():
    """Import deps + load moondream2. Raises RuntimeError on missing deps."""
    os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
    try:
        import torch  # type: ignore
        from transformers import AutoModelForCausalLM, AutoTokenizer  # type: ignore
    except Exception as exc:
        raise RuntimeError(
            "HuggingFace transformers + torch are required for HF mode. "
            "Install with: pip install transformers==4.44.2 torch Pillow"
        ) from exc

    # moondream2's bundled `modeling_phi.py` reads `config.pad_token_id`,
    # which transformers >=4.50 removed. Pinning to revision 2024-08-26
    # keeps us compatible. If user has newer transformers, the model load
    # may fail with a clear error — surface that to the UI.
    revision = "2024-08-26"
    model_id = "vikhyatk/moondream2"

    device = "cuda" if torch.cuda.is_available() else (
        "mps" if hasattr(torch.backends, "mps") and torch.backends.mps.is_available() else "cpu"
    )
    dtype = torch.float16 if device != "cpu" else torch.float32

    tokenizer = AutoTokenizer.from_pretrained(
        model_id, revision=revision, trust_remote_code=True,
    )
    model = AutoModelForCausalLM.from_pretrained(
        model_id, revision=revision, trust_remote_code=True,
        torch_dtype=dtype,
    ).to(device)
    model.eval()
    print(f"[hf] moondream2 loaded on {device} ({dtype})", file=sys.stderr)
    return model, tokenizer


HF_QUESTION = (
    "You are a manga page reader. Extract all story text on this page in correct "
    "right-to-left, top-to-bottom panel order. Keep dialogue in quotes with the "
    "speaker (or 'Unknown' if unclear). Skip credits, page numbers, and decorative "
    "SFX. Output structured text only — no commentary."
)


def call_hf_moondream(model, tokenizer, image_path):
    try:
        from PIL import Image  # type: ignore
    except Exception as exc:
        raise RuntimeError("Pillow is required for HF mode") from exc
    image = Image.open(image_path).convert("RGB")
    enc = model.encode_image(image)
    answer = model.answer_question(enc, HF_QUESTION, tokenizer)
    return str(answer).strip()


# =============================================================================
# Main extraction loop
# =============================================================================

def run_extraction(job):
    pages_dir = job["pages_dir"]
    output_dir = job["output_dir"]
    ollama_url = job.get("ollama_url", "http://127.0.0.1:11434")
    model = job.get("model", "minicpm-v:8b")
    temperature = job.get("temperature", 0.3)
    gemini_api_key = job.get("gemini_api_key", "")
    gemini_model = job.get("gemini_model", "")  # User-selected Gemini model version
    # New: explicit engine selector. Values: "auto" | "ollama" | "gemini" | "paddleocr"
    # "auto" = Gemini if key set, else Ollama, with PaddleOCR fallback on crash.
    engine = (job.get("engine") or "auto").lower()
    paddle_lang = job.get("paddle_lang", "en")

    os.makedirs(output_dir, exist_ok=True)

    page_files = sorted([
        os.path.join(pages_dir, f) for f in os.listdir(pages_dir)
        if f.lower().endswith(('.jpg', '.jpeg', '.png', '.webp', '.gif'))
    ])

    if not page_files:
        emit_error(f"No image files found in {pages_dir}")
        return

    # Resolve initial engine choice
    if engine == "auto":
        if gemini_api_key:
            active_engine = "gemini"
        else:
            # Pre-flight: probe Ollama with the smallest possible image. If the
            # model crashes here, switch to PaddleOCR before we waste any time
            # iterating pages. This catches the "llama runner terminated"
            # failure mode immediately instead of after multiple failed pages.
            emit({"type": "info", "message": f"Probing Ollama vision model: {model}…"})
            if ollama_health_check(ollama_url, model, page_files[0]):
                active_engine = "ollama"
            else:
                emit({"type": "info", "message": "Ollama vision model is unavailable — using PaddleOCR instead."})
                active_engine = "paddleocr"
    else:
        active_engine = engine

    # Pre-warm PaddleOCR if we know we'll need it. Loading is slow (~3-5s).
    if active_engine == "paddleocr":
        emit({"type": "info", "message": "Loading PaddleOCR models (one-time, ~5s)…"})
        try:
            get_paddle_ocr(paddle_lang)
        except Exception as e:
            emit_error(f"PaddleOCR unavailable: {e}")
            return

    engine_label = {
        "gemini": "Google Gemini (free)",
        "ollama": f"Ollama ({model})",
        "paddleocr": f"PaddleOCR ({paddle_lang})",
        "hf": "HuggingFace moondream2 (offline)",
    }.get(active_engine, active_engine)
    emit({"type": "info", "message": f"Extracting text from {len(page_files)} pages using {engine_label}"})

    results = []
    boxes_per_page = []  # parallel array for the optional bbox output
    consecutive_empty = 0
    consecutive_crashes = 0  # Ollama-only — auto-fall-back to PaddleOCR after 2

    def page_with_paddle(i, page_path):
        """Run PaddleOCR for one page, return result dict + boxes."""
        try:
            text, boxes = call_paddle_ocr(page_path, paddle_lang)
            return {
                "page": i,
                "file": os.path.basename(page_path),
                "text": text,
            }, boxes
        except Exception as e:
            print(f"[vision] PaddleOCR failed on page {i}: {e}", file=sys.stderr)
            return {
                "page": i,
                "file": os.path.basename(page_path),
                "text": "",
                "error": f"paddleocr: {e}",
            }, []

    # Lazy-loaded HF moondream2 — only initialised if engine == "hf"
    hf_state = {"model": None, "tokenizer": None}

    for i, page_path in enumerate(page_files):
        emit_progress(i + 1, len(page_files))

        # ── PaddleOCR engine ─────────────────────────────────────────────
        if active_engine == "paddleocr":
            r, b = page_with_paddle(i, page_path)
            results.append(r)
            boxes_per_page.append({"page": i, "file": r["file"], "boxes": b})
            emit_page(r["page"], r["file"], r["text"], r.get("error"))
            continue

        # ── HuggingFace moondream2 engine (offline) ─────────────────────
        if active_engine == "hf":
            try:
                if hf_state["model"] is None:
                    emit({"type": "info", "message": "Loading moondream2 weights (one-time, ~3.5 GB cached after first run)…"})
                    hf_state["model"], hf_state["tokenizer"] = load_hf_moondream()
                text = call_hf_moondream(hf_state["model"], hf_state["tokenizer"], page_path)
                results.append({
                    "page": i, "file": os.path.basename(page_path), "text": text,
                })
                emit_page(i, os.path.basename(page_path), text)
            except Exception as e:
                err_str = str(e)
                print(f"[vision] HF failed on page {i}: {err_str}", file=sys.stderr)
                results.append({
                    "page": i, "file": os.path.basename(page_path),
                    "text": "", "error": err_str,
                })
                emit_page(i, os.path.basename(page_path), "", err_str)
            continue

        # ── Gemini / Ollama engines ──────────────────────────────────────
        try:
            if active_engine == "gemini":
                text = call_gemini_vision(gemini_api_key, page_path, gemini_model)
            else:  # ollama
                text = call_ollama_vision(ollama_url, model, page_path, temperature)

            # Reset crash counter on any successful response (even if empty)
            consecutive_crashes = 0

            if not text or len(text.strip()) < 10:
                if active_engine == "gemini":
                    import time as _t; _t.sleep(2)
                    text2 = call_gemini_vision(gemini_api_key, page_path, gemini_model)
                else:
                    text2 = call_ollama_vision(ollama_url, model, page_path, min(temperature + 0.15, 0.8))
                if text2 and len(text2.strip()) > len((text or "").strip()):
                    text = text2

            is_empty = not text or len(text.strip()) < 5

            if is_empty:
                consecutive_empty += 1
                if consecutive_empty >= 3 and active_engine == "ollama":
                    text3 = call_ollama_vision(ollama_url, model, page_path, 0.5)
                    if text3 and len(text3.strip()) > 5:
                        text = text3
                        is_empty = False
            else:
                consecutive_empty = 0

            results.append({
                "page": i,
                "file": os.path.basename(page_path),
                "text": text if not is_empty else "",
            })
            emit_page(i, os.path.basename(page_path), text if not is_empty else "")
        except Exception as e:
            err_str = str(e)
            print(f"[vision] Failed on page {i}: {err_str}", file=sys.stderr)

            # Detect Ollama "llama runner terminated" crashes and auto-fall-back
            # to PaddleOCR for the rest of the chapter. This is the AI Manga
            # Reader-style behavior — when the LLM dies, PaddleOCR keeps going.
            is_ollama_crash = (active_engine == "ollama" and (
                "HTTP 500" in err_str
                or "llama runner" in err_str
                or "process has terminated" in err_str
                or "connection" in err_str.lower()
            ))

            if is_ollama_crash:
                consecutive_crashes += 1
                # Switch immediately on the first crash. A single
                # "llama runner terminated" almost always means the model is
                # broken on this machine — no point waiting for a second one.
                if consecutive_crashes >= 1:
                    emit({
                        "type": "info",
                        "message": "Ollama vision model crashed — falling back to PaddleOCR for remaining pages.",
                    })
                    try:
                        get_paddle_ocr(paddle_lang)
                        active_engine = "paddleocr"
                        # Retry the current page with PaddleOCR
                        r, b = page_with_paddle(i, page_path)
                        results.append(r)
                        boxes_per_page.append({"page": i, "file": r["file"], "boxes": b})
                        emit_page(r["page"], r["file"], r["text"], r.get("error"))
                        continue
                    except Exception as e2:
                        emit({
                            "type": "info",
                            "message": f"PaddleOCR fallback also failed: {e2}",
                        })

            results.append({
                "page": i,
                "file": os.path.basename(page_path),
                "text": "",
                "error": err_str,
            })
            emit_page(i, os.path.basename(page_path), "", err_str)

    output_path = os.path.join(output_dir, "vision_results.json")
    with open(output_path, "w") as f:
        json.dump(results, f, indent=2)

    # If we ran PaddleOCR (originally or via fallback), save the bbox data so
    # the Reader's overlay feature can render translated text on top.
    if boxes_per_page:
        import time as _time
        ocr_results_path = os.path.join(os.path.dirname(output_dir), "pipeline", "ocr_results.json")
        os.makedirs(os.path.dirname(ocr_results_path), exist_ok=True)
        # Match the schema produced by ocr_engine.py: [{page, file, texts, boxes}]
        ocr_payload = []
        for r, bp in zip(results, boxes_per_page):
            ocr_payload.append({
                "page": r["page"],
                "file": r["file"],
                "texts": (r.get("text") or "").split("\n") if r.get("text") else [],
                "boxes": bp["boxes"],
            })
        with open(ocr_results_path, "w") as f:
            json.dump(ocr_payload, f, indent=2)

    pages_with_text = sum(1 for r in results if r.get("text"))
    emit({"type": "info", "message": f"Extracted text from {pages_with_text}/{len(results)} pages"})
    emit({"type": "done", "output_file": "vision_results.json", "pages_with_text": pages_with_text, "total_pages": len(results)})

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 vision_extract.py <job.json>", file=sys.stderr)
        sys.exit(1)

    try:
        with open(sys.argv[1], "r") as f:
            job = json.load(f)
    except Exception as e:
        emit_error(f"Failed to read job file: {e}")
        sys.exit(1)

    try:
        run_extraction(job)
    except Exception as e:
        emit_error(f"Fatal: {e}")
        import traceback
        traceback.print_exc(file=sys.stderr)
        sys.exit(1)
