"""Fish Speech TTS engine — direct inference via fish_speech library.

Uses the fish_speech package directly (installed via pip install -e)
rather than proxying to a separate API server.
"""

from __future__ import annotations

import io
import logging
import time

import numpy as np
import soundfile as sf

from tts.engine import SynthResult, TtsEngine

logger = logging.getLogger("tts.fish")


class FishSpeechEngine(TtsEngine):
    """Direct Fish Speech inference engine.

    Loads the llama + decoder models and runs inference in-process.
    No separate API server needed.
    """

    def __init__(
        self,
        llama_checkpoint_path: str = "checkpoints/openaudio-s1-mini",
        decoder_checkpoint_path: str = "checkpoints/openaudio-s1-mini/codec.pth",
        decoder_config_name: str = "modded_dac_vq",
        references_dir: str = "tts/references",
        device: str = "cuda",
        half: bool = False,
        compile: bool = False,
    ) -> None:
        self._llama_checkpoint_path = llama_checkpoint_path
        self._decoder_checkpoint_path = decoder_checkpoint_path
        self._decoder_config_name = decoder_config_name
        self._references_dir = references_dir
        self._device = device
        self._half = half
        self._compile = compile
        self._engine = None  # TTSInferenceEngine
        self._sample_rate: int = 44100

    def is_loaded(self) -> bool:
        return self._engine is not None

    def load(self) -> None:
        if self._engine is not None:
            return

        import torch

        from fish_speech.inference_engine import TTSInferenceEngine
        from fish_speech.models.dac.inference import load_model as load_decoder_model
        from fish_speech.models.text2semantic.inference import (
            launch_thread_safe_queue,
        )

        precision = torch.half if self._half else torch.bfloat16

        # Check device availability
        device = self._device
        if torch.backends.mps.is_available():
            device = "mps"
        elif not torch.cuda.is_available() and device == "cuda":
            device = "cpu"
            logger.warning("CUDA not available, falling back to CPU")

        logger.info("Loading Fish Speech llama model from %s …", self._llama_checkpoint_path)
        llama_queue = launch_thread_safe_queue(
            checkpoint_path=self._llama_checkpoint_path,
            device=device,
            precision=precision,
            compile=self._compile,
        )

        logger.info("Loading Fish Speech decoder model …")
        decoder_model = load_decoder_model(
            config_name=self._decoder_config_name,
            checkpoint_path=self._decoder_checkpoint_path,
            device=device,
        )

        self._engine = TTSInferenceEngine(
            llama_queue=llama_queue,
            decoder_model=decoder_model,
            precision=precision,
            compile=self._compile,
        )

        # Fish Speech's ReferenceLoader hardcodes Path("references").
        # Ensure it resolves to our configured references directory.
        self._ensure_references_dir()

        # Get sample rate from decoder
        if hasattr(decoder_model, "spec_transform"):
            self._sample_rate = decoder_model.spec_transform.sample_rate
        else:
            self._sample_rate = decoder_model.sample_rate

        logger.info(
            "Fish Speech loaded on %s (sample_rate=%d, compile=%s)",
            device, self._sample_rate, self._compile,
        )

    def unload(self) -> None:
        if self._engine is None:
            return

        import gc

        import torch

        del self._engine
        self._engine = None
        gc.collect()
        if torch.cuda.is_available():
            torch.cuda.empty_cache()

    def warmup(self) -> None:
        self.load()

        from fish_speech.utils.schema import ServeTTSRequest

        logger.info("Warming up Fish Speech …")
        req = ServeTTSRequest(
            text="Hello world.",
            references=[],
            reference_id=None,
            max_new_tokens=1024,
            chunk_length=200,
            top_p=0.7,
            repetition_penalty=1.2,
            temperature=0.7,
            format="wav",
        )
        # Drain the generator to trigger compilation
        for _ in self._engine.inference(req):
            pass
        logger.info("Fish Speech warmup complete.")

    def synthesize(
        self,
        text: str,
        voice: str | None = None,
        seed: int | None = None,
    ) -> SynthResult:
        if self._engine is None:
            raise RuntimeError("Fish Speech not loaded")

        from fish_speech.utils.schema import ServeTTSRequest

        req = ServeTTSRequest(
            text=text,
            references=[],
            reference_id=voice,
            seed=seed,
            max_new_tokens=1024,
            chunk_length=200,
            top_p=0.7,
            repetition_penalty=1.2,
            temperature=0.7,
            format="wav",
        )

        t0 = time.perf_counter()
        audio_np: np.ndarray | None = None

        for result in self._engine.inference(req):
            if result.code == "error":
                raise RuntimeError(f"Fish Speech inference error: {result.error}")
            elif result.code == "final":
                if isinstance(result.audio, tuple):
                    audio_np = result.audio[1]

        if audio_np is None:
            raise RuntimeError("No audio generated")

        dt = time.perf_counter() - t0
        duration = len(audio_np) / self._sample_rate

        # Encode as WAV
        buf = io.BytesIO()
        sf.write(buf, audio_np, self._sample_rate, format="WAV", subtype="PCM_16")
        wav_bytes = buf.getvalue()

        logger.info(
            "Synthesized %.1fs audio in %.2fs (RTF=%.2fx) → '%s'",
            duration, dt, duration / dt if dt > 0 else 0, text[:60],
        )

        return SynthResult(
            audio=wav_bytes,
            sample_rate=self._sample_rate,
            duration=duration,
        )

    def supported_voices(self) -> list[str]:
        if self._engine is None:
            return []
        return self._engine.list_reference_ids()

    def _ensure_references_dir(self) -> None:
        """Ensure Fish Speech's hardcoded 'references' path resolves correctly.

        Fish Speech's ReferenceLoader uses Path("references") relative to CWD.
        If our configured references_dir is different, create a symlink.
        """
        from pathlib import Path

        target = Path(self._references_dir).resolve()
        link = Path("references")

        # Already correct
        if link.resolve() == target:
            return

        # Target doesn't exist — nothing to link
        if not target.exists():
            logger.info("References dir %s does not exist, skipping", target)
            return

        # Create or update symlink
        if link.is_symlink():
            link.unlink()
        elif link.exists():
            # Real directory exists — don't overwrite
            logger.info("references/ already exists as real directory, using it")
            return

        link.symlink_to(target)
        logger.info("Symlinked references → %s", target)
