"""LLM construction utilities for Pydantic AI.

Translates Voice Agent OS configuration (LLMConfig) into ready-to-use
Pydantic AI `Model` instances.
"""

from __future__ import annotations

from pydantic_ai.models import Model
from pydantic_ai.models.google import GoogleModel
from pydantic_ai.models.openai import OpenAIChatModel
from pydantic_ai.models.openrouter import OpenRouterModel, OpenRouterModelSettings
from pydantic_ai.providers.google import GoogleProvider
from pydantic_ai.providers.ollama import OllamaProvider
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.providers.openrouter import OpenRouterProvider

from app.lib.config import LLMConfig


def build_model(cfg: LLMConfig) -> tuple[Model, OpenRouterModelSettings | None]:
    """Construct the correct PydanticAI model for the given LLM config.

    Returns (model, model_settings) — model_settings is non-None only for
    OpenRouter (to configure reasoning and timeout).

    Supported providers:
      - ollama      → OllamaProvider (no API key)
      - openrouter  → OpenRouterModel (dedicated, with reasoning disabled)
      - openai      → OpenAI API
      - gemini      → Google Gemini (OpenAI-compatible endpoint)
      - anthropic   → Anthropic Claude
      - groq        → Groq inference
      - deepseek    → DeepSeek
      - together    → Together AI
      - fireworks   → Fireworks AI
      - vllm        → self-hosted vLLM (OpenAI-compatible)
      - custom      → any OpenAI-compatible endpoint
    """
    provider_name = cfg.provider.lower().strip()
    model: Model

    if provider_name == "ollama":
        model = OpenAIChatModel(
            model_name=cfg.model,
            provider=OllamaProvider(base_url=f"{cfg.base_url}/v1"),
        )
        return model, None

    if provider_name == "openrouter":
        model = OpenRouterModel(
            model_name=cfg.model,
            provider=OpenRouterProvider(api_key=cfg.api_key or None),
        )
        settings = OpenRouterModelSettings(
            openrouter_reasoning={"enabled": True},
            timeout=120,
        )
        return model, settings

    if provider_name == "gemini":
        model = GoogleModel(
            model_name=cfg.model,
            provider=GoogleProvider(api_key=cfg.api_key),
        )
        return model, None

    # Named providers with known base URLs
    provider_base_urls: dict[str, str] = {
        "openai": "https://api.openai.com/v1",
        "anthropic": "https://api.anthropic.com/v1",
        "groq": "https://api.groq.com/openai/v1",
        "deepseek": "https://api.deepseek.com/v1",
        "together": "https://api.together.xyz/v1",
        "fireworks": "https://api.fireworks.ai/inference/v1",
    }

    if provider_name in provider_base_urls:
        base_url = provider_base_urls[provider_name]
    else:
        # vllm / custom — use the user-specified base_url
        base_url = cfg.base_url
        if not base_url.rstrip("/").endswith("/v1"):
            base_url = f"{base_url.rstrip('/')}/v1"

    model = OpenAIChatModel(
        model_name=cfg.model,
        provider=OpenAIProvider(
            base_url=base_url,
            api_key=cfg.api_key or "no-key",
        ),
    )
    return model, None
