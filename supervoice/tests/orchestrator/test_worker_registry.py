"""Tests for the V2 orchestrator WorkerRegistry."""

from __future__ import annotations

import asyncio

import pytest

from supervoice.orchestrator.worker_registry import WorkerRegistry
from supervoice.shared.dispatch_protocol import WorkerCapabilities


def _caps(profiles: list[str], max_concurrent: int = 4) -> WorkerCapabilities:
    return WorkerCapabilities(voice_profiles=profiles, max_concurrent=max_concurrent)


@pytest.mark.asyncio
async def test_register_and_list() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"]))
    workers = await reg.all_workers()
    assert len(workers) == 1
    assert workers[0].worker_id == "w1"


@pytest.mark.asyncio
async def test_pick_returns_least_loaded() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"]))
    await reg.register("w2", "default", _caps(["hi-female"]))
    await reg.mark_dispatched("w1", "job-a")
    await reg.mark_dispatched("w1", "job-b")
    picked = await reg.pick("hi-female")
    assert picked is not None
    assert picked.worker_id == "w2"


@pytest.mark.asyncio
async def test_pick_filters_by_voice_profile() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"]))
    await reg.register("w2", "default", _caps(["en-female"]))
    picked = await reg.pick("hi-female")
    assert picked is not None and picked.worker_id == "w1"
    picked = await reg.pick("en-female")
    assert picked is not None and picked.worker_id == "w2"
    assert await reg.pick("ja-male") is None


@pytest.mark.asyncio
async def test_pick_filters_by_pool() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "pool-a", _caps(["hi-female"]))
    await reg.register("w2", "pool-b", _caps(["hi-female"]))
    picked = await reg.pick("hi-female", pool="pool-a")
    assert picked is not None and picked.worker_id == "w1"
    picked = await reg.pick("hi-female", pool="pool-b")
    assert picked is not None and picked.worker_id == "w2"
    assert await reg.pick("hi-female", pool="pool-c") is None


@pytest.mark.asyncio
async def test_pick_returns_none_when_at_capacity() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"], max_concurrent=1))
    await reg.mark_dispatched("w1", "job-a")
    assert await reg.pick("hi-female") is None


@pytest.mark.asyncio
async def test_heartbeat_timeout_deregisters() -> None:
    reg = WorkerRegistry(heartbeat_timeout_s=0.05)
    await reg.register("w1", "default", _caps(["hi-female"]))
    await asyncio.sleep(0.1)
    assert await reg.pick("hi-female") is None
    assert await reg.all_workers() == []


@pytest.mark.asyncio
async def test_mark_dispatched_and_completed_change_load() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"]))
    workers = await reg.all_workers()
    assert workers[0].load == 0
    await reg.mark_dispatched("w1", "job-a")
    workers = await reg.all_workers()
    assert workers[0].load == 1
    await reg.mark_completed("w1", "job-a")
    workers = await reg.all_workers()
    assert workers[0].load == 0


@pytest.mark.asyncio
async def test_deregister_removes_worker() -> None:
    reg = WorkerRegistry()
    await reg.register("w1", "default", _caps(["hi-female"]))
    await reg.deregister("w1")
    assert await reg.all_workers() == []
    assert await reg.pick("hi-female") is None


@pytest.mark.asyncio
async def test_heartbeat_refreshes_timestamp() -> None:
    reg = WorkerRegistry(heartbeat_timeout_s=0.2)
    await reg.register("w1", "default", _caps(["hi-female"]))
    await asyncio.sleep(0.1)
    await reg.heartbeat("w1", active_jobs=0)
    await asyncio.sleep(0.15)
    # Still alive thanks to the refreshed heartbeat.
    picked = await reg.pick("hi-female")
    assert picked is not None and picked.worker_id == "w1"
