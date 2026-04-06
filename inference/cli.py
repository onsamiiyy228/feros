#!/usr/bin/env python3
"""Speech Inference — CLI launcher.

Usage:
    python cli.py stt --engine parakeet --port 9001
    python cli.py stt --engine whisper --port 9001 --language auto
    python cli.py stt --engine whisper --gpu 1 --compute-type int8_float16

    python cli.py tts --engine fish --port 9002
    python cli.py tts --engine moss --gpu 1 --port 9002
"""

from __future__ import annotations

import argparse
import logging
import os
import sys

import uvicorn

from stt.server import create_app

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)-8s | %(name)s | %(message)s",
)
logger = logging.getLogger("voice-agent-os")

# Default models per engine
DEFAULT_MODELS = {
    "parakeet": "nvidia/parakeet-tdt-0.6b-v3",
    "whisper": "deepdml/faster-whisper-large-v3-turbo-ct2",
}


def _add_stt_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the 'stt' subcommand."""
    stt = subparsers.add_parser(
        "stt",
        help="Start an STT server (parakeet or whisper)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python cli.py stt --engine parakeet --port 9001
  python cli.py stt --engine whisper --port 9001 --language auto
  python cli.py stt --engine whisper --gpu 1 --compute-type int8_float16
        """,
    )

    # ── Common arguments ────────────────────────────────────────
    stt.add_argument(
        "--engine", required=True,
        choices=["parakeet", "whisper"],
        help="STT engine to use",
    )
    stt.add_argument("--model", default=None, help="Model name override")
    stt.add_argument("--device", default="cuda", choices=["cuda", "cpu"])
    stt.add_argument(
        "--gpu", type=int, default=None,
        help="GPU index to use (e.g. 0, 1). Sets CUDA_VISIBLE_DEVICES. "
             "Default: uses CUDA_VISIBLE_DEVICES from env, or all GPUs.",
    )
    stt.add_argument(
        "--language", default=None,
        help="Language code (e.g. 'en', 'zh') or 'auto' for auto-detect. "
             "Default: auto.",
    )
    stt.add_argument("--host", default="0.0.0.0")
    stt.add_argument("--port", type=int, default=9001)
    stt.add_argument(
        "--ttl", type=int, default=-1,
        help="Seconds before idle model auto-unloads (-1 = never)",
    )

    # ── Whisper-specific ────────────────────────────────────────
    stt.add_argument(
        "--compute-type", default="float16",
        help="CTranslate2 quantization (float16, int8, int8_float16). Whisper only.",
    )

    # ── Parakeet-specific ───────────────────────────────────────
    stt.add_argument(
        "--chunk-secs", type=float, default=1.0,
        help="Chunk size in seconds. Parakeet only.",
    )
    stt.add_argument(
        "--left-context-secs", type=float, default=10.0,
        help="Left context in seconds. Parakeet only.",
    )
    stt.add_argument(
        "--right-context-secs", type=float, default=1.0,
        help="Right context in seconds. Parakeet only.",
    )

    stt.set_defaults(func=_run_stt)


def _add_tts_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the 'tts' subcommand."""
    tts = subparsers.add_parser(
        "tts",
        help="Start a TTS server (fish or moss)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python cli.py tts --engine fish --port 9002
  python cli.py tts --engine moss --gpu 1 --port 9002
        """,
    )

    tts.add_argument(
        "--engine", required=True,
        choices=["fish", "moss"],
        help="TTS engine to use",
    )
    tts.add_argument("--device", default="cuda", choices=["cuda", "cpu"])
    tts.add_argument(
        "--gpu", type=int, default=None,
        help="GPU index to use (e.g. 0, 1). Sets CUDA_VISIBLE_DEVICES.",
    )
    tts.add_argument("--host", default="0.0.0.0")
    tts.add_argument("--port", type=int, default=9002)

    # ── Fish-specific ───────────────────────────────────────────
    tts.add_argument(
        "--llama-checkpoint-path",
        default="checkpoints/openaudio-s1-mini",
        help="Path to Fish Speech llama checkpoint (fish engine only).",
    )
    tts.add_argument(
        "--decoder-checkpoint-path",
        default="checkpoints/openaudio-s1-mini/codec.pth",
        help="Path to Fish Speech decoder checkpoint (fish engine only).",
    )
    tts.add_argument(
        "--compile", action="store_true",
        help="Enable torch.compile for faster inference (fish engine only).",
    )
    tts.add_argument(
        "--references-dir", default="tts/references",
        help="Directory containing voice references (fish engine only).",
    )
    tts.add_argument(
        "--normalize", action="store_true",
        help="Enable NeMo text normalization (converts $24/month → "
             "twenty four dollars per month). Requires nemo_text_processing.",
    )

    tts.set_defaults(func=_run_tts)


def _run_tts(args: argparse.Namespace) -> None:
    """Launch the TTS server."""
    if args.gpu is not None:
        os.environ["CUDA_VISIBLE_DEVICES"] = str(args.gpu)
        logger.info("CUDA_VISIBLE_DEVICES=%s (GPU %d)", args.gpu, args.gpu)

    from tts.server import create_app as create_tts_app

    app = create_tts_app(args)

    logger.info(
        "Starting TTS server: engine=%s port=%d",
        args.engine, args.port,
    )

    uvicorn.run(
        app,
        host=args.host,
        port=args.port,
        log_level="info",
    )


def _run_stt(args: argparse.Namespace) -> None:
    """Launch the STT server."""
    # GPU selection
    if args.gpu is not None:
        os.environ["CUDA_VISIBLE_DEVICES"] = str(args.gpu)
        logger.info("CUDA_VISIBLE_DEVICES=%s (GPU %d)", args.gpu, args.gpu)

    # Default model per engine
    if args.model is None:
        args.model = DEFAULT_MODELS[args.engine]

    # Default language
    if args.language is None:
        args.language = "auto"

    app = create_app(args)

    logger.info(
        "Starting STT server: engine=%s model=%s language=%s port=%d",
        args.engine, args.model, args.language, args.port,
    )

    uvicorn.run(
        app,
        host=args.host,
        port=args.port,
        log_level="info",
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="cli.py",
        description="Speech Inference services",
    )
    subparsers = parser.add_subparsers(dest="command")

    _add_stt_parser(subparsers)
    _add_tts_parser(subparsers)

    args = parser.parse_args()
    if not hasattr(args, "func"):
        parser.print_help()
        sys.exit(1)

    args.func(args)


if __name__ == "__main__":
    main()
