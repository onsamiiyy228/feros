"""Abstract base class for STT engines."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass

import numpy as np


@dataclass
class TranscribeResult:
    """Result of a transcription.

    Attributes:
        text: The transcribed text.
        language: ISO 639-1 detected language code (e.g. "en", "zh").
            For engines that don't detect language, this is the configured
            language code.
    """

    text: str
    language: str


class SttEngine(ABC):
    """Base class for STT engines.

    All engines share the same lifecycle:
        load() → warmup() → transcribe() (repeated) → unload()
    """

    @abstractmethod
    def load(self) -> None:
        """Load the model into GPU memory."""
        ...

    @abstractmethod
    def is_loaded(self) -> bool:
        """Return True if the model is loaded and ready."""
        ...

    @abstractmethod
    def unload(self) -> None:
        """Release GPU memory."""
        ...

    @abstractmethod
    def warmup(self) -> None:
        """Run warmup passes to compile GPU kernels."""
        ...

    @abstractmethod
    def transcribe(self, audio: np.ndarray) -> TranscribeResult:
        """Transcribe audio and return text + detected language.

        Args:
            audio: Float32 audio samples at 16 kHz mono.

        Returns:
            TranscribeResult with text and detected language code.
        """
        ...

    @abstractmethod
    def supported_languages(self) -> list[str]:
        """Return list of ISO 639-1 language codes this engine supports."""
        ...
