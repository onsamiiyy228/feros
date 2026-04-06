"""Abstract TTS engine base class.

All TTS engines (Fish Speech, MOSS-TTS, etc.) implement this interface.
The shared server in server.py calls these methods to synthesize audio.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass


@dataclass
class SynthResult:
    """Result from a TTS synthesis call."""

    audio: bytes  # WAV bytes (any sample rate)
    sample_rate: int
    duration: float  # seconds


class TtsEngine(ABC):
    """Abstract base for TTS engines."""

    @abstractmethod
    def is_loaded(self) -> bool:
        ...

    @abstractmethod
    def load(self) -> None:
        ...

    @abstractmethod
    def unload(self) -> None:
        ...

    def warmup(self) -> None:
        """Optional warmup — default just loads the model."""
        self.load()

    @abstractmethod
    def synthesize(
        self,
        text: str,
        voice: str | None = None,
        seed: int | None = None,
    ) -> SynthResult:
        """Synthesize text to WAV audio.

        Args:
            text: Text to synthesize.
            voice: Voice profile name (engine-specific).
            seed: RNG seed for consistent speaker identity.

        Returns:
            SynthResult with WAV bytes, sample rate, and duration.
        """
        ...

    @abstractmethod
    def supported_voices(self) -> list[str]:
        """Return list of available voice profile names."""
        ...
