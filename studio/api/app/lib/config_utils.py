"""Shared config-scanning utilities.

Provides a single source of truth for extracting ``secret("provider")``
references from agent config dictionaries.
"""

import re
from typing import Any

# Matches  secret("provider_name")  or  secret('provider_name')
_SECRET_RE = re.compile(r'secret\(["\']([^"\']+)["\']\)')


def extract_secret_keys(config: dict[str, Any]) -> set[str]:
    """Scan all tool scripts in a config for ``secret("key")`` calls.

    Checks both top-level ``tools`` and per-node ``tools`` inside ``nodes``
    (multi-node / graph agents).

    Returns the set of secret key names found.
    """
    keys: set[str] = set()

    # Scan top-level tools
    tools = config.get("tools", {})
    if isinstance(tools, dict):
        for tool_def in tools.values():
            script = (
                str(tool_def.get("script", "")) if isinstance(tool_def, dict) else ""
            )
            for match in _SECRET_RE.finditer(script):
                keys.add(match.group(1))

    # Scan tools inside graph nodes (multi-node agents)
    nodes = config.get("nodes", {})
    if isinstance(nodes, dict):
        for node in nodes.values():
            if not isinstance(node, dict):
                continue
            node_tools = node.get("tools", {})
            if isinstance(node_tools, dict):
                for tool_def in node_tools.values():
                    script = (
                        str(tool_def.get("script", ""))
                        if isinstance(tool_def, dict)
                        else ""
                    )
                    for match in _SECRET_RE.finditer(script):
                        keys.add(match.group(1))

    return keys
