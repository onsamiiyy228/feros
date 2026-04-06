from __future__ import annotations

from app.services.evaluations.rubrics import (
    enforce_rubric_dimensions,
    list_rubric_presets,
    resolve_rubric_preset,
)


def test_rubric_presets_exist_and_have_five_dimensions() -> None:
    presets = list_rubric_presets()
    assert len(presets) >= 3
    for preset in presets:
        assert len(preset.dimensions) == 5


def test_resolve_unknown_rubric_falls_back_to_support_quality() -> None:
    preset = resolve_rubric_preset("unknown_rubric")
    assert preset.id == "support_quality"


def test_enforce_rubric_dimensions_returns_exact_dimension_set() -> None:
    scores = {
        "intent_detection": 80,
        "value_delivery": 72,
        "unexpected_key": 99,
    }
    enforced = enforce_rubric_dimensions("sales_readiness", scores)
    assert list(enforced.keys()) == [
        "intent_detection",
        "value_delivery",
        "objection_handling",
        "next_step_clarity",
        "compliance",
    ]
    assert enforced["intent_detection"] == 80.0
    assert enforced["value_delivery"] == 72.0
    assert enforced["objection_handling"] == 0.0
