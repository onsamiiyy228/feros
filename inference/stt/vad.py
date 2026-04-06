"""Silero VAD-based silence filter for STT pre-processing.

Shared between Parakeet and Whisper engines — strips silence
from audio before transcription to prevent hallucinations on
accumulated ambient noise.
"""

from __future__ import annotations

import logging

import numpy as np
import torch

from stt.audio import SAMPLE_RATE

logger = logging.getLogger("stt.vad")


class SileroVadFilter:
    """Silero VAD-based silence filter for STT pre-processing.

    Strips silence from audio before transcription to prevent
    hallucinations on accumulated ambient noise.
    """

    def __init__(
        self,
        threshold: float = 0.5,
        min_speech_duration_ms: int = 250,
        min_silence_duration_ms: int = 300,
        speech_pad_ms: int = 200,
        window_size_samples: int = 512,
        sample_rate: int = SAMPLE_RATE,
    ) -> None:
        self._threshold = threshold
        self._min_speech_duration_ms = min_speech_duration_ms
        self._min_silence_duration_ms = min_silence_duration_ms
        self._speech_pad_ms = speech_pad_ms
        self._window_size = window_size_samples
        self._sample_rate = sample_rate
        self._model = None

    def _load(self) -> None:
        if self._model is not None:
            return

        logger.info("Loading Silero VAD model …")
        import time

        t0 = time.perf_counter()
        model, _ = torch.hub.load(
            repo_or_dir="snakers4/silero-vad",
            model="silero_vad",
            trust_repo=True,
        )
        self._model = model
        logger.info("Silero VAD loaded in %.0fms", (time.perf_counter() - t0) * 1000)

    def preload(self) -> None:
        """Eagerly load the VAD model (call during startup)."""
        self._load()

    def collect_speech(self, audio: np.ndarray) -> np.ndarray:
        """Extract only speech segments, concatenated.

        Returns empty array if no speech detected; original audio if
        too short for VAD processing.
        """
        if len(audio) < self._window_size * 2:
            return audio

        self._load()
        self._model.reset_states()

        audio_tensor = torch.from_numpy(audio).float()
        num_samples = len(audio_tensor)
        min_speech_samples = self._min_speech_duration_ms * self._sample_rate // 1000
        min_silence_samples = self._min_silence_duration_ms * self._sample_rate // 1000
        speech_pad_samples = self._speech_pad_ms * self._sample_rate // 1000

        segments: list[dict] = []
        current_speech: dict | None = None
        silence_counter = 0

        for i in range(0, num_samples, self._window_size):
            end = min(i + self._window_size, num_samples)
            chunk = audio_tensor[i:end]
            if len(chunk) < self._window_size:
                chunk = torch.nn.functional.pad(chunk, (0, self._window_size - len(chunk)))

            prob = self._model(chunk, self._sample_rate).item()

            if prob >= self._threshold:
                if current_speech is None:
                    current_speech = {"start": i}
                silence_counter = 0
            else:
                if current_speech is not None:
                    silence_counter += self._window_size
                    if silence_counter >= min_silence_samples:
                        speech_end = i - silence_counter + self._window_size
                        if speech_end - current_speech["start"] >= min_speech_samples:
                            current_speech["end"] = speech_end
                            segments.append(current_speech)
                        current_speech = None
                        silence_counter = 0

        if current_speech is not None:
            if num_samples - current_speech["start"] >= min_speech_samples:
                current_speech["end"] = num_samples
                segments.append(current_speech)

        if not segments:
            logger.debug("VAD found no speech in %.1fs audio", num_samples / self._sample_rate)
            return np.array([], dtype=np.float32)

        # Pad and merge
        for seg in segments:
            seg["start"] = max(0, seg["start"] - speech_pad_samples)
            seg["end"] = min(num_samples, seg["end"] + speech_pad_samples)

        merged = [segments[0]]
        for seg in segments[1:]:
            if seg["start"] <= merged[-1]["end"]:
                merged[-1]["end"] = max(merged[-1]["end"], seg["end"])
            else:
                merged.append(seg)

        result = np.concatenate([audio[s["start"]:s["end"]] for s in merged])

        trimmed_s = (num_samples - len(result)) / self._sample_rate
        if trimmed_s > 0.1:
            logger.debug(
                "VAD: %.1fs → %.1fs (trimmed %.1fs silence, %d segments)",
                num_samples / self._sample_rate,
                len(result) / self._sample_rate,
                trimmed_s,
                len(merged),
            )

        return result
