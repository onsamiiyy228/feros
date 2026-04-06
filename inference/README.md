# Speech Inference — Self-Hosted Voice Infrastructure

Private ML inference server for Prime8 Engine. Hosts **STT** (Faster-Whisper / Parakeet)
and **TTS** (Fish Speech / MOSS-TTS) natively on local GPUs.

This module has been re-architected away from slow segmented HTTP REST APIs (like Speaches) to strict, highly-optimized **WebSocket streaming** bridging directly into PyTorch via `uv`.

## Architecture & Flexibility

To prevent severe image bloat (bundling thousands of dependencies for unused frameworks into a single 30GB image), the Dockerfile strictly requires an `ENGINE` argument. The container will selectively install *only* what is necessary for that explicit engine.

### Available Engines
* **STT**: `whisper`, `parakeet`
* **TTS**: `fish`, `moss`

## Quick Start (Global Tasks)

Interaction with the inference Docker suite is now managed globally via the root `Makefile`. You do not need to drop into this directory.

```bash
# 1. Build strict, isolated Docker Images using uv pip
make inf-build-stt   # Builds 'prime8-inference-stt' (whisper)
make inf-build-tts   # Builds 'prime8-inference-tts' (fish)

# 2. Start the standalone containers on separate GPUs
make inf-stt         # Port 9001
make inf-tts         # Port 9002

# 3. Verify health
make inf-health      # Pings :9001/health and :9002/health
```

## Engine Integration & Routing

Because this server skips standard HTTP REST payloads to achieve ultra-low latency, `voice/engine` interacts with these containers using our custom binary WebSocket protocol (`v1/listen`).

To use these self-hosted models in the Rust engine, specify the `builtin` provider:

```json
{
  "stt_config": {
    "provider": "builtin",
    "base_url": "ws://localhost:9001",
    "model": "whisper"
  },
  "tts_config": {
    "provider": "builtin",
    "base_url": "ws://localhost:9002",
    "model": "fish"
  }
}
```

## Hardware Requirements

- **2× NVIDIA GPU** (≥12GB VRAM each recommended)
- **NVIDIA Container Toolkit** (`nvidia-docker`)

STT and TTS run on **separate containers** and should ideally be scheduled on **separate GPUs** to entirely eliminate contention during barge-in interruptions.
