"""Text normalizer for TTS — converts written form to spoken form.

Uses NeMo's WFST-based text normalization to handle currencies, dates,
percentages, and other symbolic text that TTS engines struggle with.
Language is auto-detected per request.

Example:
    "$24/month" → "twenty four dollars per month"
    "15%"       → "fifteen percent"

The normalizer is lazy-loaded per language on first use (the WFST grammar
compilation takes a few seconds) and cached for subsequent calls.
"""

from __future__ import annotations

import logging
from typing import Any

logger = logging.getLogger("tts.normalizer")

# Languages supported by NeMo text normalization
_SUPPORTED_LANGS = {
    "ar", "de", "en", "es", "fr", "hi", "hu", "hy",
    "it", "ja", "ru", "rw", "sv", "vi", "zh",
}

_normalizer_cache: dict[str, Any] = {}


def _get_normalizer(lang: str) -> Any:
    """Get or create a cached NeMo Normalizer for the given language."""
    if lang in _normalizer_cache:
        return _normalizer_cache[lang]

    try:
        from nemo_text_processing.text_normalization.normalize import (
            Normalizer,
        )
    except ImportError:
        logger.warning(
            "nemo_text_processing not installed — text normalization disabled. "
            "Install with: pip install nemo_text_processing"
        )
        _normalizer_cache[lang] = None
        return None

    logger.info("Initializing NeMo text normalizer for lang=%s …", lang)
    try:
        normalizer = Normalizer(input_case="cased", lang=lang)
        _normalizer_cache[lang] = normalizer
        logger.info("NeMo text normalizer ready for lang=%s", lang)
        return normalizer
    except Exception:
        logger.exception(
            "Failed to initialize NeMo normalizer for lang=%s", lang
        )
        _normalizer_cache[lang] = None
        return None


def _detect_lang(text: str) -> str:
    """Detect the language of the input text.

    Returns a 2-letter language code (e.g. "en", "zh", "de").
    Falls back to "en" if detection fails or the library is unavailable.
    """
    try:
        from langdetect import detect

        lang = detect(text)
        # langdetect returns codes like "en", "zh-cn", "zh-tw", etc.
        return lang.split("-")[0]
    except Exception:
        return "en"


def normalize_text(text: str) -> str:
    """Normalize text for TTS synthesis.

    Auto-detects language and converts written forms (currencies, dates,
    percentages, etc.) into their spoken equivalents using NeMo's
    WFST-based normalizer.

    Falls back to the original text if normalization is unavailable,
    the language is unsupported, or normalization fails.

    Args:
        text: Input text to normalize.

    Returns:
        Normalized text in spoken form.
    """
    if not text or not text.strip():
        return text

    lang = _detect_lang(text)

    if lang not in _SUPPORTED_LANGS:
        logger.debug("Language %r not supported by normalizer, skipping", lang)
        return text

    normalizer = _get_normalizer(lang)
    if normalizer is None:
        return text

    try:
        result = normalizer.normalize(text, verbose=False)
        if result != text:
            logger.debug("Normalized [%s]: %r → %r", lang, text[:80], result[:80])
        return result
    except Exception:
        logger.warning("Normalization failed for: %r", text[:80], exc_info=True)
        return text
