"""Per-worker job registry — dispatched jobs map to AgentAdapter lifecycles.

Accepting a ``Dispatch`` frame:
    1. Capacity check (``max_concurrent``).
    2. Build a ``JobContext`` from the frame.
    3. Instantiate an ``AgentAdapter`` and store under ``job_id``.
    4. Spawn the lifecycle task (attach → wait → detach + report).

The runner publishes ``StateChanged`` and ``JobCompleted`` upstream via the
``upstream_send`` callable injected by ``worker/main.py``.
"""

from __future__ import annotations

import asyncio
import contextlib
import time
from typing import Any, Awaitable, Callable, Literal

from loguru import logger
from pydantic import SecretStr

from supervoice.shared.dispatch_protocol import (
    Dispatch,
    JobCompleted,
    StateChanged,
)
from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog

from .agent_adapter import AgentAdapter, JobContext


SendFrame = Callable[[dict[str, Any]], Awaitable[None]]
AdapterFactory = Callable[[JobContext], AgentAdapter]
FinalState = Literal["ended", "failed", "rejected", "timed_out"]


def _default_adapter_factory(
    api_keys: dict[str, SecretStr],
    catalog: VoiceProfileCatalog,
) -> AdapterFactory:
    def _factory(ctx: JobContext) -> AgentAdapter:
        return AgentAdapter(ctx=ctx, api_keys=api_keys, catalog=catalog)

    return _factory


class JobRunner:
    """Tracks active jobs and runs their lifecycle."""

    def __init__(
        self,
        *,
        max_concurrent: int,
        api_keys: dict[str, SecretStr],
        catalog: VoiceProfileCatalog,
        upstream_send: SendFrame,
        adapter_factory: AdapterFactory | None = None,
    ) -> None:
        self._max_concurrent = max_concurrent
        self._upstream_send = upstream_send
        self._adapter_factory = adapter_factory or _default_adapter_factory(
            api_keys, catalog
        )
        self._active: dict[str, AgentAdapter] = {}
        self._tasks: dict[str, asyncio.Task[None]] = {}
        self._lock = asyncio.Lock()

    def active_count(self) -> int:
        """Cheap snapshot — safe to call from the heartbeat loop."""
        return len(self._active)

    async def accept(self, frame: Dispatch) -> bool:
        """Try to accept a dispatched job. Returns False if at capacity."""
        async with self._lock:
            if len(self._active) >= self._max_concurrent:
                return False
            if frame.job_id in self._active:
                logger.warning(f"duplicate dispatch for job {frame.job_id}")
                return False
            ctx = JobContext(
                job_id=frame.job_id,
                session_id=frame.session_id,
                room=frame.room,
                voice_profile_id=frame.voice_profile_id,
                runner_url=frame.runner_url,
                agent_secret=frame.agent_secret,
                metadata=frame.metadata,
            )
            adapter = self._adapter_factory(ctx)
            self._active[frame.job_id] = adapter

        task = asyncio.create_task(self._run_job(frame.job_id, adapter))
        self._tasks[frame.job_id] = task
        return True

    async def _run_job(self, job_id: str, adapter: AgentAdapter) -> None:
        started = time.monotonic()
        final_state: FinalState = "ended"
        try:
            await adapter.attach()
            await self._upstream_send(
                StateChanged(job_id=job_id, state="connected").model_dump()
            )
            await adapter.wait_for_end()
        except Exception as e:
            logger.exception(f"job {job_id} failed: {e}")
            final_state = "failed"
        finally:
            duration = time.monotonic() - started
            with contextlib.suppress(Exception):
                await adapter.detach(reason=final_state)
            async with self._lock:
                self._active.pop(job_id, None)
                self._tasks.pop(job_id, None)
            with contextlib.suppress(Exception):
                await self._upstream_send(
                    JobCompleted(
                        job_id=job_id,
                        duration_s=duration,
                        final_state=final_state,
                    ).model_dump()
                )

    async def shutdown(self) -> None:
        """Detach every active job and await their lifecycle tasks."""
        async with self._lock:
            adapters = list(self._active.values())
            tasks = list(self._tasks.values())
        for a in adapters:
            with contextlib.suppress(Exception):
                await a.detach(reason="ended")
        for t in tasks:
            with contextlib.suppress(Exception):
                await t


__all__ = ["JobRunner"]
