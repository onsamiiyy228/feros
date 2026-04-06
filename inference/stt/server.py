"""Shared FastAPI application factory for STT servers.

Creates a FastAPI app with engine-agnostic WebSocket handler
using the proto-based protocol.
"""

from __future__ import annotations

import argparse
import asyncio
import logging
from concurrent.futures import ThreadPoolExecutor
from contextlib import asynccontextmanager

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from google.protobuf import json_format

from stt.engine import SttEngine
from stt.protocol import StreamingSession
from stt.vad import SileroVadFilter
from stt.stt_pb2 import SttRequest

logger = logging.getLogger("stt.server")

_pool = ThreadPoolExecutor(max_workers=2)


def create_engine(args: argparse.Namespace) -> SttEngine:
    """Create the appropriate engine based on CLI args."""
    if args.engine == "parakeet":
        from stt.parakeet.engine import ParakeetEngine, StreamingConfig

        return ParakeetEngine(
            model_name=args.model,
            device=args.device,
            streaming_config=StreamingConfig(
                chunk_secs=getattr(args, "chunk_secs", 1.0),
                left_context_secs=getattr(args, "left_context_secs", 10.0),
                right_context_secs=getattr(args, "right_context_secs", 1.0),
            ),
            ttl=args.ttl,
        )
    elif args.engine == "whisper":
        from stt.whisper.engine import WhisperEngine

        return WhisperEngine(
            model_name=args.model,
            device=args.device,
            compute_type=getattr(args, "compute_type", "float16"),
            language=args.language,
            ttl=args.ttl,
        )
    else:
        raise ValueError(f"Unknown engine: {args.engine}")


def create_app(args: argparse.Namespace) -> FastAPI:
    """Create a FastAPI app with the specified STT engine."""
    engine: SttEngine | None = None
    vad_filter: SileroVadFilter | None = None

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        nonlocal engine, vad_filter
        engine = create_engine(args)
        engine.warmup()
        # Whisper uses its own built-in Silero VAD (vad_filter=True).
        # Only load our VAD for Parakeet.
        if args.engine != "whisper":
            vad_filter = SileroVadFilter()
            vad_filter.preload()
        yield

    app = FastAPI(
        title=f"STT ({args.engine})",
        lifespan=lifespan,
    )

    @app.websocket("/v1/listen")
    async def streaming_transcribe(
        ws: WebSocket,
        language: str = "en",
    ) -> None:
        """Streaming STT — binary audio + JSON control protocol.

        Audio accumulates in a buffer.  When the client sends a
        ``finalize`` control message, batch transcription runs once
        and returns the final result.

        Wire format (schema defined in proto/stt.proto):
          Binary frames: raw PCM-16 LE audio
          JSON text frames: proto types serialized via json_format
        """
        await ws.accept()
        logger.info("WS /v1/listen connected (lang=%s, engine=%s)", language, args.engine)

        if engine is None:
            await ws.close(1011, "server not initialized")
            return

        session = StreamingSession(engine=engine, vad_filter=vad_filter)
        loop = asyncio.get_running_loop()

        try:
            while True:
                msg = await ws.receive()

                # Binary frame — raw PCM-16 audio (just buffer it)
                if "bytes" in msg and msg["bytes"] is not None:
                    data = msg["bytes"]
                    if data:
                        session.append_audio(data)

                # Text frame — JSON control message (proto canonical format)
                elif "text" in msg and msg["text"] is not None:
                    text = msg["text"]

                    try:
                        req = json_format.Parse(text, SttRequest())
                    except json_format.ParseError:
                        logger.warning("Invalid JSON control message")
                        continue

                    msg_type = req.WhichOneof("request")

                    if msg_type == "finalize":
                        result_dict = await loop.run_in_executor(
                            _pool,
                            session.finalize,
                        )
                        await ws.send_json(result_dict)

                    elif msg_type == "close":
                        logger.info("Client requested close")
                        break

                    else:
                        logger.warning("Unknown control message type: %s", msg_type)

        except WebSocketDisconnect:
            logger.info("Client disconnected")
        except Exception as e:
            logger.error("WS error: %s", e, exc_info=True)
        finally:
            try:
                await ws.close()
            except Exception:
                pass

    @app.get("/health")
    async def health() -> dict:
        return {
            "status": "ok" if (engine is not None and engine.is_loaded()) else "loading",
            "engine": args.engine,
        }

    return app
