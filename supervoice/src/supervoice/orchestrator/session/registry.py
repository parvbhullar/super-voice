"""In-memory SessionRegistry with tenant isolation and reconnect TTL.

Storage key is ``(tenant_id, session_id)`` so cross-tenant lookups return
``None`` by construction. Drain finalization happens lazily on ``get``
rather than via a background sweeper — adequate for V1; a background
sweeper can be added when registries grow large.

Time source: ``time.monotonic`` (matches ``Session.created_at``).
"""

from __future__ import annotations

import asyncio
import time

from .state import Session


class SessionRegistry:
    """Tenant-scoped session storage with TTL-driven drain finalization."""

    def __init__(self, reconnect_ttl_s: float = 30.0) -> None:
        self._sessions: dict[tuple[str, str], Session] = {}
        self._drain_started: dict[tuple[str, str], float] = {}
        self._reconnect_ttl_s = reconnect_ttl_s
        self._lock = asyncio.Lock()

    async def register(self, session: Session) -> None:
        """Insert (or replace) a session under its tenant/session key."""
        async with self._lock:
            self._sessions[(session.tenant_id, session.session_id)] = session

    async def get(self, session_id: str, *, tenant_id: str) -> Session | None:
        """Return the session for ``(tenant_id, session_id)`` or ``None``.

        If a drain timer for this session has elapsed past the configured
        ``reconnect_ttl_s``, the session is transitioned to ``ended``
        before being returned (lazy finalization).
        """
        async with self._lock:
            session = self._sessions.get((tenant_id, session_id))
            if session is None:
                return None
            self._maybe_finalize_drain(session)
            return session

    async def list(self, *, tenant_id: str) -> list[Session]:
        """Return all sessions belonging to ``tenant_id``."""
        async with self._lock:
            return [
                s for (t, _), s in self._sessions.items() if t == tenant_id
            ]

    async def mark_draining(self, session_id: str, *, tenant_id: str) -> None:
        """Start the reconnect-TTL drain timer for a session.

        State transitions remain the caller's responsibility; this method
        only records the drain start timestamp so a subsequent ``get``
        can decide to finalize.
        """
        async with self._lock:
            key = (tenant_id, session_id)
            if key in self._sessions:
                self._drain_started[key] = time.monotonic()

    def _maybe_finalize_drain(self, session: Session) -> None:
        key = (session.tenant_id, session.session_id)
        start = self._drain_started.get(key)
        if start is None:
            return
        if time.monotonic() - start >= self._reconnect_ttl_s:
            if session.state not in {"ended", "rejected", "timed_out", "failed"}:
                session.transition("ended")
            self._drain_started.pop(key, None)
