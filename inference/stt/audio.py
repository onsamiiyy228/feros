"""Shared audio helpers for STT engines."""

from __future__ import annotations

import numpy as np

SAMPLE_RATE = 16000
BYTES_PER_SAMPLE = 2  # PCM-16


def pcm16_to_float32(b: bytes) -> np.ndarray:
    """Convert PCM-16 LE bytes to float32 in [-1, 1]."""
    return np.frombuffer(b, dtype=np.int16).astype(np.float32) / 32768.0


def make_divisible_by(num: int, factor: int) -> int:
    """Make num divisible by factor (round down)."""
    return (num // factor) * factor
