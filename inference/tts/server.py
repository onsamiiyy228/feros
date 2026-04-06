"""Shared FastAPI application factory for TTS servers.

Creates a FastAPI app with engine-agnostic POST /v1/tts endpoint
compatible with the voice pipeline (Rust TTS client).
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager

from fastapi import FastAPI, HTTPException
from fastapi.responses import Response
from pydantic import BaseModel, Field

from tts.engine import TtsEngine

logger = logging.getLogger("tts.server")

_pool = ThreadPoolExecutor(max_workers=2)


class TTSRequest(BaseModel):
    """TTS synthesis request (matches Rust TTS client payload)."""

    text: str = Field(..., description="Text to synthesize")
    reference_id: str | None = Field(
        None, description="Voice profile name"
    )
    seed: int | None = Field(
        None, description="RNG seed for consistent speaker identity"
    )


def create_engine(args: argparse.Namespace) -> TtsEngine:
    """Create the appropriate TTS engine based on CLI args."""
    if args.engine == "fish":
        from tts.fish.engine import FishSpeechEngine

        return FishSpeechEngine(
            llama_checkpoint_path=getattr(
                args, "llama_checkpoint_path", "checkpoints/openaudio-s1-mini"
            ),
            decoder_checkpoint_path=getattr(
                args, "decoder_checkpoint_path",
                "checkpoints/openaudio-s1-mini/codec.pth",
            ),
            references_dir=getattr(args, "references_dir", "tts/references"),
            device=args.device,
            compile=getattr(args, "compile", False),
        )
    elif args.engine == "moss":
        from tts.moss.engine import MossEngine

        return MossEngine(
            device=args.device,
        )
    else:
        raise ValueError(f"Unknown TTS engine: {args.engine}")


def create_app(args: argparse.Namespace) -> FastAPI:
    """Create a FastAPI app with the specified TTS engine."""
    engine: TtsEngine | None = None
    normalize_enabled: bool = getattr(args, "normalize", False)

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        nonlocal engine
        engine = create_engine(args)
        engine.warmup()

        # Pre-warm the normalizer so the first request isn't slow
        if normalize_enabled:
            from tts.normalizer import normalize_text

            normalize_text("warmup")
            logger.info("Text normalization enabled")

        yield

    app = FastAPI(
        title=f"TTS ({args.engine})",
        lifespan=lifespan,
    )

    @app.post("/v1/tts")
    async def tts(request: TTSRequest) -> Response:
        """Synthesize text to speech.

        Returns WAV audio compatible with the Rust TTS client.
        """
        if not request.text.strip():
            raise HTTPException(status_code=400, detail="Empty text")

        if engine is None:
            raise HTTPException(status_code=503, detail="Server not initialized")

        loop = asyncio.get_running_loop()

        # Apply text normalization if enabled
        synth_text = request.text
        if normalize_enabled:
            from tts.normalizer import normalize_text

            synth_text = normalize_text(synth_text)

        try:
            result = await loop.run_in_executor(
                _pool,
                lambda: engine.synthesize(
                    text=synth_text,
                    voice=request.reference_id or "default",
                    seed=request.seed,
                ),
            )
        except Exception as e:
            logger.exception("TTS synthesis failed")
            raise HTTPException(
                status_code=500,
                detail={"message": "Failed to generate speech", "error": str(e)},
            ) from e

        return Response(
            content=result.audio,
            media_type="audio/wav",
            headers={
                "X-Audio-Duration": f"{result.duration:.2f}",
            },
        )

    @app.get("/health")
    async def health() -> dict:
        return {
            "status": "ok" if (engine is not None and engine.is_loaded()) else "loading",
            "engine": args.engine,
        }

    return app
