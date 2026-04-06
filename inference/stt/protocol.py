"""Shared STT WebSocket protocol and session management.

Wire format (schema in proto/stt.proto, serialized as protobuf canonical JSON):
  Audio:   Binary WS frames (raw PCM-16 LE)
  Control: JSON text frames  {"finalize": {}} / {"close": {}}
  Results: JSON text frames  {"transcript": {"is_final": true, "text": "...", ...}}
"""

from __future__ import annotations

import logging
import time

import numpy as np
from google.protobuf import json_format

from stt.audio import BYTES_PER_SAMPLE, SAMPLE_RATE, pcm16_to_float32
from stt.engine import SttEngine
from stt.vad import SileroVadFilter
from stt.stt_pb2 import SttRequest, SttResponse, TranscriptResult as PbTranscriptResult

logger = logging.getLogger("stt.protocol")


def build_transcript_response(
    text: str,
    confidence: float,
    is_final: bool,
    start_time: float,
    duration: float,
) -> dict:
    """Build an SttResponse dict with TranscriptResult."""
    tr = PbTranscriptResult(
        is_final=is_final,
        text=text,
        confidence=confidence,
        start_time=start_time,
        duration=duration,
    )
    resp = SttResponse(transcript=tr)
    return json_format.MessageToDict(resp, preserving_proto_field_name=True)


class StreamingSession:
    """Per-connection streaming state.

    Accumulates PCM audio and runs a single batch transcription
    on finalize.  No interim processing — the Rust client only
    uses final results, so interim GPU work is wasted.
    """

    def __init__(
        self,
        engine: SttEngine,
        vad_filter: SileroVadFilter | None = None,
    ) -> None:
        self._engine = engine
        self._vad_filter = vad_filter
        self._audio_buffer = bytearray()

    def append_audio(self, data: bytes) -> None:
        """Append raw PCM-16 audio bytes to the buffer."""
        self._audio_buffer.extend(data)

    def finalize(self) -> dict:
        """Run final inference on all audio and return response.

        Uses batch transcription with VAD filtering for maximum
        accuracy.  The VAD strips any accumulated silence to prevent
        hallucinations (especially after barge-in).
        """
        all_pcm = bytes(self._audio_buffer)
        total_duration = len(all_pcm) / BYTES_PER_SAMPLE / SAMPLE_RATE

        if len(all_pcm) < BYTES_PER_SAMPLE * 100:
            text = ""
        else:
            audio = pcm16_to_float32(all_pcm)

            # VAD filter: strip silence to prevent hallucinations
            if self._vad_filter is not None:
                audio = self._vad_filter.collect_speech(audio)

            if len(audio) < 100:
                text = ""
            else:
                # Use batch transcription for maximum accuracy
                t0 = time.perf_counter()
                result = self._engine.transcribe(audio)
                text = result.text
                dt = time.perf_counter() - t0
                logger.info(
                    "Finalized %.1fs audio in %.0fms (lang=%s): '%s'",
                    len(audio) / SAMPLE_RATE,
                    dt * 1000,
                    result.language,
                    text[:80],
                )

        self._audio_buffer.clear()

        return build_transcript_response(
            text=text,
            confidence=0.99,
            is_final=True,
            start_time=0.0,
            duration=round(total_duration, 3),
        )
