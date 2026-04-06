"""Tool-mock profile defaults for deterministic sandbox resolution."""

from __future__ import annotations

from app.schemas.evaluation import ScenarioProfile, ToolSandboxProfileWeights


def default_profile_weights(profile: ScenarioProfile) -> ToolSandboxProfileWeights:
    if profile == ScenarioProfile.HAPPY_PATH:
        return ToolSandboxProfileWeights(
            success=0.9,
            empty=0.04,
            partial=0.03,
            timeout=0.01,
            http_4xx=0.01,
            http_5xx=0.005,
            malformed=0.005,
        )
    if profile == ScenarioProfile.FAILURE_HEAVY:
        return ToolSandboxProfileWeights(
            success=0.25,
            empty=0.1,
            partial=0.15,
            timeout=0.2,
            http_4xx=0.15,
            http_5xx=0.1,
            malformed=0.05,
        )
    return ToolSandboxProfileWeights()
