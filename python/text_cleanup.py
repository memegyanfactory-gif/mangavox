"""
Text cleanup for OCR output.
Merges fragmented text, removes SFX, orders by reading position.
"""
import re

def cleanup_texts(ocr_results):
    """
    Takes OCR results (list of page dicts with texts/boxes),
    returns cleaned segments: [{page: int, text: str}]
    """
    segments = []

    for page_data in ocr_results:
        page_num = page_data["page"]
        texts = page_data.get("texts", [])

        if not texts:
            continue

        # Filter out likely SFX (very short, all caps, or common SFX patterns)
        cleaned = []
        for text in texts:
            text = text.strip()
            if not text:
                continue
            # Skip very short fragments that are likely SFX
            if len(text) <= 2 and text.upper() == text:
                continue
            # Skip common manga SFX
            if re.match(r'^[A-Z!?*]+$', text) and len(text) <= 5:
                continue
            # Skip ellipsis-only
            if re.match(r'^[.\u2026]+$', text):
                continue
            cleaned.append(text)

        if cleaned:
            # Join the text fragments into a coherent paragraph
            combined = ' '.join(cleaned)
            # Clean up spacing
            combined = re.sub(r'\s+', ' ', combined).strip()
            # Fix common OCR artifacts
            combined = combined.replace(' .', '.').replace(' ,', ',')
            combined = combined.replace(' !', '!').replace(' ?', '?')

            if combined and len(combined) > 3:
                segments.append({
                    "page": page_num,
                    "text": combined,
                })

    return segments
