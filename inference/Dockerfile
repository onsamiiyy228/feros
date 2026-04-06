FROM nvidia/cuda:12.8.0-cudnn-runtime-ubuntu22.04

ENV DEBIAN_FRONTEND=noninteractive
ENV PYTHONUNBUFFERED=1

# Install base dependencies and uv
RUN apt-get update && apt-get install -y \
    python3 python3-pip python3-dev \
    git curl wget bzip2 build-essential ffmpeg libsndfile1 \
    && rm -rf /var/lib/apt/lists/* \
    && curl -LsSf https://astral.sh/uv/install.sh | sh

ENV PATH="/root/.local/bin:$PATH"
WORKDIR /workspace

# Install PyTorch
RUN uv pip install --system torch==2.8.0 torchaudio==2.8.0 \
    --index-url https://download.pytorch.org/whl/cu128

# Install Core Server Deps
RUN uv pip install --system fastapi "uvicorn[standard]" websockets python-multipart loguru soundfile

# Required ARG to determine which model dependencies to install
# e.g., 'whisper', 'parakeet', 'fish', or 'moss'
ARG ENGINE

# Conditionally install specific engine dependencies to prevent container bloat
RUN if [ "$ENGINE" = "whisper" ]; then \
        echo "Installing Whisper STT dependencies..." && \
        uv pip install --system faster-whisper; \
    elif [ "$ENGINE" = "parakeet" ]; then \
        echo "Installing Parakeet STT dependencies..." && \
        uv pip install --system "nemo_toolkit[asr]" fiddle cloudpickle nv-one-logger-training-telemetry nv-one-logger-core "lhotse>=1.32.2" sentencepiece "kaldialign<=0.9.1" kaldi-python-io braceexpand editdistance "jiwer>=3.1.0,<4.0.0" "librosa>=0.10.1" pyannote.core pyannote.metrics pydub pyloudnorm resampy sacrebleu "sox<=1.5.0" whisper_normalizer webdataset omegaconf hydra-core pytorch-lightning protobuf; \
    elif [ "$ENGINE" = "fish" ]; then \
        echo "Installing Fish-Speech TTS dependencies..." && \
        git clone https://github.com/fishaudio/fish-speech.git /workspace/fish-speech && \
        cd /workspace/fish-speech && uv pip install --system --extra-index-url https://download.pytorch.org/whl/cu128 -e ".[stable]"; \
    elif [ "$ENGINE" = "moss" ]; then \
        echo "Installing MOSS TTS dependencies..." && \
        git clone https://github.com/OpenMOSS/MOSS-TTS.git /workspace/MOSS-TTS && \
        cd /workspace/MOSS-TTS && uv pip install --system --extra-index-url https://download.pytorch.org/whl/cu128 -e "." && \
        MAX_JOBS=4 uv pip install --system --extra-index-url https://download.pytorch.org/whl/cu128 -e ".[flash-attn]" || true; \
    else \
        echo "ERROR: Invalid or missing ENGINE arg. Must be whisper, parakeet, fish, or moss." && exit 1; \
    fi

# Copy source code
COPY . /workspace

EXPOSE 9001
EXPOSE 9002

ENTRYPOINT ["python3", "cli.py"]
