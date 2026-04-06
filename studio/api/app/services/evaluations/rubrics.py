"""Rubric preset registry for evaluation judging."""

from __future__ import annotations

from dataclasses import dataclass

from app.schemas.evaluation import RubricDimensionResponse, RubricPresetResponse


@dataclass(frozen=True, slots=True)
class RubricDimension:
    key: str
    label: str


@dataclass(frozen=True, slots=True)
class RubricPreset:
    id: str
    display_name: str
    description: str
    dimensions: tuple[RubricDimension, ...]


_RUBRICS: tuple[RubricPreset, ...] = (
    RubricPreset(
        id="support_quality",
        display_name="Support Quality",
        description="Balanced customer-support evaluation for service interactions.",
        dimensions=(
            RubricDimension("task_completion", "Task Completion"),
            RubricDimension("accuracy", "Accuracy"),
            RubricDimension("persona_fit", "Persona Fit"),
            RubricDimension("tool_usage", "Tool Usage"),
            RubricDimension("safety", "Safety"),
        ),
    ),
    RubricPreset(
        id="sales_readiness",
        display_name="Sales Readiness",
        description="Measures qualification and conversion-oriented call quality.",
        dimensions=(
            RubricDimension("intent_detection", "Intent Detection"),
            RubricDimension("value_delivery", "Value Delivery"),
            RubricDimension("objection_handling", "Objection Handling"),
            RubricDimension("next_step_clarity", "Next-Step Clarity"),
            RubricDimension("compliance", "Compliance"),
        ),
    ),
    RubricPreset(
        id="compliance_safety",
        display_name="Compliance and Safety",
        description="Prioritizes policy adherence and safe behavior under pressure.",
        dimensions=(
            RubricDimension("policy_adherence", "Policy Adherence"),
            RubricDimension("risk_handling", "Risk Handling"),
            RubricDimension("disclosure_clarity", "Disclosure Clarity"),
            RubricDimension("escalation_judgment", "Escalation Judgment"),
            RubricDimension("consistency", "Consistency"),
        ),
    ),
    RubricPreset(
        id="retention_recovery",
        display_name="Retention and Recovery",
        description="Evaluates churn-risk handling and trust rebuilding quality.",
        dimensions=(
            RubricDimension("empathy", "Empathy"),
            RubricDimension("root_cause_probing", "Root-Cause Probing"),
            RubricDimension("resolution_quality", "Resolution Quality"),
            RubricDimension("confidence_rebuild", "Confidence Rebuild"),
            RubricDimension("follow_up_commitment", "Follow-Up Commitment"),
        ),
    ),
)

_RUBRIC_BY_ID = {rubric.id: rubric for rubric in _RUBRICS}


def list_rubric_presets() -> list[RubricPreset]:
    return list(_RUBRICS)


def resolve_rubric_preset(rubric_id: str | None) -> RubricPreset:
    if rubric_id and rubric_id in _RUBRIC_BY_ID:
        return _RUBRIC_BY_ID[rubric_id]
    return _RUBRIC_BY_ID["support_quality"]


def rubric_to_response(rubric: RubricPreset) -> RubricPresetResponse:
    return RubricPresetResponse(
        id=rubric.id,
        display_name=rubric.display_name,
        description=rubric.description,
        dimensions=[
            RubricDimensionResponse(key=d.key, label=d.label) for d in rubric.dimensions
        ],
    )


def enforce_rubric_dimensions(
    rubric_id: str | None, rubric_scores: dict[str, float]
) -> dict[str, float]:
    """Return exactly the preset's 5 dimensions, preserving order."""

    preset = resolve_rubric_preset(rubric_id)
    normalized_input = {k.strip().lower(): float(v) for k, v in rubric_scores.items()}

    enforced: dict[str, float] = {}
    for dimension in preset.dimensions:
        key = dimension.key
        value = normalized_input.get(key, 0.0)
        enforced[key] = round(min(max(value, 0.0), 100.0), 2)
    return enforced
