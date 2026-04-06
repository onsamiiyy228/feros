"""Artifact tools — unified interface for builder LLM memory and uploaded files.

Four tools:
  - save_artifact(name, content)                — persist a text artifact (DB)
  - read_artifact(name, start_line?, end_line?)  — read text by line range,
                                                   or image as base64
  - search_artifact(name, query)                — keyword search in text
  - list_artifacts()                            — show all artifact names

Text artifacts are stored in the database (agent_artifacts table), scoped per
agent_id and always plain text.

User-uploaded files (managed by FileStore) are visible through the same tools:
read_artifact and search_artifact fall back to FileStore when the name is not
found in the database, so the LLM only needs one mental model.  Uploaded
images are served as base64 from FileStore; they are never written to the DB.
"""

import uuid
from typing import Any

from loguru import logger
from pydantic_ai import Agent, RunContext
from sqlalchemy import select

from app.agent_builder.deps import BuilderDeps
from app.lib.database import async_session
from app.lib.file_store import (
    MAX_READ_LINES,
    MAX_SEARCH_HITS,
    FileType,
    ReadResult,
    file_store,
)
from app.models.agent import AgentArtifact

MAX_ARTIFACT_SIZE = 100_000  # 100K characters


# ── DB helpers ───────────────────────────────────────────────────────────────


async def _upsert_artifact(agent_id: str, name: str, content: str) -> None:
    agent_uuid = uuid.UUID(agent_id)
    async with async_session() as db:
        stmt = select(AgentArtifact).where(
            AgentArtifact.agent_id == agent_uuid,
            AgentArtifact.name == name,
        )
        result = await db.execute(stmt)
        existing = result.scalar_one_or_none()

        if existing:
            existing.content = content
        else:
            db.add(
                AgentArtifact(
                    agent_id=agent_uuid,
                    name=name,
                    content=content,
                )
            )
        await db.commit()


async def _read_artifact(agent_id: str, name: str) -> str | None:
    """Read artifact content from the database.  Returns the plain string
    (not the ORM object) so callers never risk a DetachedInstanceError."""
    agent_uuid = uuid.UUID(agent_id)
    async with async_session() as db:
        stmt = select(AgentArtifact).where(
            AgentArtifact.agent_id == agent_uuid,
            AgentArtifact.name == name,
        )
        result = await db.execute(stmt)
        artifact = result.scalar_one_or_none()
        return artifact.content if artifact else None


async def _list_artifacts(agent_id: str) -> list[tuple[str, int]]:
    """Returns [(name, content_length)]."""
    agent_uuid = uuid.UUID(agent_id)
    async with async_session() as db:
        stmt = (
            select(AgentArtifact)
            .where(AgentArtifact.agent_id == agent_uuid)
            .order_by(AgentArtifact.name)
        )
        result = await db.execute(stmt)
        artifacts = result.scalars().all()
        return [(a.name, len(a.content)) for a in artifacts]


async def _not_found_message(agent_id: str, name: str) -> str:
    """Build a helpful not-found message listing all available artifacts and files."""
    available = await _list_artifacts(agent_id)
    names = [n for n, _ in available]
    uploaded = [f["file_id"] for f in file_store.list_files(agent_id)]
    all_names = sorted(set(names + uploaded))
    return (
        f"Artifact '{name}' not found. "
        f"Available: {', '.join(all_names) if all_names else 'none'}"
    )


# ── Tool registration ─────────────────────────────────────────────────────────


def register_artifact_tools(agent: Agent[BuilderDeps, Any]) -> None:
    """Register unified artifact tools on the builder agent."""

    @agent.tool
    async def save_artifact(
        ctx: RunContext[BuilderDeps], name: str, content: str
    ) -> str:
        """Save a named text artifact for this agent.

        Use this to persist important information across conversation turns:
        - Requirements and decisions ("requirements.md")
        - Task progress ("task_list.md")
        - API endpoints, auth patterns ("integrations.md")
        - Architecture decisions ("design_notes.md")

        The artifact persists even when conversation history is trimmed.
        Use descriptive names with .md or .json extensions.
        """
        agent_id: str = ctx.deps.agent_id
        if len(content) > MAX_ARTIFACT_SIZE:
            return (
                f"Artifact too large ({len(content)} chars, max {MAX_ARTIFACT_SIZE}). "
                "Summarize or split the content into smaller artifacts."
            )
        await _upsert_artifact(agent_id, name, content)
        logger.info("Builder saved artifact '{}' for agent {}", name, agent_id)
        return f"Artifact '{name}' saved ({len(content)} chars)."

    @agent.tool
    async def read_artifact(
        ctx: RunContext[BuilderDeps],
        name: str,
        start_line: int = 0,
        end_line: int = 0,
    ) -> str:
        """Read a saved artifact or user-uploaded file.

        For text: returns content, optionally limited to a line range
        (start_line / end_line, 1-indexed inclusive). Omit both for
        the first 800 lines.

        For uploaded images: returns base64 data (ignores line args).

        For user-uploaded files: pass the file_id shown in the upload
        message (works the same as a text artifact).

        Call list_artifacts() first if you're unsure what's available.
        """
        agent_id: str = ctx.deps.agent_id

        # ── Try DB first (DB takes precedence over FileStore on name collision) ──
        content = await _read_artifact(agent_id, name)
        if content is not None:
            logger.info("Builder read artifact '{}' for agent {}", name, agent_id)
            return _slice_text(content, start_line, end_line, name)

        # ── Fall back to FileStore (user-uploaded files) ─────────

        file_type = file_store.get_file_type(agent_id, name)
        if file_type is None:
            return await _not_found_message(agent_id, name)

        if file_type == FileType.IMAGE:
            raw = file_store.read_raw(agent_id, name)
            if raw is None:
                return f"File '{name}' is no longer available."
            return f"data:{raw.mime_type};base64,{raw.base64_data}"

        # Text file in FileStore — normalise the requested range, fetch it,
        # then format using _format_read_result which tracks the true total.
        s = max(1, start_line) if start_line > 0 else 1
        e = end_line if end_line > 0 else s + MAX_READ_LINES - 1
        entry = file_store.read_text_range(agent_id, name, s, e)
        if entry is None:
            return f"File '{name}' is no longer available. Ask the user to re-upload."
        return _format_read_result(entry)

    @agent.tool
    async def search_artifact(
        ctx: RunContext[BuilderDeps], name: str, query: str
    ) -> str:
        """Search a text artifact or uploaded file for a keyword.

        Returns matching lines with 3 lines of context around each match.
        Case-insensitive. Use this instead of reading the whole artifact
        when you only need specific content.
        """
        agent_id: str = ctx.deps.agent_id

        # ── Try DB artifact (DB takes precedence over FileStore on name collision) ──
        content = await _read_artifact(agent_id, name)
        if content is not None:
            return _search_text(content, name, query)

        # ── Fall back to FileStore ───────────────────────────────

        # Check type first: search is only meaningful on text files.
        file_type = file_store.get_file_type(agent_id, name)
        if file_type is None:
            return await _not_found_message(agent_id, name)
        if file_type == FileType.IMAGE:
            return (
                f"'{name}' is an image and cannot be searched. "
                f"Use read_artifact('{name}') to retrieve it as base64."
            )

        result = file_store.search_text(agent_id, name, query)
        if result is None:
            return await _not_found_message(agent_id, name)
        if not result.hits:
            return f"No matches for '{query}' in {result.filename} ({result.total_lines} lines)."

        lines: list[str] = [
            f"🔍 Found {result.total_hits} match(es) for '{query}' in {result.filename}:"
        ]
        if result.total_hits > len(result.hits):
            lines.append(f"(showing first {len(result.hits)} of {result.total_hits})")
        for hit in result.hits:
            lines.append(f"\n--- Line {hit.line_number} ---")
            for ctx_line in hit.context_before:
                lines.append(f"  {ctx_line}")
            lines.append(f"▶ L{hit.line_number}: {hit.line_content}")
            for ctx_line in hit.context_after:
                lines.append(f"  {ctx_line}")
        return "\n".join(lines)

    @agent.tool
    async def list_artifacts(ctx: RunContext[BuilderDeps]) -> str:
        """List all saved artifacts and uploaded files for this agent.

        Returns names, types, and sizes. Use read_artifact(name) to read
        the full content, or search_artifact(name, query) to find specific
        content in large files.
        """
        agent_id: str = ctx.deps.agent_id

        db_items = await _list_artifacts(agent_id)

        uploaded = file_store.list_files(agent_id)

        if not db_items and not uploaded:
            return "No artifacts or uploaded files yet."

        lines: list[str] = []
        for name, size in db_items:
            lines.append(f"- {name} ({size} chars) [artifact]")
        for f in uploaded:
            fid = f["file_id"]
            fname = f["filename"]
            ftype = f["file_type"]
            tag = "image" if ftype == "image" else f"{f['total_lines']} lines"
            lines.append(f"- {fid} ({fname}) [uploaded, {tag}]")
        return "\n".join(lines)


# ── Pure helper functions ─────────────────────────────────────────────────────


def _format_read_result(result: ReadResult) -> str:
    """Format a FileStore ReadResult into the same style as _slice_text.

    Uses result.total_lines (the true file total) so the 'N of M' figures
    are accurate even when only a range was fetched.
    """
    name = result.filename
    s, e, total = result.start_line, result.end_line, result.total_lines

    if total == 0 or not result.content:
        return f"📝 {name} — empty (0 lines)"

    header = f"📝 {name} — lines {s}–{e} of {total} total\n{'=' * 60}\n"
    footer = ""
    if e < total:
        footer = (
            f"\n{'=' * 60}\n"
            f"... {total - e} more lines. "
            f"Call read_artifact('{result.file_id}', start_line={e + 1}) to continue."
        )
    return header + result.content + footer


def _slice_text(content: str, start_line: int, end_line: int, name: str) -> str:
    """Apply an optional 1-indexed line range to text content."""
    lines = content.splitlines()
    total = len(lines)

    if total == 0:
        return f"📝 {name} — empty (0 lines)"

    if start_line <= 0 and end_line <= 0:
        # Default: first MAX_READ_LINES lines
        chunk = lines[:MAX_READ_LINES]
        s, e = 1, len(chunk)
    else:
        s = max(1, start_line) if start_line > 0 else 1
        e = min(end_line, total) if end_line > 0 else min(s + MAX_READ_LINES - 1, total)
        e = min(e, s + MAX_READ_LINES - 1)
        chunk = lines[s - 1 : e]

    header = f"📝 {name} — lines {s}–{e} of {total} total\n{'=' * 60}\n"
    text = "\n".join(chunk)
    footer = ""
    if e < total:
        footer = (
            f"\n{'=' * 60}\n"
            f"... {total - e} more lines. "
            f"Call read_artifact('{name}', start_line={e + 1}) to continue."
        )
    return header + text + footer


def _search_text(content: str, name: str, query: str) -> str:
    """Simple keyword search with 3 lines of context."""
    if not query.strip():
        return "Query must not be empty."

    lines = content.splitlines()
    q = query.lower()
    hits: list[tuple[int, str]] = [
        (i + 1, line) for i, line in enumerate(lines) if q in line.lower()
    ]

    if not hits:
        return f"No matches for '{query}' in {name} ({len(lines)} lines)."

    ctx = 3
    out: list[str] = [f"🔍 Found {len(hits)} match(es) for '{query}' in {name}:"]
    for lineno, line_content in hits[:MAX_SEARCH_HITS]:
        out.append(f"\n--- Line {lineno} ---")
        for cl in lines[max(0, lineno - 1 - ctx) : lineno - 1]:
            out.append(f"  {cl}")
        out.append(f"▶ L{lineno}: {line_content}")
        for cl in lines[lineno : min(len(lines), lineno + ctx)]:
            out.append(f"  {cl}")
    if len(hits) > MAX_SEARCH_HITS:
        out.append(f"\n(showing first {MAX_SEARCH_HITS} of {len(hits)} matches)")
    return "\n".join(out)
