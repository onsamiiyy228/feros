"""Tests for TTS text normalizer — language detection and normalization."""

from __future__ import annotations

import pytest

from tts.normalizer import _detect_lang, _SUPPORTED_LANGS


# ── Language detection tests ─────────────────────────────────────


class TestDetectLang:
    """Verify _detect_lang returns codes that match _SUPPORTED_LANGS."""

    def test_english(self) -> None:
        assert _detect_lang("Our plan is twenty four dollars per month") == "en"

    def test_arabic(self) -> None:
        assert _detect_lang("مرحبا بالعالم، كيف حالك اليوم؟") == "ar"

    def test_german(self) -> None:
        assert _detect_lang("Guten Tag, wie geht es Ihnen heute?") == "de"

    def test_spanish(self) -> None:
        assert _detect_lang("Hola, ¿cómo estás hoy? Me llamo Carlos.") == "es"

    def test_french(self) -> None:
        assert _detect_lang("Bonjour, comment allez-vous aujourd'hui?") == "fr"

    def test_hindi(self) -> None:
        assert _detect_lang("नमस्ते, आज आप कैसे हैं?") == "hi"

    def test_hungarian(self) -> None:
        assert _detect_lang("Jó napot kívánok, hogy van ma?") == "hu"

    def test_italian(self) -> None:
        assert _detect_lang("Buongiorno, come stai oggi?") == "it"

    def test_japanese(self) -> None:
        assert _detect_lang("こんにちは、今日はお元気ですか？") == "ja"

    def test_russian(self) -> None:
        assert _detect_lang("Здравствуйте, как у вас дела сегодня?") == "ru"

    def test_swedish(self) -> None:
        assert _detect_lang("Välkommen till vår tjänst, hur kan jag hjälpa dig idag?") == "sv"

    def test_vietnamese(self) -> None:
        assert _detect_lang("Xin chào, hôm nay bạn có khỏe không?") == "vi"

    def test_chinese_maps_to_zh(self) -> None:
        """langdetect returns 'zh-cn'/'zh-tw' — we strip to 'zh'."""
        result = _detect_lang("你好，今天你怎么样？欢迎来到我们的服务。")
        assert result == "zh"

    def test_detected_lang_in_supported_set(self) -> None:
        """Common languages should map to codes in _SUPPORTED_LANGS."""
        test_cases = {
            "en": "Hello, how are you today? I'm doing great.",
            "de": "Guten Tag, wie geht es Ihnen heute?",
            "es": "Hola, ¿cómo estás hoy? Me llamo Carlos.",
            "fr": "Bonjour, comment allez-vous aujourd'hui?",
            "it": "Buongiorno, come stai oggi? Tutto bene.",
            "ja": "こんにちは、今日はお元気ですか？",
            "ru": "Здравствуйте, как у вас дела сегодня?",
            "zh": "你好，今天你怎么样？欢迎来到我们的服务。",
        }
        for expected_lang, text in test_cases.items():
            detected = _detect_lang(text)
            assert detected in _SUPPORTED_LANGS, (
                f"Expected {expected_lang!r} in _SUPPORTED_LANGS, "
                f"got {detected!r} for text: {text[:30]!r}"
            )

    def test_unsupported_lang_not_in_set(self) -> None:
        """Korean is not in _SUPPORTED_LANGS — should be detected but skipped."""
        result = _detect_lang("안녕하세요, 오늘 어떻게 지내세요?")
        assert result == "ko"
        assert result not in _SUPPORTED_LANGS

    def test_empty_string_fallback(self) -> None:
        """Empty or whitespace input should fall back to 'en'."""
        assert _detect_lang("") == "en"
        assert _detect_lang("   ") == "en"

    def test_short_text_fallback(self) -> None:
        """Very short text may fail detection — should fall back to 'en'."""
        result = _detect_lang("Hi")
        # langdetect may or may not detect "Hi" — just ensure no crash
        assert isinstance(result, str)
        assert len(result) >= 2


# ── Supported langs sanity ───────────────────────────────────────


class TestSupportedLangs:
    """Verify the _SUPPORTED_LANGS set is correct."""

    def test_expected_langs(self) -> None:
        expected = {
            "ar", "de", "en", "es", "fr", "hi", "hu", "hy",
            "it", "ja", "ru", "rw", "sv", "vi", "zh",
        }
        assert _SUPPORTED_LANGS == expected

    def test_count(self) -> None:
        assert len(_SUPPORTED_LANGS) == 15
