"""Lightweight file store — text navigation and binary file storage.

Disk-backed with in-memory TTL cache:
  - Text files: content saved to ``{base_dir}/{agent_id}/{file_id}/content.txt``
  - Binary files: raw bytes saved to ``{base_dir}/{agent_id}/{file_id}/content.bin``
  - Lines/bytes are loaded into memory only on access, evicted after *ttl* seconds
  - Disk files are cleaned up on explicit removal or via ``cleanup_expired``

Provides operations for LLM tool-calling:

  Text files:
    1. read_doc(file_id)                — first 800 lines + total line count
    2. search_doc(file_id, kw)          — keyword search with surrounding context
    3. read_doc_range(file_id, start, end) — read lines by range

  Binary files (images, etc.):
    4. read_raw(file_id)                — returns base64-encoded content

No embeddings, no vector DB, no ML models. Just files + smart navigation.
"""

from __future__ import annotations

import base64
import io
import json
import os
import re
import shutil
import threading
import time
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any

from loguru import logger

# ── Optional dependency feature flags ────────────────────────────

try:
    import fitz as _fitz  # type: ignore[import-untyped]

    _HAS_PDF = True
except ImportError:
    _HAS_PDF = False

try:
    from docx import Document as _DocxDocument

    _HAS_DOCX = True
except ImportError:
    _HAS_DOCX = False


# ── File type enum ───────────────────────────────────────────────


class FileType(StrEnum):
    """Type of stored file — determines which read operations are valid."""

    TEXT = "text"  # line-based navigation (read_doc, search_doc, read_doc_range)
    IMAGE = "image"  # binary blob (read_raw)


# ── Data types ───────────────────────────────────────────────────


@dataclass
class SearchHit:
    """A single keyword match with surrounding context."""

    line_number: int  # 1-indexed
    line_content: str
    context_before: list[str]  # preceding lines
    context_after: list[str]  # following lines


@dataclass
class ReadResult:
    """Result of reading a text file (first N lines or a range)."""

    file_id: str
    filename: str
    total_lines: int
    start_line: int  # 1-indexed
    end_line: int  # 1-indexed, inclusive
    content: str  # the actual text


@dataclass
class RawReadResult:
    """Result of reading a binary file (image, etc.)."""

    file_id: str
    filename: str
    mime_type: str
    size_bytes: int
    base64_data: str  # base64-encoded content


@dataclass
class SearchResult:
    """Result of searching a text file for keywords."""

    file_id: str
    filename: str
    total_lines: int
    query: str
    hits: list[SearchHit]
    total_hits: int  # may be more than len(hits) if capped


# ── Internal cache entries ───────────────────────────────────────

_DEFAULT_BASE_DIR = "/tmp/studio_docs"
_DEFAULT_TTL = 30 * 60  # 30 minutes

# Only allow safe characters in agent_id and file_id.
# Prevents path traversal (../, /, \) and null bytes.
_SAFE_ID_RE = re.compile(r"^[a-zA-Z0-9_.-]+$")


def _safe_id(value: str, label: str = "id") -> bool:
    """Validate that an ID is safe for use in file paths.

    Rejects empty strings, strings containing path separators,
    parent-dir references, and any non-alphanumeric characters
    (except underscore, hyphen, dot).
    """
    if not value or not _SAFE_ID_RE.match(value):
        logger.warning("Rejected unsafe {}: {!r}", label, value)
        return False
    # Extra guard: reject reserved names
    if value in (".", ".."):
        logger.warning("Rejected reserved {}: {!r}", label, value)
        return False
    return True


@dataclass
class _TextCacheEntry:
    """In-memory cache of a text file's lines + metadata."""

    file_id: str
    filename: str
    lines: list[str]
    last_accessed: float  # time.monotonic()

    @property
    def total_lines(self) -> int:
        return len(self.lines)

    def touch(self) -> None:
        """Update last-accessed timestamp."""
        self.last_accessed = time.monotonic()


@dataclass
class _BinaryCacheEntry:
    """In-memory cache of a binary file's raw bytes + metadata."""

    file_id: str
    filename: str
    data: bytes
    mime_type: str
    last_accessed: float  # time.monotonic()

    @property
    def size_bytes(self) -> int:
        return len(self.data)

    def touch(self) -> None:
        """Update last-accessed timestamp."""
        self.last_accessed = time.monotonic()


# Union of possible cache entries
_CacheEntry = _TextCacheEntry | _BinaryCacheEntry


@dataclass
class _FileMeta:
    """On-disk metadata for a stored file (stored as meta.json)."""

    file_id: str
    filename: str
    file_type: FileType
    total_lines: int  # 0 for binary files
    mime_type: str  # e.g. "text/plain", "image/png"
    created_at: float  # time.time() (wall clock for disk expiry)


# ── Mime type detection ──────────────────────────────────────────

# Extensions that are treated as text (extracted to plain text)
TEXT_EXTENSIONS = {".txt", ".md", ".pdf", ".docx"}

# Extensions that are treated as images (stored as binary)
IMAGE_EXTENSIONS = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"}

_EXT_TO_MIME: dict[str, str] = {
    ".txt": "text/plain",
    ".md": "text/markdown",
    ".pdf": "text/plain",  # after extraction
    ".docx": "text/plain",  # after extraction
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".svg": "image/svg+xml",
}


def _detect_file_type(filename: str) -> FileType:
    """Detect file type from extension."""
    ext = Path(filename).suffix.lower()
    if ext in IMAGE_EXTENSIONS:
        return FileType.IMAGE
    return FileType.TEXT


def _detect_mime_type(filename: str, file_type: FileType) -> str:
    """Detect MIME type from extension and file type."""
    ext = Path(filename).suffix.lower()
    if ext in _EXT_TO_MIME:
        return _EXT_TO_MIME[ext]
    if file_type == FileType.IMAGE:
        return "application/octet-stream"
    return "text/plain"


# ── File Store ───────────────────────────────────────────────────

MAX_READ_LINES = 800
MAX_SEARCH_HITS = 30
CONTEXT_LINES = 3  # lines before/after each search hit


class FileStore:
    """Disk-backed file store with in-memory TTL cache.

    Supports two file types:
      - TEXT: stored as plain text, supports line-range reading and search
      - IMAGE: stored as binary, supports raw/base64 reading

    Files are saved to disk on add (``{base_dir}/{agent_id}/{file_id}/``).
    Content is loaded into an in-memory cache on access and evicted
    after ``ttl`` seconds of inactivity.

    Thread-safe via a simple lock on the cache dict.
    """

    def __init__(
        self,
        base_dir: str = _DEFAULT_BASE_DIR,
        ttl: int = _DEFAULT_TTL,
    ) -> None:
        self._base_dir = Path(base_dir)
        self._ttl = ttl
        # {agent_id: {file_id: _CacheEntry}}
        self._cache: dict[str, dict[str, _CacheEntry]] = {}
        self._lock = threading.Lock()

    # ── Disk paths ───────────────────────────────────────────────

    def _file_dir(self, agent_id: str, file_id: str) -> Path:
        return self._base_dir / agent_id / file_id

    def _text_path(self, agent_id: str, file_id: str) -> Path:
        return self._file_dir(agent_id, file_id) / "content.txt"

    def _binary_path(self, agent_id: str, file_id: str) -> Path:
        return self._file_dir(agent_id, file_id) / "content.bin"

    def _meta_path(self, agent_id: str, file_id: str) -> Path:
        return self._file_dir(agent_id, file_id) / "meta.json"

    # ── Disk I/O ─────────────────────────────────────────────────

    def _set_permissions(self, agent_id: str) -> None:
        """Restrict directory permissions: only the server process can read/write."""
        os.chmod(self._base_dir, 0o700)
        agent_dir = self._base_dir / agent_id
        if agent_dir.exists():
            os.chmod(agent_dir, 0o700)

    def _write_text_to_disk(
        self, agent_id: str, file_id: str, filename: str, text: str
    ) -> _FileMeta:
        """Write text content and metadata to disk."""
        file_dir = self._file_dir(agent_id, file_id)
        file_dir.mkdir(parents=True, exist_ok=True)
        self._set_permissions(agent_id)

        self._text_path(agent_id, file_id).write_text(text, encoding="utf-8")

        total_lines = len(text.splitlines())
        mime_type = _detect_mime_type(filename, FileType.TEXT)
        meta = _FileMeta(
            file_id=file_id,
            filename=filename,
            file_type=FileType.TEXT,
            total_lines=total_lines,
            mime_type=mime_type,
            created_at=time.time(),
        )
        self._write_meta(agent_id, file_id, meta)
        return meta

    def _write_binary_to_disk(
        self,
        agent_id: str,
        file_id: str,
        filename: str,
        data: bytes,
        mime_type: str,
    ) -> _FileMeta:
        """Write binary content and metadata to disk."""
        file_dir = self._file_dir(agent_id, file_id)
        file_dir.mkdir(parents=True, exist_ok=True)
        self._set_permissions(agent_id)

        self._binary_path(agent_id, file_id).write_bytes(data)

        meta = _FileMeta(
            file_id=file_id,
            filename=filename,
            file_type=FileType.IMAGE,
            total_lines=0,
            mime_type=mime_type,
            created_at=time.time(),
        )
        self._write_meta(agent_id, file_id, meta)
        return meta

    def _write_meta(self, agent_id: str, file_id: str, meta: _FileMeta) -> None:
        """Write metadata JSON to disk."""
        self._meta_path(agent_id, file_id).write_text(
            json.dumps(
                {
                    "file_id": meta.file_id,
                    "filename": meta.filename,
                    "file_type": meta.file_type.value,
                    "total_lines": meta.total_lines,
                    "mime_type": meta.mime_type,
                    "created_at": meta.created_at,
                }
            ),
            encoding="utf-8",
        )

    def _read_meta(self, agent_id: str, file_id: str) -> _FileMeta | None:
        """Read metadata from disk, or None if not found."""
        meta_path = self._meta_path(agent_id, file_id)
        if not meta_path.exists():
            return None
        try:
            data = json.loads(meta_path.read_text(encoding="utf-8"))
            # Backward compat: old meta.json may lack file_type/mime_type
            file_type_str = data.get("file_type", "text")
            return _FileMeta(
                file_id=data.get("file_id", data.get("doc_id", file_id)),
                filename=data["filename"],
                file_type=FileType(file_type_str),
                total_lines=data.get("total_lines", 0),
                mime_type=data.get("mime_type", "text/plain"),
                created_at=data["created_at"],
            )
        except (json.JSONDecodeError, KeyError, OSError, ValueError):
            logger.warning("Corrupt metadata for agent={} file={}", agent_id, file_id)
            return None

    def _load_lines(self, agent_id: str, file_id: str) -> list[str] | None:
        """Load lines from disk text file."""
        text_path = self._text_path(agent_id, file_id)
        if not text_path.exists():
            return None
        try:
            text = text_path.read_text(encoding="utf-8")
            return text.splitlines()
        except OSError:
            logger.warning(
                "Failed to read text for agent={} file={}", agent_id, file_id
            )
            return None

    def _load_binary(self, agent_id: str, file_id: str) -> bytes | None:
        """Load raw bytes from disk binary file."""
        bin_path = self._binary_path(agent_id, file_id)
        if not bin_path.exists():
            return None
        try:
            return bin_path.read_bytes()
        except OSError:
            logger.warning(
                "Failed to read binary for agent={} file={}", agent_id, file_id
            )
            return None

    # ── Cache management ─────────────────────────────────────────

    def _get_cached(self, agent_id: str, file_id: str) -> _CacheEntry | None:
        """Get a file from cache, loading from disk if needed. Returns None if not found."""
        with self._lock:
            agent_cache = self._cache.get(agent_id, {})
            entry = agent_cache.get(file_id)

            if entry is not None:
                entry.touch()
                return entry

        # Not in cache — try loading from disk (outside lock)
        meta = self._read_meta(agent_id, file_id)
        if meta is None:
            return None

        loaded_entry: _CacheEntry
        if meta.file_type == FileType.TEXT:
            lines = self._load_lines(agent_id, file_id)
            if lines is None:
                return None
            loaded_entry = _TextCacheEntry(
                file_id=file_id,
                filename=meta.filename,
                lines=lines,
                last_accessed=time.monotonic(),
            )
        else:
            data = self._load_binary(agent_id, file_id)
            if data is None:
                return None
            loaded_entry = _BinaryCacheEntry(
                file_id=file_id,
                filename=meta.filename,
                data=data,
                mime_type=meta.mime_type,
                last_accessed=time.monotonic(),
            )

        with self._lock:
            self._cache.setdefault(agent_id, {})[file_id] = loaded_entry

        return loaded_entry

    def _get_text_entry(self, agent_id: str, file_id: str) -> _TextCacheEntry | None:
        """Get a text file from cache. Returns None if not found or wrong type."""
        entry = self._get_cached(agent_id, file_id)
        if isinstance(entry, _TextCacheEntry):
            return entry
        return None

    def _get_binary_entry(
        self, agent_id: str, file_id: str
    ) -> _BinaryCacheEntry | None:
        """Get a binary file from cache. Returns None if not found or wrong type."""
        entry = self._get_cached(agent_id, file_id)
        if isinstance(entry, _BinaryCacheEntry):
            return entry
        return None

    def evict_expired(self) -> int:
        """Evict expired entries from the in-memory cache.

        Returns:
            Number of entries evicted.
        """
        now = time.monotonic()
        evicted = 0

        with self._lock:
            for agent_id in list(self._cache):
                agent_cache = self._cache[agent_id]
                expired = [
                    file_id
                    for file_id, entry in agent_cache.items()
                    if now - entry.last_accessed > self._ttl
                ]
                for file_id in expired:
                    del agent_cache[file_id]
                    evicted += 1
                if not agent_cache:
                    del self._cache[agent_id]

        if evicted:
            logger.info("Evicted {} expired file cache entries", evicted)
        return evicted

    def cleanup_expired_disk(self, max_age_seconds: int = 3600) -> int:
        """Remove files from disk that are older than max_age_seconds.

        Args:
            max_age_seconds: Maximum age for disk files (default 1 hour).

        Returns:
            Number of file directories removed.
        """
        if not self._base_dir.exists():
            return 0

        now = time.time()
        removed = 0

        for agent_dir in self._base_dir.iterdir():
            if not agent_dir.is_dir():
                continue
            for file_dir in agent_dir.iterdir():
                if not file_dir.is_dir():
                    continue
                meta_path = file_dir / "meta.json"
                try:
                    if meta_path.exists():
                        data = json.loads(meta_path.read_text(encoding="utf-8"))
                        created_at = data.get("created_at", 0)
                        if now - created_at > max_age_seconds:
                            shutil.rmtree(file_dir, ignore_errors=True)
                            removed += 1
                    else:
                        # No metadata — orphan, remove
                        shutil.rmtree(file_dir, ignore_errors=True)
                        removed += 1
                except (json.JSONDecodeError, OSError):
                    shutil.rmtree(file_dir, ignore_errors=True)
                    removed += 1

            # Remove empty agent dirs
            try:
                if agent_dir.exists() and not any(agent_dir.iterdir()):
                    agent_dir.rmdir()
            except OSError:
                pass

        if removed:
            logger.info("Cleaned up {} expired file directories from disk", removed)
        return removed

    # ── Public API ───────────────────────────────────────────────

    def add_text_file(
        self,
        agent_id: str,
        file_id: str,
        filename: str,
        text: str,
    ) -> _FileMeta:
        """Save extracted text to disk and cache it.

        Args:
            agent_id: Agent/session this file belongs to.
            file_id: Unique file identifier.
            filename: Original filename.
            text: Extracted plain text content.

        Returns:
            File metadata with line count.

        Raises:
            ValueError: If agent_id or file_id contain unsafe characters.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            raise ValueError("Invalid agent_id or file_id")

        meta = self._write_text_to_disk(agent_id, file_id, filename, text)

        # Populate the cache immediately
        lines = text.splitlines()
        entry = _TextCacheEntry(
            file_id=file_id,
            filename=filename,
            lines=lines,
            last_accessed=time.monotonic(),
        )
        with self._lock:
            self._cache.setdefault(agent_id, {})[file_id] = entry

        logger.info(
            "Stored text file '{}' ({} lines) for agent {}",
            filename,
            meta.total_lines,
            agent_id,
        )
        return meta

    def add_binary_file(
        self,
        agent_id: str,
        file_id: str,
        filename: str,
        data: bytes,
        mime_type: str | None = None,
    ) -> _FileMeta:
        """Save binary file (image, etc.) to disk and cache it.

        Args:
            agent_id: Agent/session this file belongs to.
            file_id: Unique file identifier.
            filename: Original filename.
            data: Raw binary content.
            mime_type: MIME type (auto-detected from extension if None).

        Returns:
            File metadata.

        Raises:
            ValueError: If agent_id or file_id contain unsafe characters.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            raise ValueError("Invalid agent_id or file_id")

        resolved_mime = mime_type or _detect_mime_type(filename, FileType.IMAGE)
        meta = self._write_binary_to_disk(
            agent_id,
            file_id,
            filename,
            data,
            resolved_mime,
        )

        # Populate the cache immediately
        entry = _BinaryCacheEntry(
            file_id=file_id,
            filename=filename,
            data=data,
            mime_type=resolved_mime,
            last_accessed=time.monotonic(),
        )
        with self._lock:
            self._cache.setdefault(agent_id, {})[file_id] = entry

        logger.info(
            "Stored binary file '{}' ({} bytes, {}) for agent {}",
            filename,
            len(data),
            resolved_mime,
            agent_id,
        )
        return meta

    def get_file_type(self, agent_id: str, file_id: str) -> FileType | None:
        """Get the type of a stored file, or None if not found."""
        entry = self._get_cached(agent_id, file_id)
        if isinstance(entry, _TextCacheEntry):
            return FileType.TEXT
        if isinstance(entry, _BinaryCacheEntry):
            return FileType.IMAGE
        return None

    def list_files(self, agent_id: str) -> list[dict[str, Any]]:
        """List all files for an agent (reads from disk).

        Returns:
            List of {file_id, filename, file_type, total_lines, mime_type} dicts.
        """
        if not _safe_id(agent_id, "agent_id"):
            return []

        agent_dir = self._base_dir / agent_id
        if not agent_dir.exists():
            return []

        results: list[dict[str, Any]] = []
        for file_dir in sorted(agent_dir.iterdir()):
            if not file_dir.is_dir():
                continue
            meta = self._read_meta(agent_id, file_dir.name)
            if meta:
                results.append(
                    {
                        "file_id": meta.file_id,
                        "filename": meta.filename,
                        "file_type": meta.file_type.value,
                        "total_lines": meta.total_lines,
                        "mime_type": meta.mime_type,
                    }
                )
        return results

    # ── Text file operations ─────────────────────────────────────

    def read_text(
        self,
        agent_id: str,
        file_id: str,
        max_lines: int = MAX_READ_LINES,
    ) -> ReadResult | None:
        """Read the first N lines of a text file.

        Args:
            agent_id: Agent/session scope.
            file_id: File to read.
            max_lines: Maximum lines to return (default 800).

        Returns:
            ReadResult with content and total line count, or None if not found.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            return None

        entry = self._get_text_entry(agent_id, file_id)
        if not entry:
            return None

        end = min(max_lines, entry.total_lines)
        content = "\n".join(entry.lines[:end])

        return ReadResult(
            file_id=file_id,
            filename=entry.filename,
            total_lines=entry.total_lines,
            start_line=1,
            end_line=end,
            content=content,
        )

    def read_text_range(
        self,
        agent_id: str,
        file_id: str,
        start_line: int,
        end_line: int,
    ) -> ReadResult | None:
        """Read a specific line range from a text file.

        Args:
            agent_id: Agent/session scope.
            file_id: File to read.
            start_line: First line to read (1-indexed, inclusive).
            end_line: Last line to read (1-indexed, inclusive).

        Returns:
            ReadResult with content, or None if not found.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            return None

        entry = self._get_text_entry(agent_id, file_id)
        if not entry:
            return None

        # Clamp to valid range
        start = max(1, start_line)
        end = min(end_line, entry.total_lines)

        if start > end:
            return ReadResult(
                file_id=file_id,
                filename=entry.filename,
                total_lines=entry.total_lines,
                start_line=start,
                end_line=end,
                content="",
            )

        # Cap at MAX_READ_LINES
        if end - start + 1 > MAX_READ_LINES:
            end = start + MAX_READ_LINES - 1

        # Convert to 0-indexed for slicing
        content = "\n".join(entry.lines[start - 1 : end])

        return ReadResult(
            file_id=file_id,
            filename=entry.filename,
            total_lines=entry.total_lines,
            start_line=start,
            end_line=end,
            content=content,
        )

    def search_text(
        self,
        agent_id: str,
        file_id: str,
        query: str,
        case_insensitive: bool = True,
        max_hits: int = MAX_SEARCH_HITS,
        context_lines: int = CONTEXT_LINES,
    ) -> SearchResult | None:
        """Search a text file for keyword matches.

        Returns matching lines with surrounding context lines,
        similar to grep with -B/-A flags.

        Args:
            agent_id: Agent/session scope.
            file_id: File to search.
            query: Search string.
            case_insensitive: Whether to ignore case (default True).
            max_hits: Maximum matches to return (default 30).
            context_lines: Lines of context before/after each hit.

        Returns:
            SearchResult with hits, or None if file not found.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            return None

        entry = self._get_text_entry(agent_id, file_id)
        if not entry:
            return None

        if not query.strip():
            return SearchResult(
                file_id=file_id,
                filename=entry.filename,
                total_lines=entry.total_lines,
                query=query,
                hits=[],
                total_hits=0,
            )

        pattern = re.escape(query)
        flags = re.IGNORECASE if case_insensitive else 0
        compiled = re.compile(pattern, flags)

        hits: list[SearchHit] = []
        total_hits = 0

        for i, line in enumerate(entry.lines):
            if compiled.search(line):
                total_hits += 1
                if len(hits) < max_hits:
                    # Gather context
                    ctx_start = max(0, i - context_lines)
                    ctx_end = min(len(entry.lines), i + context_lines + 1)

                    before = [
                        f"L{ctx_start + j + 1}: {entry.lines[ctx_start + j]}"
                        for j in range(i - ctx_start)
                    ]
                    after = [
                        f"L{i + j + 2}: {entry.lines[i + j + 1]}"
                        for j in range(ctx_end - i - 1)
                    ]

                    hits.append(
                        SearchHit(
                            line_number=i + 1,  # 1-indexed
                            line_content=line,
                            context_before=before,
                            context_after=after,
                        )
                    )

        return SearchResult(
            file_id=file_id,
            filename=entry.filename,
            total_lines=entry.total_lines,
            query=query,
            hits=hits,
            total_hits=total_hits,
        )

    # ── Binary file operations ───────────────────────────────────

    def read_raw(
        self,
        agent_id: str,
        file_id: str,
    ) -> RawReadResult | None:
        """Read a binary file and return base64-encoded content.

        Args:
            agent_id: Agent/session scope.
            file_id: File to read.

        Returns:
            RawReadResult with base64 data, or None if not found.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            return None

        entry = self._get_binary_entry(agent_id, file_id)
        if not entry:
            return None

        return RawReadResult(
            file_id=file_id,
            filename=entry.filename,
            mime_type=entry.mime_type,
            size_bytes=entry.size_bytes,
            base64_data=base64.b64encode(entry.data).decode("ascii"),
        )

    # ── Removal operations ───────────────────────────────────────

    def remove_agent_files(self, agent_id: str) -> int:
        """Remove all files for an agent (cache + disk).

        Returns:
            Number of files removed.
        """
        if not _safe_id(agent_id, "agent_id"):
            return 0

        # Clear cache
        with self._lock:
            agent_cache = self._cache.pop(agent_id, {})

        # Clear disk
        agent_dir = self._base_dir / agent_id
        file_count = len(agent_cache)
        if agent_dir.exists():
            # Count disk files too (may have been evicted from cache)
            disk_files = [d for d in agent_dir.iterdir() if d.is_dir()]
            file_count = max(file_count, len(disk_files))
            shutil.rmtree(agent_dir, ignore_errors=True)

        if file_count:
            logger.info("Cleaned up {} files for agent {}", file_count, agent_id)
        return file_count

    def remove_file(self, agent_id: str, file_id: str) -> bool:
        """Remove a specific file (cache + disk).

        Returns:
            True if the file was found and removed.
        """
        if not _safe_id(agent_id, "agent_id") or not _safe_id(file_id, "file_id"):
            return False

        found = False

        # Remove from cache
        with self._lock:
            agent_cache = self._cache.get(agent_id, {})
            if file_id in agent_cache:
                del agent_cache[file_id]
                found = True
                if not agent_cache:
                    self._cache.pop(agent_id, None)

        # Remove from disk
        file_dir = self._file_dir(agent_id, file_id)
        if file_dir.exists():
            shutil.rmtree(file_dir, ignore_errors=True)
            found = True

        return found


def extract_text(content: bytes, filename: str) -> str:
    """Extract plain text from various file formats.

    Supports: PDF, TXT, MD, DOCX

    Args:
        content: Raw file bytes.
        filename: Original filename (used to detect format).

    Returns:
        Extracted plain text.

    Raises:
        ValueError: If the file type requires an uninstalled optional
            dependency, or if the file cannot be parsed.
    """
    ext = Path(filename).suffix.lower()

    if ext in {".txt", ".md"}:
        return content.decode("utf-8", errors="ignore")

    if ext == ".pdf":
        if not _HAS_PDF:
            raise ValueError(
                "PDF support requires pymupdf. Install with: pip install pymupdf"
            )
        doc = _fitz.open(stream=content, filetype="pdf")
        text = ""
        for page in doc:
            text += page.get_text() + "\n"
        doc.close()
        return text

    if ext == ".docx":
        if not _HAS_DOCX:
            raise ValueError(
                "DOCX support requires python-docx. "
                "Install with: pip install python-docx"
            )
        doc = _DocxDocument(io.BytesIO(content))
        return "\n".join(p.text for p in doc.paragraphs)

    # Unknown extension — attempt plain-text decode
    return content.decode("utf-8", errors="ignore")


# Module-level singleton
file_store = FileStore()
