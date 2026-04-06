from __future__ import annotations

import sys
from types import MethodType, ModuleType, SimpleNamespace
from typing import Any

import pytest

from app.agent_builder.service import (
    BuilderDeps,
    BuilderResult,
    BuilderService,
    _normalize_escaped_tool_scripts,
)


@pytest.mark.asyncio
async def test_edit_path_correction_uses_failed_candidate_as_base(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original_config = {
        "entry": "appointment_assistant",
        "nodes": {
            "appointment_assistant": {
                "system_prompt": "Book appointments.",
                "tools": [
                    "google_calendar_check_availability",
                    "google_calendar_create_event",
                ],
                "edges": [],
            }
        },
        "tools": {
            "google_calendar_check_availability": {
                "description": "Check availability",
                "params": [],
                "script": "return { result: 'ok' };",
                "side_effect": False,
            },
            "google_calendar_create_event": {
                "description": "Create event",
                "params": [],
                "script": "return { result: 'ok' };",
                "side_effect": True,
            },
        },
    }
    failed_candidate = {
        "entry": "appointment_assistant",
        "nodes": {
            "appointment_assistant": {
                "system_prompt": "Book appointments and notify webhook.",
                "tools": [
                    "google_calendar_check_availability",
                    "google_calendar_create_event",
                    "send_to_make_webhook",
                ],
                "edges": [],
            }
        },
        "tools": {
            **original_config["tools"],
            "send_to_make_webhook": {
                "description": "Notify Make.com",
                "params": [{"name": "name", "type": "string", "required": True}],
                "script": (
                    "let key = secret('custom_webhook');\n"
                    "return http_post_h('https://hooks.example.com', {name: name}, "
                    "{'api-key': key});"
                ),
                "side_effect": True,
            },
        },
    }
    corrected_candidate = {
        "entry": "appointment_assistant",
        "nodes": {
            "appointment_assistant": {
                "system_prompt": "Book appointments and notify webhook.",
                "tools": [
                    "google_calendar_check_availability",
                    "google_calendar_create_event",
                    "send_to_make_webhook",
                ],
                "edges": [],
            }
        },
        "tools": {
            **original_config["tools"],
            "send_to_make_webhook": {
                "description": "Notify Make.com",
                "params": [{"name": "name", "type": "string", "required": True}],
                "script": (
                    "let key = secret('custom_webhook');\n"
                    "let header = secret('custom_webhook.header_name');\n"
                    "let headers = {'Content-Type': 'application/json'};\n"
                    "headers[header] = key;\n"
                    "return http_post_h('https://hooks.example.com', {name: name}, headers);"
                ),
                "side_effect": True,
            },
        },
    }

    service = BuilderService.__new__(BuilderService)
    service._compression_model = object()
    service._model_settings = None
    service.stream_agent = SimpleNamespace(model="test-model")

    observed_bases: list[dict[str, Any] | None] = []
    deps_holder: dict[str, BuilderDeps] = {}

    async def fake_iter_agent_events(
        self: BuilderService,
        user_prompt: str,
        deps: BuilderDeps,
        message_history: list[Any],
    ):
        del user_prompt, message_history
        deps_holder["deps"] = deps
        observed_bases.append(deps.current_config)
        if len(observed_bases) == 1:
            deps.emitted_config = failed_candidate
            deps.emitted_change_summary = "initial webhook edit"
            deps.used_edit_path = True
        else:
            if deps.current_config == failed_candidate:
                deps.emitted_config = corrected_candidate
            else:
                deps.emitted_config = original_config
            deps.emitted_change_summary = "correction"
            deps.used_edit_path = True
        if False:
            yield {}

    async def fake_ensure_action_cards(
        self: BuilderService, result: BuilderResult, agent_id: str = ""
    ) -> BuilderResult:
        del self, agent_id
        return result

    def fake_ensure_side_effects(
        self: BuilderService, result: BuilderResult
    ) -> BuilderResult:
        del self
        return result

    def fake_collect_all_errors(self: BuilderService, cfg: dict[str, Any]) -> list[str]:
        del self
        if cfg == failed_candidate:
            return ["webhook header is hardcoded"]
        if cfg == corrected_candidate:
            return []
        return ["correction lost webhook changes"]

    async def fake_compress_history(model: Any, history: list[Any]) -> list[Any]:
        del model
        return history

    async def fake_generate_graph_mermaid_llm(
        cfg: dict[str, Any],
        model: Any,
        previous_mermaid: str | None = None,
        change_summary: str | None = None,
    ) -> str:
        del cfg, model, previous_mermaid, change_summary
        return "graph TD"

    async def fake_get_connection_status(
        agent_id: str, current_config: dict[str, Any] | None
    ) -> str:
        del agent_id, current_config
        return "No integrations checked."

    monkeypatch.setattr(
        service, "_iter_agent_events", MethodType(fake_iter_agent_events, service)
    )
    monkeypatch.setattr(
        service, "_ensure_action_cards", MethodType(fake_ensure_action_cards, service)
    )
    monkeypatch.setattr(
        service, "_ensure_side_effects", MethodType(fake_ensure_side_effects, service)
    )
    monkeypatch.setattr(
        service, "_collect_all_errors", MethodType(fake_collect_all_errors, service)
    )
    monkeypatch.setattr(
        "app.agent_builder.service.compress_history", fake_compress_history
    )
    monkeypatch.setattr(
        "app.agent_builder.service.generate_graph_mermaid_llm",
        fake_generate_graph_mermaid_llm,
    )
    monkeypatch.setattr(
        "app.agent_builder.tools.connections.get_connection_status",
        fake_get_connection_status,
    )

    outputs: list[dict[str, Any] | BuilderResult] = []
    async for item in service.process_message_stream(
        user_message="Add webhook after booking",
        current_config=original_config,
        agent_name="Test Agent",
        agent_id="test-agent",
    ):
        outputs.append(item)

    final = outputs[-1]
    assert isinstance(final, BuilderResult)
    assert final.config is not None
    assert final.config["nodes"]["appointment_assistant"]["tools"] == [
        "google_calendar_check_availability",
        "google_calendar_create_event",
        "send_to_make_webhook",
    ]
    assert (
        final.config["tools"]["send_to_make_webhook"]["script"]
        == corrected_candidate["tools"]["send_to_make_webhook"]["script"]
    )
    assert observed_bases == [original_config, failed_candidate]
    assert deps_holder["deps"].current_config == original_config


def test_normalize_escaped_tool_scripts_decodes_literal_newlines_when_decoded_script_valid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    voice_engine = ModuleType("voice_engine")

    def fake_validate_javascript(script: str) -> list[str]:
        return ["JS Syntax Error"] if "\\n" in script else []

    voice_engine.validate_javascript = fake_validate_javascript
    monkeypatch.setitem(sys.modules, "voice_engine", voice_engine)

    cfg = {
        "tools": {
            "post_to_make_webhook": {
                "description": "Notify webhook",
                "params": [],
                "script": (
                    "let resp = http_post('https://hooks.example.com', { name: patient_name });\\n"
                    "if (resp.status >= 200 && resp.status < 300) {\\n"
                    "  return { result: 'ok' };\\n"
                    "} else {\\n"
                    "  return { error: 'bad' };\\n"
                    "}"
                ),
                "side_effect": True,
            }
        }
    }

    normalized = _normalize_escaped_tool_scripts(cfg)

    script = normalized["tools"]["post_to_make_webhook"]["script"]
    assert "\\n" not in script
    assert "\n" in script
