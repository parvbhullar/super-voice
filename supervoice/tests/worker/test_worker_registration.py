"""Tests for the worker-side registration loop."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from supervoice.shared.dispatch_protocol import (
    Dispatch,
    DispatchAck,
    Heartbeat,
    Register,
    Registered,
    WorkerCapabilities,
    parse_frame,
)
from supervoice.worker.registration import WorkerLink, WorkerRegistration


class FakeLink:
    """In-memory bidirectional link used to simulate the orchestrator."""

    def __init__(self) -> None:
        # frames the worker sends are read here by the test
        self.from_worker: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        # frames the test wants delivered to the worker
        self.to_worker: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self.closed = False

    async def send(self, frame: dict[str, Any]) -> None:
        await self.from_worker.put(frame)

    async def recv(self) -> dict[str, Any]:
        return await self.to_worker.get()

    async def close(self) -> None:
        self.closed = True


def _caps() -> WorkerCapabilities:
    return WorkerCapabilities(voice_profiles=["en-female"], max_concurrent=2)


def _build_registration(
    link: FakeLink,
    *,
    handler: Any,
    active: int = 0,
    heartbeat_interval_s: int | None = None,
) -> WorkerRegistration:
    async def link_factory(_url: str, _secret: str) -> WorkerLink:
        return link

    reg = WorkerRegistration(
        orchestrator_url="ws://test",
        shared_secret="s",
        worker_id="w-1",
        pool="default",
        capabilities=_caps(),
        dispatch_handler=handler,
        active_jobs_counter=lambda: active,
        link_factory=link_factory,
        reconnect_delay_s=0.0,
    )
    if heartbeat_interval_s is not None:
        reg._heartbeat_interval_s = heartbeat_interval_s
    return reg


async def test_register_sends_correct_frame() -> None:
    link = FakeLink()

    async def handler(_d: Dispatch) -> bool:
        return True

    reg = _build_registration(link, handler=handler)

    # Pre-queue the Registered response so serve_once doesn't block.
    await link.to_worker.put(Registered(heartbeat_interval_s=60).model_dump())

    task = asyncio.create_task(reg.serve_once())
    try:
        sent = await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        frame = parse_frame(sent)
        assert isinstance(frame, Register)
        assert frame.worker_id == "w-1"
        assert frame.pool == "default"
        assert frame.capabilities.voice_profiles == ["en-female"]
        assert frame.capabilities.max_concurrent == 2
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task


async def test_registered_response_sets_heartbeat_interval() -> None:
    link = FakeLink()

    async def handler(_d: Dispatch) -> bool:
        return True

    reg = _build_registration(link, handler=handler)
    await link.to_worker.put(Registered(heartbeat_interval_s=7).model_dump())

    task = asyncio.create_task(reg.serve_once())
    try:
        # wait until register sent + handshake processed
        await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        # Give the handshake one event-loop tick to apply the interval.
        for _ in range(20):
            if reg._heartbeat_interval_s == 7:
                break
            await asyncio.sleep(0.01)
        assert reg._heartbeat_interval_s == 7
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task


async def test_dispatch_calls_handler_and_acks_accepted() -> None:
    link = FakeLink()
    called: list[Dispatch] = []

    async def handler(d: Dispatch) -> bool:
        called.append(d)
        return True

    reg = _build_registration(link, handler=handler)
    await link.to_worker.put(Registered(heartbeat_interval_s=60).model_dump())
    dispatch = Dispatch(
        job_id="j1",
        session_id="s1",
        room={"url": "u", "token": "t", "name": "n"},
        voice_profile_id="en-female",
        runner_url="wss://r",
        agent_secret="x",
    )
    await link.to_worker.put(dispatch.model_dump())

    task = asyncio.create_task(reg.serve_once())
    try:
        # Drain Register frame
        await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        # Next frame from worker must be the DispatchAck
        ack_raw = await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        ack = parse_frame(ack_raw)
        assert isinstance(ack, DispatchAck)
        assert ack.job_id == "j1"
        assert ack.status == "accepted"
        assert ack.reason is None
        assert len(called) == 1
        assert called[0].job_id == "j1"
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task


async def test_handler_rejection_yields_reject_ack() -> None:
    link = FakeLink()

    async def handler(_d: Dispatch) -> bool:
        return False

    reg = _build_registration(link, handler=handler)
    await link.to_worker.put(Registered(heartbeat_interval_s=60).model_dump())
    await link.to_worker.put(
        Dispatch(
            job_id="j2",
            session_id="s2",
            room={"url": "u", "token": "t", "name": "n"},
            voice_profile_id="en-female",
            runner_url="wss://r",
            agent_secret="x",
        ).model_dump()
    )

    task = asyncio.create_task(reg.serve_once())
    try:
        await asyncio.wait_for(link.from_worker.get(), timeout=1.0)  # Register
        ack_raw = await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        ack = parse_frame(ack_raw)
        assert isinstance(ack, DispatchAck)
        assert ack.status == "rejected"
        assert ack.reason == "no_slot"
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task


async def test_invalid_frame_continues_loop() -> None:
    link = FakeLink()
    seen: list[Dispatch] = []

    async def handler(d: Dispatch) -> bool:
        seen.append(d)
        return True

    reg = _build_registration(link, handler=handler)
    await link.to_worker.put(Registered(heartbeat_interval_s=60).model_dump())
    # Invalid frame (no type field) — should be logged and skipped.
    await link.to_worker.put({"junk": True})
    await link.to_worker.put(
        Dispatch(
            job_id="j3",
            session_id="s3",
            room={"url": "u", "token": "t", "name": "n"},
            voice_profile_id="en-female",
            runner_url="wss://r",
            agent_secret="x",
        ).model_dump()
    )

    task = asyncio.create_task(reg.serve_once())
    try:
        await asyncio.wait_for(link.from_worker.get(), timeout=1.0)  # Register
        ack_raw = await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        ack = parse_frame(ack_raw)
        assert isinstance(ack, DispatchAck)
        assert ack.job_id == "j3"
        assert ack.status == "accepted"
        assert len(seen) == 1
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task


async def test_heartbeat_sent_with_active_jobs_count() -> None:
    link = FakeLink()

    async def handler(_d: Dispatch) -> bool:
        return True

    counter = {"v": 3}

    async def link_factory(_u: str, _s: str) -> WorkerLink:
        return link

    reg = WorkerRegistration(
        orchestrator_url="ws://test",
        shared_secret="s",
        worker_id="w-1",
        pool="default",
        capabilities=_caps(),
        dispatch_handler=handler,
        active_jobs_counter=lambda: counter["v"],
        link_factory=link_factory,
        reconnect_delay_s=0.0,
    )

    # Very small heartbeat interval. Registered must use ge=1.
    await link.to_worker.put(Registered(heartbeat_interval_s=1).model_dump())

    task = asyncio.create_task(reg.serve_once())
    try:
        # Drain Register
        await asyncio.wait_for(link.from_worker.get(), timeout=1.0)
        # First heartbeat ~1s away. Increase tolerance.
        hb_raw = await asyncio.wait_for(link.from_worker.get(), timeout=2.5)
        hb = parse_frame(hb_raw)
        assert isinstance(hb, Heartbeat)
        assert hb.active_jobs == 3
    finally:
        await reg.close()
        task.cancel()
        with pytest.raises((asyncio.CancelledError, Exception)):
            await task
