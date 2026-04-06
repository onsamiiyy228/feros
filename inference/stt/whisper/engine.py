"""Whisper Large v3 Turbo engine using faster-whisper (CTranslate2).

Supports ~100 languages with built-in auto-detection.
Uses BatchedInferencePipeline for ~3x faster transcription.
"""

from __future__ import annotations

import gc
import logging
import threading
import time

import numpy as np

from stt.audio import SAMPLE_RATE
from stt.engine import SttEngine, TranscribeResult

logger = logging.getLogger("stt.whisper")

# Whisper supports ~100 languages.  This is the full list from faster-whisper.
WHISPER_LANGUAGES = [
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo",
    "br", "bs", "ca", "cs", "cy", "da", "de", "el", "en", "es",
    "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw",
    "he", "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja",
    "jw", "ka", "kk", "km", "kn", "ko", "la", "lb", "ln", "lo",
    "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt",
    "my", "ne", "nl", "nn", "no", "oc", "pa", "pl", "ps", "pt",
    "ro", "ru", "sa", "sd", "si", "sk", "sl", "sn", "so", "sq",
    "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl",
    "tr", "tt", "uk", "ur", "uz", "vi", "yi", "yo", "zh", "yue",
]


class WhisperEngine(SttEngine):
    """faster-whisper engine using Whisper Large v3 Turbo.

    - Uses faster-whisper (CTranslate2) for maximum speed
    - Model: large-v3-turbo (4x faster than large-v3, similar quality)
    - Supports auto language detection (language=None)
    - Thread-safe with lock + active request tracking
    - Optional TTL-based auto-unload
    """

    def __init__(
        self,
        model_name: str = "deepdml/faster-whisper-large-v3-turbo-ct2",
        device: str = "cuda",
        compute_type: str = "float16",
        language: str | None = None,
        ttl: int = -1,
    ) -> None:
        self._model_name = model_name
        self._device = device
        self._compute_type = compute_type
        # None means auto-detect; "auto" is treated as None too
        self._language = None if language in (None, "auto") else language
        self._ttl = ttl

        self._model = None
        self._batched_model = None
        self._lock = threading.Lock()
        self._last_used: float = 0.0
        self._unload_timer: threading.Timer | None = None
        self._active_requests: int = 0

    def is_loaded(self) -> bool:
        return self._model is not None

    def load(self) -> None:
        if self._model is not None:
            return

        from faster_whisper import WhisperModel, BatchedInferencePipeline

        logger.info(
            "Loading faster-whisper %s on %s (%s) …",
            self._model_name, self._device, self._compute_type,
        )
        t0 = time.perf_counter()
        self._model = WhisperModel(
            self._model_name,
            device=self._device,
            compute_type=self._compute_type,
        )
        self._batched_model = BatchedInferencePipeline(model=self._model)
        dt = time.perf_counter() - t0
        logger.info("Model loaded in %.1fs", dt)

    def unload(self) -> None:
        with self._lock:
            if self._model is None:
                return
            if self._active_requests > 0:
                logger.debug("Model active, skipping unload")
                self._schedule_unload(self._ttl)
                return

            elapsed = time.monotonic() - self._last_used
            if elapsed < self._ttl and self._ttl > 0:
                self._schedule_unload(self._ttl - elapsed)
                return

            logger.info("Unloading faster-whisper model")
            del self._batched_model
            self._batched_model = None
            del self._model
            self._model = None
            gc.collect()

    def _schedule_unload(self, delay: float | None = None) -> None:
        if self._ttl <= 0:
            return
        if self._unload_timer is not None:
            self._unload_timer.cancel()
        self._unload_timer = threading.Timer(
            delay or self._ttl, self.unload,
        )
        self._unload_timer.daemon = True
        self._unload_timer.start()

    def _touch(self) -> None:
        self._last_used = time.monotonic()
        self._schedule_unload()

    def warmup(self) -> None:
        """Run warmup transcriptions to pre-compile CUDA kernels.

        CTranslate2 compiles different CUDA kernels lazily depending on
        input size.  We run multiple passes with varying audio lengths
        so the first real request doesn't pay the compilation cost.
        """
        self.load()

        logger.info("Warming up faster-whisper (3 passes) …")
        for dur_s in (1, 5, 15):
            # White noise gives Whisper something non-trivial to decode
            audio = np.random.randn(dur_s * SAMPLE_RATE).astype(np.float32) * 0.02
            t0 = time.perf_counter()
            self.transcribe(audio)
            logger.info(
                "  warmup %ds pass done in %.0fms",
                dur_s, (time.perf_counter() - t0) * 1000,
            )
        logger.info("Warmup complete — CUDA kernels compiled.")

    def transcribe(self, audio: np.ndarray) -> TranscribeResult:
        """Transcribe float32 audio with optional language detection.

        When self._language is None, faster-whisper auto-detects the
        language and reports it in info.language.
        """
        with self._lock:
            self.load()
            self._touch()
            self._active_requests += 1
            batched = self._batched_model

        try:
            assert batched is not None
            t0 = time.perf_counter()
            segments, info = batched.transcribe(
                audio,
                language=self._language,
                batch_size=16,
                without_timestamps=True,
                vad_filter=True,
                vad_parameters=dict(
                    min_silence_duration_ms=300,
                    speech_pad_ms=200,
                ),
            )
            text = "".join(seg.text for seg in segments).strip()
            detected_lang = info.language or self._language or "unknown"
            dt = time.perf_counter() - t0
            logger.info(
                "Transcribed %.1fs audio in %.0fms (lang=%s, prob=%.2f) → '%s'",
                len(audio) / SAMPLE_RATE,
                dt * 1000,
                detected_lang,
                info.language_probability,
                text[:80] + ("…" if len(text) > 80 else ""),
            )
            return TranscribeResult(text=text, language=detected_lang)
        finally:
            with self._lock:
                self._active_requests -= 1
                self._touch()

    def supported_languages(self) -> list[str]:
        return WHISPER_LANGUAGES
