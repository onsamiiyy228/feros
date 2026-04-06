"""Builder package — the vibe-code engine.

Re-exports for backward-compatible imports:
    from app.agent_builder import builder_service, BuilderResult
"""

from app.agent_builder.deps import (
    BuilderDeps,
    BuilderResult,
)
from app.agent_builder.graph import (
    generate_graph_mermaid,
    generate_graph_mermaid_llm,
    validate_graph,
)
from app.agent_builder.service import (
    BuilderService,
    builder_service,
)

__all__ = [
    "BuilderDeps",
    "BuilderResult",
    "BuilderService",
    "builder_service",
    "generate_graph_mermaid",
    "generate_graph_mermaid_llm",
    "validate_graph",
]
