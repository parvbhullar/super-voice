"""In-memory `(tenant_id, to_number) -> AgentConfig` cache with TTL."""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True)
class AgentConfig:
    """Agent configuration resolved for an inbound call mapping."""

    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class _CachedEntry:
    config: AgentConfig
    inserted_at: float


class NumberMappingCache:
    """In-memory `(tenant_id, to_number) -> AgentConfig` map with TTL.

    Stale entries (older than ``ttl_s``) are evicted lazily on ``get()``.
    ``upsert()`` overwrites the entry (used by initial sync + webhook).
    """

    def __init__(self, ttl_s: float = 300.0) -> None:
        self._entries: dict[tuple[str, str], _CachedEntry] = {}
        self._ttl_s = ttl_s
        self._lock = asyncio.Lock()

    async def get(
        self, *, tenant_id: str, to_number: str
    ) -> AgentConfig | None:
        """Return the cached config or ``None`` if missing/expired."""
        async with self._lock:
            entry = self._entries.get((tenant_id, to_number))
            if entry is None:
                return None
            if time.monotonic() - entry.inserted_at > self._ttl_s:
                self._entries.pop((tenant_id, to_number), None)
                return None
            return entry.config

    async def upsert(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None:
        """Insert or replace the cached config for the given key."""
        async with self._lock:
            self._entries[(tenant_id, to_number)] = _CachedEntry(
                config=config, inserted_at=time.monotonic()
            )

    async def remove(self, *, tenant_id: str, to_number: str) -> None:
        """Drop the cached entry, if any."""
        async with self._lock:
            self._entries.pop((tenant_id, to_number), None)

    async def size(self) -> int:
        """Return the current number of cached entries."""
        async with self._lock:
            return len(self._entries)
