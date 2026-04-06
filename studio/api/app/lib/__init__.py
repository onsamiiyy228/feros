"""Core application package — re-exports from submodules for convenience."""

from pathlib import Path

from app.lib.config import (
    AuthConfig,
    DatabaseConfig,
    Environment,
    GeminiConfig,
    LLMConfig,
    RedisConfig,
    Settings,
    STTConfig,
    TelephonyConfig,
    TTSConfig,
    get_settings,
)


def _find_project_root() -> Path:
    """Walk up from this file to find the directory containing pyproject.toml."""
    current = Path(__file__).resolve().parent
    while current != current.parent:
        if (current / "pyproject.toml").exists():
            return current
        current = current.parent
    raise RuntimeError("Could not find project root (no pyproject.toml found)")


PROJECT_ROOT = _find_project_root()

__all__ = [
    "PROJECT_ROOT",
    "AuthConfig",
    "DatabaseConfig",
    "Environment",
    "GeminiConfig",
    "LLMConfig",
    "RedisConfig",
    "STTConfig",
    "Settings",
    "TTSConfig",
    "TelephonyConfig",
    "get_settings",
]
