# syntax=docker/dockerfile:1
FROM python:3.12-slim

WORKDIR /workspace/studio/api

# Install system deps (build-essential for Rust/C extensions, libssl-dev + pkg-config for openssl-sys)
RUN apt-get update && apt-get install -y --no-install-recommends --fix-missing \
    build-essential \
    clang \
    cmake \
    curl \
    libclang-dev \
    patch \
    pkg-config \
    libssl-dev \
    libopus-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Install Rust (required to compile voice-engine)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"
# voice-engine/.cargo/config.toml sets rustc-wrapper=sccache for local dev.
# sccache is not installed in this image — clear the wrapper so cargo uses rustc directly.
ENV RUSTC_WRAPPER=""

# Keep the venv outside /app so it isn't shadowed by the ./studio/api bind mount
ENV UV_PROJECT_ENVIRONMENT=/opt/venv

# Install uv
COPY --from=ghcr.io/astral-sh/uv:0.10.9 /uv /usr/local/bin/uv

# ── Stage 1: Install all third-party Python deps ─────────────────────────────
# voice-engine is a path dependency (not a uv workspace member), so
# --no-install-workspace does nothing for it.  Use --no-install-package instead.
# uv still needs voice-engine/pyproject.toml to resolve the lock file (it reads
# package metadata from the path source), but does NOT need Cargo.toml / src/.
# This layer is only invalidated when pyproject.toml / uv.lock change.
COPY studio/api/pyproject.toml studio/api/uv.lock* ./
COPY voice/engine/pyproject.toml /workspace/voice/engine/pyproject.toml
COPY integrations/pyproject.toml /workspace/integrations/pyproject.toml
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --frozen --no-dev --no-install-package voice-engine --no-install-package integrations

# ── Stage 2: Compile the local voice-engine and integrations PyO3 extensions ──
# Copying sources here means source changes only invalidate this step,
# not the expensive third-party download step above.
COPY voice/engine /workspace/voice/engine
COPY integrations /workspace/integrations
COPY proto /workspace/proto
RUN --mount=type=cache,target=/root/.cache/uv \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/workspace/voice/engine/target \
    --mount=type=cache,target=/workspace/integrations/target \
    uv sync --frozen --no-dev

# ── Stage 3: Copy application source ─────────────────────────────────────────
COPY studio/api/ .

EXPOSE 8000 8300

CMD ["uv", "run", "python", "-m", "uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
