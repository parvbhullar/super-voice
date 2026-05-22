"""Shared FastAPI dependencies and shared interfaces for the orchestrator.

This module also defines the :class:`NumberMappingCache` Protocol and the
:class:`AgentConfig` dataclass that Stream G's concrete cache will satisfy
structurally. The final wiring (DB-backed cache) lands in Task 21.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol

from fastapi import Request


@dataclass(frozen=True)
class AgentConfig:
    """Per-tenant agent configuration resolved from a number mapping."""

    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict = field(default_factory=dict)


class NumberMappingCache(Protocol):
    """Tenant-scoped to_number -> AgentConfig lookup.

    Stream G provides the concrete impl (DB + in-memory cache). Stream F
    only depends on this Protocol surface.
    """

    async def get(
        self, *, tenant_id: str, to_number: str
    ) -> AgentConfig | None: ...

    async def upsert(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None: ...


def get_room_engine(request: Request) -> object:
    """Return the RoomEngine bound to the app."""
    return request.app.state.room_engine


def get_mapping_cache(request: Request) -> NumberMappingCache:
    """Return the NumberMappingCache bound to the app."""
    return request.app.state.mapping_cache


def get_worker_dispatcher(request: Request) -> object:
    """Return the WorkerDispatcher bound to the app."""
    return request.app.state.worker_dispatcher


def get_session_registry(request: Request) -> object:
    """Return the SessionRegistry bound to the app."""
    return request.app.state.session_registry


def get_idempotency_key(request: Request) -> str | None:
    """Read the optional ``Idempotency-Key`` request header."""
    return request.headers.get("idempotency-key")


__all__ = [
    "AgentConfig",
    "NumberMappingCache",
    "get_idempotency_key",
    "get_mapping_cache",
    "get_room_engine",
    "get_session_registry",
    "get_worker_dispatcher",
]
