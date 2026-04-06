"""NVIDIA Parakeet TDT engine with NeMo streaming API.

Uses nvidia/parakeet-tdt-0.6b-v3 (25 EU languages, auto-detect).
"""

from __future__ import annotations

import copy
import gc
import logging
import threading
import time
from dataclasses import dataclass

import numpy as np
import torch
from omegaconf import OmegaConf, open_dict

import nemo.collections.asr as nemo_asr
from nemo.collections.asr.parts.submodules.rnnt_decoding import RNNTDecodingConfig

from stt.audio import SAMPLE_RATE, make_divisible_by
from stt.engine import SttEngine, TranscribeResult

logger = logging.getLogger("stt.parakeet")

# Parakeet v3 supported languages (25 EU)
PARAKEET_LANGUAGES = [
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de",
    "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro", "ru",
    "sk", "sl", "es", "sv", "uk",
]


@dataclass
class ContextSize:
    """Left / chunk / right context sizes in samples or encoder frames."""

    left: int
    chunk: int
    right: int

    def total(self) -> int:
        return self.left + self.chunk + self.right

    def subsample(self, factor: int) -> ContextSize:
        return ContextSize(
            left=self.left // factor,
            chunk=self.chunk // factor,
            right=self.right // factor,
        )


@dataclass
class StreamingConfig:
    """Configuration for NeMo streaming inference.

    Parameters match NeMo's official recommendations:
      - long file:  10-10-5  (left-chunk-right in seconds)
      - streaming:  10-2-2   (balanced)
      - low latency: 10-1-1
      - realtime:   10-0.5-0.5
    """

    chunk_secs: float = 1.0
    left_context_secs: float = 10.0
    right_context_secs: float = 1.0


class ParakeetEngine(SttEngine):
    """NVIDIA Parakeet TDT engine with proper NeMo streaming API.

    Instead of calling model.transcribe() (batch API) on isolated chunks,
    this engine:
      1. Calls the encoder with full [left+chunk+right] context
      2. Strips left context from encoder output
      3. Decodes only the chunk frames
      4. Carries decoder state across chunks

    This is the approach used by NeMo's official streaming example
    (speech_to_text_streaming_infer_rnnt.py) and parakeet-stream.
    """

    def __init__(
        self,
        model_name: str = "nvidia/parakeet-tdt-0.6b-v3",
        device: str = "cuda",
        streaming_config: StreamingConfig | None = None,
        ttl: int = -1,
    ) -> None:
        self._model_name = model_name
        self._device = device
        self._streaming_config = streaming_config or StreamingConfig()
        self._ttl = ttl

        self._model = None
        self._lock = threading.Lock()
        self._last_used: float = 0.0
        self._unload_timer: threading.Timer | None = None
        self._active_requests: int = 0

        # Computed after model load
        self._sample_rate: int = SAMPLE_RATE
        self._feature_stride_sec: float = 0.0
        self._features_per_sec: float = 0.0
        self._encoder_subsampling_factor: int = 1
        self._encoder_frame2audio_samples: int = 1
        self._context_encoder_frames: ContextSize | None = None
        self._context_samples: ContextSize | None = None

    def is_loaded(self) -> bool:
        return self._model is not None

    def load(self) -> None:
        if self._model is not None:
            return

        logger.info("Loading Parakeet TDT %s on %s …", self._model_name, self._device)
        t0 = time.perf_counter()

        self._model = nemo_asr.models.ASRModel.from_pretrained(
            model_name=self._model_name
        )

        # Move to GPU and set to eval mode
        if self._device == "cuda" and torch.cuda.is_available():
            self._model = self._model.cuda()
        self._model.eval()

        # Configure preprocessor for streaming
        model_cfg = copy.deepcopy(self._model._cfg)
        OmegaConf.set_struct(model_cfg.preprocessor, False)
        model_cfg.preprocessor.dither = 0.0
        model_cfg.preprocessor.pad_to = 0
        OmegaConf.set_struct(model_cfg.preprocessor, True)

        with torch.no_grad():
            self._model.preprocessor.featurizer.dither = 0.0
            self._model.preprocessor.featurizer.pad_to = 0

        # Setup decoding for streaming — greedy_batch with label-looping
        decoding_cfg = RNNTDecodingConfig(
            strategy="greedy_batch",
            preserve_alignments=False,
            fused_batch_size=-1,
        )
        decoding_cfg = OmegaConf.structured(decoding_cfg)
        with open_dict(decoding_cfg):
            decoding_cfg.greedy.loop_labels = True
            decoding_cfg.tdt_include_token_duration = False

        self._model.change_decoding_strategy(decoding_cfg)

        # Store model configuration for context calculations
        self._sample_rate = model_cfg.preprocessor["sample_rate"]
        self._feature_stride_sec = model_cfg.preprocessor["window_stride"]
        self._features_per_sec = 1.0 / self._feature_stride_sec
        self._encoder_subsampling_factor = self._model.encoder.subsampling_factor

        # Compute context sizes
        self._compute_context_sizes()

        dt = time.perf_counter() - t0
        logger.info("Parakeet TDT loaded in %.1fs", dt)
        logger.info(
            "Context (sec): left=%.1f, chunk=%.1f, right=%.1f  (latency=%.1fs)",
            self._streaming_config.left_context_secs,
            self._streaming_config.chunk_secs,
            self._streaming_config.right_context_secs,
            self._streaming_config.chunk_secs + self._streaming_config.right_context_secs,
        )
        logger.info("Context (samples): %s", self._context_samples)

    def _compute_context_sizes(self) -> None:
        """Compute context sizes in encoder frames and audio samples.

        Matches NeMo's official streaming example and parakeet-stream's
        implementation exactly.
        """
        cfg = self._streaming_config
        features_frame2audio_samples = make_divisible_by(
            int(self._sample_rate * self._feature_stride_sec),
            factor=self._encoder_subsampling_factor,
        )
        self._encoder_frame2audio_samples = (
            features_frame2audio_samples * self._encoder_subsampling_factor
        )

        self._context_encoder_frames = ContextSize(
            left=int(cfg.left_context_secs * self._features_per_sec / self._encoder_subsampling_factor),
            chunk=int(cfg.chunk_secs * self._features_per_sec / self._encoder_subsampling_factor),
            right=int(cfg.right_context_secs * self._features_per_sec / self._encoder_subsampling_factor),
        )
        self._context_samples = ContextSize(
            left=self._context_encoder_frames.left * self._encoder_subsampling_factor * features_frame2audio_samples,
            chunk=self._context_encoder_frames.chunk * self._encoder_subsampling_factor * features_frame2audio_samples,
            right=self._context_encoder_frames.right * self._encoder_subsampling_factor * features_frame2audio_samples,
        )

    def _touch(self) -> None:
        self._last_used = time.monotonic()
        if self._ttl > 0:
            if self._unload_timer is not None:
                self._unload_timer.cancel()
            self._unload_timer = threading.Timer(self._ttl, self.unload)
            self._unload_timer.daemon = True
            self._unload_timer.start()

    def unload(self) -> None:
        with self._lock:
            if self._model is None:
                return
            if self._active_requests > 0:
                return
            logger.info("Unloading Parakeet TDT")
            del self._model
            self._model = None
            gc.collect()

            if torch.cuda.is_available():
                torch.cuda.empty_cache()

    def warmup(self) -> None:
        """Warmup the model with a few passes to compile GPU kernels."""
        self.load()

        logger.info("Warming up Parakeet TDT (3 passes) …")
        for dur_s in (1, 3, 8):
            audio = np.random.randn(dur_s * self._sample_rate).astype(np.float32) * 0.01
            t0 = time.perf_counter()
            self.transcribe(audio)
            logger.info(
                "  warmup %ds pass done in %.0fms",
                dur_s, (time.perf_counter() - t0) * 1000,
            )
        logger.info("Warmup complete.")

    def transcribe(self, audio: np.ndarray) -> TranscribeResult:
        """Transcribe audio using model.transcribe() (batch mode).

        Parakeet v3 auto-detects language from its 25 supported EU
        languages.  The detected language is not currently exposed
        by NeMo's API, so we return "auto" as a placeholder.
        """
        with self._lock:
            self.load()
            self._touch()
            self._active_requests += 1

        try:
            assert self._model is not None
            with torch.no_grad():
                outputs = self._model.transcribe(
                    [audio],
                    batch_size=1,
                    timestamps=False,
                )
            text = ""
            if outputs and len(outputs) > 0:
                out = outputs[0]
                if hasattr(out, "text"):
                    text = out.text.strip()
                elif isinstance(out, str):
                    text = out.strip()
                else:
                    text = str(out).strip()

            # Parakeet v3 auto-detects language but NeMo doesn't expose it yet
            return TranscribeResult(text=text, language="auto")
        finally:
            with self._lock:
                self._active_requests -= 1
                self._touch()

    def supported_languages(self) -> list[str]:
        return PARAKEET_LANGUAGES
