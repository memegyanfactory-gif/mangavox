"""
OCR Engine for MangaVox
Priority: Gemini API (free, best quality) → Ollama vision LLM → macOS Vision → PaddleOCR → stub.
"""
import os
import sys
import json
import subprocess
import time

def run_ocr(page_files, output_dir, progress_cb=None, engine_pref=None, ollama_config=None, gemini_api_key=None):
    """
    Run OCR on a list of page image files.
    Returns a list of dicts: [{page: int, file: str, texts: [str], boxes: [...]}]
    """
    # Priority: Gemini (free API, best quality) → Ollama → local OCR
    if gemini_api_key:
        ocr_engine = _get_gemini_engine(gemini_api_key)
    elif ollama_config and ollama_config.get("url") and ollama_config.get("model"):
        ocr_engine = _get_ollama_engine(ollama_config)
    else:
        ocr_engine = _get_ocr_engine(engine_pref)
    results = []

    for i, page_path in enumerate(page_files):
        if progress_cb:
            progress_cb("ocr", i + 1, len(page_files))

        try:
            texts, boxes = ocr_engine(page_path)
            results.append({
                "page": i,
                "file": os.path.basename(page_path),
                "texts": texts,
                "boxes": boxes,
            })
        except Exception as e:
            print(f"[ocr] Failed on page {i}: {e}", file=sys.stderr)
            results.append({
                "page": i,
                "file": os.path.basename(page_path),
                "texts": [],
                "boxes": [],
            })

    # Save OCR results
    ocr_path = os.path.join(output_dir, "ocr_results.json")
    with open(ocr_path, 'w') as f:
        json.dump(results, f, indent=2)

    return results

def _get_ocr_engine(preference=None):
    """Try preferred engine, then macOS Vision, PaddleOCR, manga-ocr, stub."""

    # ── macOS Vision Framework (zero-dependency, built into macOS) ──
    vision_binary = os.path.join(os.path.dirname(os.path.abspath(__file__)), "vision_ocr")
    if os.path.exists(vision_binary) and sys.platform == "darwin":
        _vision_logged = [False]
        def vision_ocr(image_path):
            if not _vision_logged[0]:
                print("[ocr] Using macOS Vision framework (built-in)", file=sys.stderr)
                _vision_logged[0] = True
            try:
                result = subprocess.run(
                    [vision_binary, image_path],
                    capture_output=True, text=True, timeout=30,
                )
                if result.returncode != 0:
                    print(f"[ocr] Vision error: {result.stderr[:200]}", file=sys.stderr)
                    return [], []

                items = json.loads(result.stdout.strip() or "[]")
                texts = [item["text"] for item in items if item.get("text")]
                boxes = [{
                    "points": [
                        [item["x"], item["y"]],
                        [item["x"] + item["w"], item["y"]],
                        [item["x"] + item["w"], item["y"] + item["h"]],
                        [item["x"], item["y"] + item["h"]],
                    ],
                    "confidence": item.get("confidence", 0.9),
                } for item in items if item.get("text")]
                return texts, boxes
            except subprocess.TimeoutExpired:
                print(f"[ocr] Vision OCR timed out for {image_path}", file=sys.stderr)
                return [], []
            except Exception as e:
                print(f"[ocr] Vision OCR error: {e}", file=sys.stderr)
                return [], []

        return vision_ocr

    # ── PaddleOCR ──
    try:
        from paddleocr import PaddleOCR
        ocr = PaddleOCR(use_angle_cls=True, lang='en', show_log=False)

        def paddle_ocr(image_path):
            result = ocr.ocr(image_path, cls=True)
            texts = []
            boxes = []
            if result and result[0]:
                for line in result[0]:
                    box, (text, conf) = line[0], line[1]
                    if conf > 0.5:
                        texts.append(text)
                        boxes.append({
                            "points": [[int(p[0]), int(p[1])] for p in box],
                            "confidence": round(conf, 3),
                        })
            return texts, boxes

        print("[ocr] Using PaddleOCR engine", file=sys.stderr)
        return paddle_ocr
    except ImportError:
        pass

    # ── manga-ocr (Japanese-focused) ──
    try:
        from manga_ocr import MangaOcr
        mocr = MangaOcr()

        def manga_ocr_fn(image_path):
            from PIL import Image
            img = Image.open(image_path)
            text = mocr(img)
            return [text] if text.strip() else [], []

        print("[ocr] Using manga-ocr engine", file=sys.stderr)
        return manga_ocr_fn
    except ImportError:
        pass

    # ── Stub: no OCR available ──
    print("[ocr] WARNING: No OCR engine available. On macOS, the Vision binary should be compiled.", file=sys.stderr)
    print("[ocr] Run: cd python && swiftc -O -o vision_ocr vision_ocr.swift -framework Vision -framework AppKit", file=sys.stderr)

    def stub_ocr(image_path):
        return [], []

    return stub_ocr


# Vision-capable model keywords for auto-detection
VISION_MODEL_KEYWORDS = ['llava', 'vision', 'bakllava', 'moondream', 'cogvlm', 'minicpm-v', 'gemma3']

# Preferred order when auto-selecting a vision model
VISION_MODEL_PREFERENCE = ['minicpm-v', 'gemma3', 'llava-llama3', 'llava:13b', 'llava:7b', 'llava', 'moondream', 'vision']


def _list_ollama_models(ollama_url):
    """Query Ollama for installed models. Returns list of model name strings."""
    import urllib.request
    import urllib.error
    try:
        url = ollama_url.rstrip("/") + "/api/tags"
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            return [m["name"] for m in data.get("models", [])]
    except Exception as e:
        print(f"[ocr] Cannot list Ollama models: {e}", file=sys.stderr)
        return []


def _find_best_vision_model(installed_models):
    """Pick the best installed vision model based on preference order."""
    vision_models = [m for m in installed_models
                     if any(kw in m.lower() for kw in VISION_MODEL_KEYWORDS)]
    if not vision_models:
        return None
    # Sort by preference order
    def rank(name):
        lower = name.lower()
        for i, pref in enumerate(VISION_MODEL_PREFERENCE):
            if pref in lower:
                return i
        return len(VISION_MODEL_PREFERENCE)
    vision_models.sort(key=rank)
    return vision_models[0]


def _get_ollama_engine(ollama_config):
    """Use a local Ollama vision LLM for OCR. Auto-detects model if configured one isn't installed."""
    import base64
    import urllib.request
    import urllib.error
    import io

    ollama_url = ollama_config["url"].rstrip("/")
    requested_model = ollama_config["model"]
    temperature = ollama_config.get("temperature", 0.3)

    # Check if the requested model is actually installed
    installed = _list_ollama_models(ollama_url)
    if requested_model in installed:
        model = requested_model
    else:
        # Try to find any installed vision model
        fallback = _find_best_vision_model(installed)
        if fallback:
            print(f"[ocr] Model '{requested_model}' not installed. Using '{fallback}' instead.", file=sys.stderr)
            model = fallback
        else:
            print(f"[ocr] No vision model installed! Available: {installed}", file=sys.stderr)
            print(f"[ocr] Install one with: ollama pull llava:7b", file=sys.stderr)
            # Fall back to non-Ollama OCR engine
            print(f"[ocr] Falling back to non-Ollama OCR engine", file=sys.stderr)
            return _get_ocr_engine(None)

    url = ollama_url + "/api/chat"

    SYSTEM_PROMPT = (
        "You are a precise manga page reader. Extract ONLY story text in correct reading order.\n\n"
        "READ: Dialogue bubbles (round/oval with tail → speaker), narration boxes (rectangular, no tail), "
        "on-screen text (phones/signs/monitors), emphasis/shouting (large/bold/spiky bubbles).\n"
        "IGNORE: Publisher credits, author names, disclaimers, chapter titles, page numbers, ads, "
        "decorative SFX katakana (unless critical like explosions).\n\n"
        "READING ORDER: Panels top-right first, move left across row, then next row from right. "
        "Inside panel: rightmost bubble first, move left. Top narration before dialogue, bottom after.\n\n"
        "OUTPUT FORMAT per panel:\n"
        "PANEL [n]\n"
        "[DIALOGUE] Speaker: \"text\" (tone)\n"
        "[NARRATION] N/A: text (tone)\n"
        "[ON-SCREEN] N/A: text\n"
        "[SFX] N/A: sound (only if critical)\n\n"
        "RULES: Write in English. Translate non-English. Unknown speaker if no tail. "
        "No art descriptions. No commentary. Just structured story text."
    )

    print(f"[ocr] Using Ollama vision model: {model}", file=sys.stderr)

    def _prepare_image(image_path):
        """Upscale small images for better OCR."""
        try:
            from PIL import Image
            img = Image.open(image_path)
            w, h = img.size
            if img.mode not in ('RGB', 'L'):
                img = img.convert('RGB')
            if max(w, h) < 800:
                scale = 1200 / max(w, h)
                img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)
            if max(w, h) > 1536:
                scale = 1536 / max(w, h)
                img = img.resize((int(w * scale), int(h * scale)), Image.LANCZOS)
            buf = io.BytesIO()
            img.save(buf, format="PNG", quality=95)
            return base64.b64encode(buf.getvalue()).decode("utf-8")
        except ImportError:
            with open(image_path, "rb") as f:
                return base64.b64encode(f.read()).decode("utf-8")

    def ollama_ocr(image_path):
        try:
            image_b64 = _prepare_image(image_path)

            payload = json.dumps({
                "model": model,
                "stream": False,
                "options": {"temperature": temperature, "num_predict": 4096, "top_p": 0.9},
                "messages": [
                    {"role": "system", "content": SYSTEM_PROMPT},
                    {
                        "role": "user",
                        "content": "Read this manga page. Extract all story text in correct right-to-left panel order using the structured format.",
                        "images": [image_b64],
                    },
                ],
            }).encode("utf-8")

            req = urllib.request.Request(
                url, data=payload,
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=180) as resp:
                result = json.loads(resp.read().decode("utf-8"))
                text = result.get("message", {}).get("content", "").strip()

                # Remove common model preamble artifacts
                preambles = [
                    "Here is the text", "Here's the transcribed", "The text on this page",
                    "Text from this", "Here's the text", "The text reads",
                    "I can see the following", "Reading the page",
                ]
                lower = text.lower()
                for prefix in preambles:
                    if lower.startswith(prefix.lower()):
                        colon_idx = text.find(":", 0, len(prefix) + 10)
                        if colon_idx >= 0:
                            text = text[colon_idx + 1:].strip()
                        break

                # Remove trailing meta-commentary
                for marker in ["\n\nNote:", "\n\n(Note:", "\n\nThis page", "\n\nThe page"]:
                    idx = text.find(marker)
                    if idx > 0:
                        text = text[:idx].strip()

                if text and text.strip() != "[NO TEXT]" and len(text.strip()) > 2:
                    return [text], []
                return [], []
        except Exception as e:
            print(f"[ocr] Ollama vision error: {e}", file=sys.stderr)
            return [], []

    return ollama_ocr


# ── Google Gemini API (free tier: 15 RPM, 1M tokens/day) ──

GEMINI_NARRATION_PROMPT = (
    "You are a precise manga page reader. Extract ONLY story text in correct reading order.\n\n"
    "READ: Dialogue bubbles (round/oval with tail → speaker), narration boxes (rectangular, no tail), "
    "on-screen text (phones/signs/monitors), emphasis/shouting (large/bold/spiky bubbles).\n"
    "IGNORE: Publisher credits, author names, disclaimers, chapter titles, page numbers, ads, "
    "decorative SFX katakana (unless critical like explosions).\n\n"
    "READING ORDER: Panels top-right first, move left across row, then next row from right. "
    "Inside panel: rightmost bubble first, move left. Top narration before dialogue, bottom after.\n\n"
    "OUTPUT FORMAT per panel:\n"
    "PANEL [n]\n"
    "[DIALOGUE] Speaker: \"text\" (tone)\n"
    "[NARRATION] N/A: text (tone)\n"
    "[ON-SCREEN] N/A: text\n"
    "[SFX] N/A: sound (only if critical)\n\n"
    "RULES: Write in English. Translate non-English. Unknown speaker if no tail. "
    "No art descriptions. No commentary. Just structured story text.\n"
    "If purely visual with no story text: [VISUAL ONLY] Brief scene description."
)


def _get_gemini_engine(api_key, model=None):
    """Use Google Gemini API (free tier) for excellent manga narration."""
    import base64
    import urllib.request
    import urllib.error

    model_name = model if model else 'gemini-2.0-flash'
    print(f"[ocr] Using Google Gemini API (free tier) — model: {model_name}", file=sys.stderr)

    def gemini_ocr(image_path):
        try:
            # Read and base64 encode the image
            with open(image_path, "rb") as f:
                image_data = base64.b64encode(f.read()).decode("utf-8")

            # Detect mime type
            ext = os.path.splitext(image_path)[1].lower()
            mime_map = {'.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.png': 'image/png',
                        '.webp': 'image/webp', '.gif': 'image/gif'}
            mime_type = mime_map.get(ext, 'image/jpeg')

            payload = json.dumps({
                "contents": [{
                    "parts": [
                        {"text": "Read this manga page. Extract all story text in correct right-to-left panel order using the structured format."},
                        {"inline_data": {"mime_type": mime_type, "data": image_data}},
                    ]
                }],
                "systemInstruction": {"parts": [{"text": GEMINI_NARRATION_PROMPT}]},
                "generationConfig": {
                    "temperature": 0.4,
                    "maxOutputTokens": 4096,
                    "topP": 0.9,
                }
            }).encode("utf-8")

            # Use the model from config, defaulting to gemini-2.0-flash if not set
            model_name = model if model else 'gemini-2.0-flash'
            url = f"https://generativelanguage.googleapis.com/v1beta/models/{model_name}:generateContent?key={api_key}"
            req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})

            with urllib.request.urlopen(req, timeout=60) as resp:
                result = json.loads(resp.read().decode("utf-8"))
                # Extract text from Gemini response
                candidates = result.get("candidates", [])
                if not candidates:
                    return [], []
                parts = candidates[0].get("content", {}).get("parts", [])
                text = ""
                for part in parts:
                    if "text" in part:
                        text += part["text"]
                text = text.strip()

                if text and len(text) > 5:
                    return [text], []
                return [], []

        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")[:300]
            print(f"[ocr] Gemini API error {e.code}: {body}", file=sys.stderr)
            # Rate limit — wait and retry once
            if e.code == 429:
                print("[ocr] Rate limited, waiting 10s...", file=sys.stderr)
                time.sleep(10)
                try:
                    with urllib.request.urlopen(req, timeout=60) as resp:
                        result = json.loads(resp.read().decode("utf-8"))
                        parts = result.get("candidates", [{}])[0].get("content", {}).get("parts", [])
                        text = "".join(p.get("text", "") for p in parts).strip()
                        if text and len(text) > 5:
                            return [text], []
                except Exception:
                    pass
            return [], []
        except Exception as e:
            print(f"[ocr] Gemini error: {e}", file=sys.stderr)
            return [], []

    return gemini_ocr
