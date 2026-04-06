"""MOSS-TTS engine — direct inference with MOSS-TTS-Local-Transformer.

Wraps the MOSS-TTS model (1.7B) in our TtsEngine interface.
Supports voice cloning from WAV files in voices/ directory.
"""

from __future__ import annotations

import gc
import importlib.util
import io
import logging
import time
from pathlib import Path
from typing import Any

import numpy as np
import soundfile as sf
import torch
import torchaudio
from transformers import GenerationConfig

from tts.engine import SynthResult, TtsEngine

logger = logging.getLogger("tts.moss")

# ── Constants ────────────────────────────────────────────────────

MODEL_ID = "OpenMOSS-Team/MOSS-TTS-Local-Transformer"
SAMPLE_RATE = 24000

DEFAULT_N_VQ = 16
DEFAULT_TEXT_TEMP = 1.5
DEFAULT_TEXT_TOP_P = 1.0
DEFAULT_TEXT_TOP_K = 50
DEFAULT_AUDIO_TEMP = 0.95
DEFAULT_AUDIO_TOP_P = 0.95
DEFAULT_AUDIO_TOP_K = 50
DEFAULT_AUDIO_REP_PENALTY = 1.1
DEFAULT_MAX_NEW_TOKENS = 4096

REFERENCES_DIR = Path(__file__).resolve().parent.parent / "references"


class DelayGenerationConfig(GenerationConfig):
    """Custom GenerationConfig for MOSS-TTS-Local-Transformer."""

    def __init__(self, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.layers = kwargs.get("layers", [{} for _ in range(32)])
        self.do_samples = kwargs.get("do_samples", None)
        self.n_vq_for_inference = 32


class MossEngine(TtsEngine):
    """MOSS-TTS-Local-Transformer (1.7B) engine.

    - Direct inference via HuggingFace transformers
    - 24kHz output, voice cloning from WAV reference files
    - Auto-selects best attention backend (flash_attn > sdpa > eager)
    """

    def __init__(self, device: str = "cuda") -> None:
        self._device_str = device
        self._model: Any = None
        self._processor: Any = None
        self._device = torch.device("cpu")
        self._dtype = torch.float32
        self._sample_rate = SAMPLE_RATE
        self._voice_profiles: dict[str, str] = {}

    def is_loaded(self) -> bool:
        return self._model is not None

    def load(self) -> None:
        if self._model is not None:
            return

        from transformers import AutoModel, AutoProcessor

        # CUDA backend config
        torch.backends.cuda.enable_cudnn_sdp(False)
        torch.backends.cuda.enable_flash_sdp(True)
        torch.backends.cuda.enable_mem_efficient_sdp(True)
        torch.backends.cuda.enable_math_sdp(True)

        self._device = torch.device(
            self._device_str if torch.cuda.is_available() else "cpu"
        )
        self._dtype = torch.bfloat16 if self._device.type == "cuda" else torch.float32

        logger.info("Loading MOSS-TTS processor from %s …", MODEL_ID)
        self._processor = AutoProcessor.from_pretrained(
            MODEL_ID, trust_remote_code=True
        )
        if hasattr(self._processor, "audio_tokenizer"):
            self._processor.audio_tokenizer = self._processor.audio_tokenizer.to(
                self._device
            )

        if self._device.type == "cuda":
            torch.cuda.empty_cache()
            gc.collect()

        attn_impl = self._resolve_attn()
        logger.info("Loading MOSS-TTS model (dtype=%s, attn=%s) …", self._dtype, attn_impl)

        model_kwargs: dict[str, Any] = {
            "trust_remote_code": True,
            "torch_dtype": self._dtype,
            "low_cpu_mem_usage": True,
        }
        if attn_impl:
            model_kwargs["attn_implementation"] = attn_impl

        self._model = AutoModel.from_pretrained(MODEL_ID, **model_kwargs).to(
            self._device
        )
        self._model.eval()

        if self._device.type == "cuda":
            torch.cuda.empty_cache()
            gc.collect()

        self._sample_rate = int(
            getattr(self._processor.model_config, "sampling_rate", SAMPLE_RATE)
        )
        logger.info(
            "MOSS-TTS loaded on %s | sample_rate=%d | GPU=%.1fGB",
            self._device,
            self._sample_rate,
            (
                torch.cuda.memory_allocated(self._device) / 1e9
                if self._device.type == "cuda"
                else 0
            ),
        )

        self._load_voice_profiles()

    def unload(self) -> None:
        if self._model is None:
            return
        del self._model
        self._model = None
        del self._processor
        self._processor = None
        gc.collect()
        if torch.cuda.is_available():
            torch.cuda.empty_cache()

    def synthesize(
        self,
        text: str,
        voice: str | None = None,
        seed: int | None = None,
    ) -> SynthResult:
        if self._model is None or self._processor is None:
            raise RuntimeError("Model not loaded")

        # Resolve voice → reference audio path
        ref_audio: str | None = None
        if voice and voice in self._voice_profiles:
            ref_audio = self._voice_profiles[voice]
        elif voice:
            logger.warning("Unknown voice '%s', using random voice", voice)

        # Build conversation
        user_kwargs: dict[str, Any] = {"text": text}
        if ref_audio:
            user_kwargs["reference"] = [ref_audio]

        conversations = [[self._processor.build_user_message(**user_kwargs)]]
        batch = self._processor(conversations, mode="generation")
        input_ids = batch["input_ids"].to(self._device)
        attention_mask = batch["attention_mask"].to(self._device)

        # Build generation config
        gen_config = self._build_gen_config()

        if self._device.type == "cuda":
            torch.cuda.empty_cache()
            gc.collect()

        t0 = time.perf_counter()
        with torch.no_grad():
            if seed is not None:
                torch.manual_seed(seed)
                if self._device.type == "cuda":
                    torch.cuda.manual_seed(seed)
            outputs = self._model.generate(
                input_ids=input_ids,
                attention_mask=attention_mask,
                generation_config=gen_config,
            )

        decoded_messages = self._processor.decode(outputs)
        if not decoded_messages or decoded_messages[0] is None:
            raise RuntimeError("Model did not return decodable audio")

        audio = decoded_messages[0].audio_codes_list[0]
        dt = time.perf_counter() - t0
        duration = len(audio) / self._sample_rate

        del outputs, input_ids, attention_mask, decoded_messages
        if self._device.type == "cuda":
            torch.cuda.empty_cache()
            gc.collect()

        # Convert to WAV bytes
        wav_bytes = self._audio_to_wav(audio)

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
        return list(self._voice_profiles.keys())

    # ── Private helpers ──────────────────────────────────────────

    def _resolve_attn(self) -> str | None:
        if (
            self._device.type == "cuda"
            and importlib.util.find_spec("flash_attn") is not None
            and self._dtype in {torch.float16, torch.bfloat16}
        ):
            major, _ = torch.cuda.get_device_capability(self._device)
            if major >= 8:
                return "flash_attention_2"
        if self._device.type == "cuda":
            return "sdpa"
        return "eager"

    def _load_voice_profiles(self) -> None:
        self._voice_profiles.clear()
        if not REFERENCES_DIR.exists():
            logger.info("No references/ directory found")
            return
        for voice_dir in sorted(REFERENCES_DIR.iterdir()):
            if not voice_dir.is_dir():
                continue
            # Find first WAV file in the voice directory
            wav_files = list(voice_dir.glob("*.wav"))
            if wav_files:
                name = voice_dir.name
                self._voice_profiles[name] = str(wav_files[0].resolve())
                logger.info("Loaded voice profile: '%s' (%s)", name, wav_files[0].name)

    def _build_gen_config(self) -> DelayGenerationConfig:
        n_vq = DEFAULT_N_VQ
        gen_config = DelayGenerationConfig()
        gen_config.pad_token_id = self._processor.tokenizer.pad_token_id
        gen_config.eos_token_id = 151653
        gen_config.max_new_tokens = DEFAULT_MAX_NEW_TOKENS
        gen_config.use_cache = True
        gen_config.do_sample = True
        gen_config.num_beams = 1
        gen_config.n_vq_for_inference = n_vq
        gen_config.do_samples = [True] * (n_vq + 1)
        gen_config.layers = [
            {
                "repetition_penalty": 1.0,
                "temperature": DEFAULT_TEXT_TEMP,
                "top_p": DEFAULT_TEXT_TOP_P,
                "top_k": DEFAULT_TEXT_TOP_K,
            }
        ] + [
            {
                "repetition_penalty": DEFAULT_AUDIO_REP_PENALTY,
                "temperature": DEFAULT_AUDIO_TEMP,
                "top_p": DEFAULT_AUDIO_TOP_P,
                "top_k": DEFAULT_AUDIO_TOP_K,
            }
        ] * n_vq
        return gen_config

    def _audio_to_wav(self, audio: torch.Tensor) -> bytes:
        audio_np = audio.cpu().float().numpy()
        buf = io.BytesIO()
        sf.write(buf, audio_np, self._sample_rate, format="WAV", subtype="PCM_16")
        return buf.getvalue()
