"""Pydantic contracts for Auto E2E evaluation configs, runs, and events.

Phase 0 scope:
- Decision-complete request/response wire contracts.
- Event taxonomy for live/replay timelines.
- Deterministic sandbox and evaluator input/output schemas.
"""

from __future__ import annotations

import uuid
from datetime import datetime
from enum import StrEnum
from typing import Annotated, Any, Literal

from pydantic import BaseModel, Field, model_validator

# ── Enums ────────────────────────────────────────────────────────


class EvaluationConfigStatus(StrEnum):
    ACTIVE = "active"
    ARCHIVED = "archived"


class EvaluationRunStatus(StrEnum):
    QUEUED = "queued"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"


class PersonaPreset(StrEnum):
    COOPERATIVE = "cooperative"
    CONFUSED = "confused"
    IMPATIENT = "impatient"
    ADVERSARIAL = "adversarial"
    SILENT = "silent"


class ScenarioProfile(StrEnum):
    BALANCED = "balanced"
    HAPPY_PATH = "happy_path"
    FAILURE_HEAVY = "failure_heavy"


class ToolMockOutcome(StrEnum):
    SUCCESS = "success"
    EMPTY = "empty"
    PARTIAL = "partial"
    TIMEOUT = "timeout"
    HTTP_4XX = "http_4xx"
    HTTP_5XX = "http_5xx"
    MALFORMED = "malformed"


class EvaluationRunEventType(StrEnum):
    TURN_STARTED = "turn_started"
    CALLER_UTTERANCE = "caller_utterance"
    ASSISTANT_REPLY = "assistant_reply"
    TOOL_CALL = "tool_call"
    TOOL_MOCK_RESULT = "tool_mock_result"
    JUDGMENT = "judgment"
    RUN_FINISHED = "run_finished"
    RUN_FAILED = "run_failed"


# ── Config payload ───────────────────────────────────────────────


class GoalTarget(BaseModel):
    """A user-selected end goal for a test configuration."""

    id: str = Field(..., min_length=1, max_length=128)
    title: str = Field(..., min_length=1, max_length=255)
    description: str | None = None
    success_criteria: str | None = None


class EvaluationJudgeConfig(BaseModel):
    """LLM judge behavior for a run."""

    enabled: bool = True
    rubric_version: str = Field(default="support_quality")


class RubricDimensionResponse(BaseModel):
    key: str
    label: str


class RubricPresetResponse(BaseModel):
    id: str
    display_name: str
    description: str | None = None
    dimensions: list[RubricDimensionResponse] = Field(default_factory=list)


class RubricPresetListResponse(BaseModel):
    rubrics: list[RubricPresetResponse] = Field(default_factory=list)


class EvaluationConfigPayload(BaseModel):
    """Versioned, immutable evaluation config JSON payload."""

    persona_preset: PersonaPreset = PersonaPreset.COOPERATIVE
    persona_instructions: str | None = None
    scenario_profile: ScenarioProfile = ScenarioProfile.BALANCED
    goals: list[GoalTarget] = Field(default_factory=list)
    max_turns: int = Field(default=12, ge=1, le=100)
    timeout_seconds: int = Field(default=180, ge=10, le=3600)
    seed: int = Field(default=42, ge=0)
    run_count: int = Field(default=1, ge=1, le=20)
    judge: EvaluationJudgeConfig = Field(default_factory=EvaluationJudgeConfig)


class EvaluationConfigCreate(BaseModel):
    """Create a reusable evaluation config."""

    name: str = Field(..., min_length=1, max_length=255)
    config: EvaluationConfigPayload


class EvaluationConfigVersionCreate(BaseModel):
    """Create a new immutable version for an existing config."""

    config: EvaluationConfigPayload


class EvaluationConfigVersionResponse(BaseModel):
    id: uuid.UUID
    config_id: uuid.UUID
    version: int
    config: EvaluationConfigPayload
    created_at: datetime


class EvaluationConfigResponse(BaseModel):
    id: uuid.UUID
    agent_id: uuid.UUID
    name: str
    status: EvaluationConfigStatus
    latest_version: int
    created_at: datetime
    updated_at: datetime


class EvaluationConfigListResponse(BaseModel):
    configs: list[EvaluationConfigResponse]
    total: int


class EvaluationConfigDetailResponse(BaseModel):
    config: EvaluationConfigResponse
    versions: list[EvaluationConfigVersionResponse]


# ── Run requests/responses ───────────────────────────────────────


class EvaluationRunCreate(BaseModel):
    """Start a new run from an existing config/version."""

    config_version: int | None = Field(
        default=None,
        description="When omitted, uses the config's latest version",
    )


class EvaluationRunRerunRequest(BaseModel):
    """Exact rerun defaults to same config version + same seed.

    Optional `seed_override` intentionally breaks exactness to allow controlled
    variance without creating a new config version.
    """

    seed_override: int | None = Field(default=None, ge=0)


class EvaluationRunSummary(BaseModel):
    id: uuid.UUID
    agent_id: uuid.UUID
    config_id: uuid.UUID
    config_version: int
    target_agent_version: int | None
    status: EvaluationRunStatus
    aggregate_score: float | None = Field(default=None, ge=0, le=100)
    started_at: datetime | None
    ended_at: datetime | None
    created_at: datetime


class EvaluationRunListResponse(BaseModel):
    runs: list[EvaluationRunSummary]
    total: int


class EvaluationRunDetailResponse(BaseModel):
    run: EvaluationRunSummary
    hard_checks: dict[str, bool] = Field(default_factory=dict)
    rubric_scores: dict[str, float] = Field(default_factory=dict)
    summary: str | None = None


class EvaluationRunsDeleteResponse(BaseModel):
    deleted_count: int = Field(default=0, ge=0)
    skipped_active_count: int = Field(default=0, ge=0)


# ── Event taxonomy (live + replay) ──────────────────────────────


class _BaseRunEvent(BaseModel):
    seq_no: int = Field(..., ge=1)
    timestamp: datetime


class TurnStartedEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.TURN_STARTED]
    turn_id: int = Field(..., ge=1)


class CallerUtteranceEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.CALLER_UTTERANCE]
    turn_id: int = Field(..., ge=1)
    text: str


class AssistantReplyEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.ASSISTANT_REPLY]
    turn_id: int = Field(..., ge=1)
    text: str


class ToolCallEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.TOOL_CALL]
    turn_id: int = Field(..., ge=1)
    tool_call_id: str
    tool_id: str
    args_json: dict[str, Any] = Field(default_factory=dict)


class ToolMockResultEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.TOOL_MOCK_RESULT]
    turn_id: int = Field(..., ge=1)
    tool_call_id: str
    tool_id: str
    outcome: ToolMockOutcome
    status_code: int | None = None
    body_json: dict[str, Any] | None = None
    error_message: str | None = None
    decision_source: Literal["override", "rule", "profile"] | None = None
    hook_stage: Literal["before", "after"] | None = None


class JudgmentEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.JUDGMENT]
    hard_checks: dict[str, bool] = Field(default_factory=dict)
    rubric_scores: dict[str, float] = Field(default_factory=dict)
    notes: str | None = None


class RunFinishedEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.RUN_FINISHED]
    aggregate_score: float | None = Field(default=None, ge=0, le=100)


class RunFailedEvent(_BaseRunEvent):
    event_type: Literal[EvaluationRunEventType.RUN_FAILED]
    error_code: str | None = None
    error_message: str


EvaluationRunEvent = Annotated[
    (
        TurnStartedEvent
        | CallerUtteranceEvent
        | AssistantReplyEvent
        | ToolCallEvent
        | ToolMockResultEvent
        | JudgmentEvent
        | RunFinishedEvent
        | RunFailedEvent
    ),
    Field(discriminator="event_type"),
]


class EvaluationRunEventEnvelope(BaseModel):
    """SSE/replay wire envelope for events."""

    run_id: uuid.UUID
    event: EvaluationRunEvent


# ── Deterministic sandbox contracts ─────────────────────────────


class ToolSandboxOverride(BaseModel):
    """Per-turn forced outcome, highest precedence."""

    turn_id: int = Field(..., ge=1)
    tool_id: str = Field(..., min_length=1)
    outcome: ToolMockOutcome


class ToolSandboxRule(BaseModel):
    """Rule-based deterministic outcome for a tool.

    `condition` is a declarative expression string (parsed by runtime later).
    """

    tool_id: str = Field(..., min_length=1)
    condition: str = Field(..., min_length=1)
    outcome: ToolMockOutcome


class ToolSandboxProfileWeights(BaseModel):
    """Seeded fallback distribution by outcome type."""

    success: float = Field(default=0.6, ge=0)
    empty: float = Field(default=0.1, ge=0)
    partial: float = Field(default=0.1, ge=0)
    timeout: float = Field(default=0.05, ge=0)
    http_4xx: float = Field(default=0.1, ge=0)
    http_5xx: float = Field(default=0.04, ge=0)
    malformed: float = Field(default=0.01, ge=0)

    @model_validator(mode="after")
    def validate_total_weight(self) -> ToolSandboxProfileWeights:
        total = (
            self.success
            + self.empty
            + self.partial
            + self.timeout
            + self.http_4xx
            + self.http_5xx
            + self.malformed
        )
        if total <= 0:
            raise ValueError("ToolSandboxProfileWeights total weight must be > 0")
        return self


class ToolSandboxResolutionInput(BaseModel):
    """Input to deterministic sandbox resolver."""

    scenario_profile: ScenarioProfile
    seed: int = Field(..., ge=0)
    turn_id: int = Field(..., ge=1)
    tool_call_index: int = Field(..., ge=0)
    tool_id: str
    args_json: dict[str, Any] = Field(default_factory=dict)
    overrides: list[ToolSandboxOverride] = Field(default_factory=list)
    rules: list[ToolSandboxRule] = Field(default_factory=list)
    profile_weights: ToolSandboxProfileWeights = Field(
        default_factory=ToolSandboxProfileWeights
    )


class ToolSandboxResolutionResult(BaseModel):
    """Deterministic outcome emitted by sandbox resolver."""

    outcome: ToolMockOutcome
    status_code: int | None = None
    body_json: dict[str, Any] | None = None
    error_message: str | None = None
    source: Literal["override", "rule", "profile"]


# ── Evaluator contracts ─────────────────────────────────────────


class EvaluationJudgeRequest(BaseModel):
    """Input payload for LLM judge scoring."""

    run_id: uuid.UUID
    config: EvaluationConfigPayload
    transcript: list[dict[str, Any]] = Field(default_factory=list)
    tool_timeline: list[dict[str, Any]] = Field(default_factory=list)
    hard_check_results: dict[str, bool] = Field(default_factory=dict)


class EvaluationJudgeResponse(BaseModel):
    """Output payload from LLM judge scoring."""

    rubric_scores: dict[str, float] = Field(default_factory=dict)
    summary: str
    failure_highlights: list[str] = Field(default_factory=list)
    recommendations: list[str] = Field(default_factory=list)
