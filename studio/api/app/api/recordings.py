"""Recording URI resolution utilities.

The ``calls.recording_url`` column stores a **canonical storage URI** written
by voice-server:

| URI scheme         | What it means                                 |
|--------------------|-----------------------------------------------|
| ``file:///abs``    | Local filesystem path (bare metal or volume). |
| ``s3://bucket/key``| Object in an S3-compatible store.             |

This module provides ``resolve_recording_http_url`` — a pure function that
translates a stored URI into an HTTP URL the frontend can consume.
The actual file-serving endpoint lives in ``calls.py`` (``GET /calls/{id}/recording``)
to keep it co-located with the rest of the call resources.
"""

from __future__ import annotations

import uuid
from urllib.parse import urlparse


def resolve_recording_http_url(call_id: uuid.UUID, uri: str | None) -> str | None:
    """Translate a canonical storage URI into an HTTP URL for the frontend.

    Called at query time when building ``CallResponse`` objects — the raw URI
    is never sent to the frontend.

    Scheme mapping:

    - ``file://…``   → ``/api/calls/{call_id}/recording``  (proxied by the API)
    - ``s3://…``     → pre-signed URL  (*not yet implemented — returns None*)
    - ``http(s)://`` → returned as-is  (legacy absolute URLs from older releases)
    - ``None``       → ``None``
    """
    if not uri:
        return None

    parsed = urlparse(uri)

    if parsed.scheme == "file":
        # Route through the proxy endpoint — browsers cannot access local paths.
        return f"/api/calls/{call_id}/recording"

    if parsed.scheme == "s3":
        # TODO: generate a pre-signed URL using aiobotocore / boto3.
        # Until implemented, return None so the UI shows no player rather than
        # exposing a raw s3:// URI.
        return None

    if parsed.scheme in ("http", "https"):
        # Legacy: absolute HTTP URL stored by an older voice-server release.
        return uri

    return None
