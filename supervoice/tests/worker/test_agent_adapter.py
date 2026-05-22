"""Tests for AgentAdapter lifecycle (attach/detach/wait_for_end)."""

from __future__ import annotations

import asyncio
from typing import Any
from unittest.mock import AsyncMock, MagicMock

from pydantic import SecretStr

from supervoice.shared.voice_profile.catalog import (
    STTSpec,
    TTSSpec,
    VoiceProfile,
    VoiceProfileCatalog,
)
from supervoice.worker.agent_adapter import AgentAdapter, JobContext


def _make_profile() -> VoiceProfile:
    return VoiceProfile(
        id="test-profile",
        language="en",
        persona="test",
        stt_preference=[STTSpec(provider="deepgram", language="en")],
        tts_preference=[TTSSpec(provider="cartesia", voice_id="v1")],
    )


def _make_catalog() -> VoiceProfileCatalog:
    return VoiceProfileCatalog(profiles=[_make_profile()])


def _make_ctx() -> JobContext:
    return JobContext(
        job_id="job-1",
        session_id="sess-1",
        room={"url": "wss://x", "token": "tk", "name": "r1"},
        voice_profile_id="test-profile",
        runner_url="wss://runner.example/ws",
        agent_secret="sek",
        metadata={},
    )


def _api_keys() -> dict[str, SecretStr]:
    return {
        "deepgram": SecretStr("dg"),
        "cartesia": SecretStr("ca"),
    }


def _make_bridge_client_factory() -> tuple[Any, MagicMock]:
    client = MagicMock()
    client.connect = AsyncMock()
    client.close = AsyncMock()
    return (lambda url: client), client


def _make_pipeline_builder() -> tuple[Any, MagicMock]:
    bridge = MagicMock()
    bridge.attach_client = MagicMock()
    bridge.start = AsyncMock()
    bridge.stop = AsyncMock()
    pipeline = MagicMock()
    return (lambda cfg: (pipeline, bridge)), bridge


def _make_runner_factory() -> tuple[Any, MagicMock]:
    runner = MagicMock()
    run_event = asyncio.Event()

    async def _run(_task: Any) -> None:
        await run_event.wait()

    runner.run = AsyncMock(side_effect=_run)
    runner._run_event = run_event  # type: ignore[attr-defined]
    return (lambda: runner), runner


async def test_attach_opens_bridge_and_starts_pipeline() -> None:
    bcf, bridge_client = _make_bridge_client_factory()
    pb, bridge_proc = _make_pipeline_builder()
    rf, _runner = _make_runner_factory()

    adapter = AgentAdapter(
        ctx=_make_ctx(),
        api_keys=_api_keys(),
        catalog=_make_catalog(),
        bridge_client_factory=bcf,
        pipeline_builder=pb,
        runner_factory=rf,
    )

    await adapter.attach()

    bridge_client.connect.assert_awaited_once()
    bridge_proc.attach_client.assert_called_once_with(bridge_client)
    bridge_proc.start.assert_awaited_once()
    assert adapter._pipeline_task is not None
    assert not adapter._pipeline_task.done()

    await adapter.detach()


async def test_detach_closes_resources() -> None:
    bcf, bridge_client = _make_bridge_client_factory()
    pb, bridge_proc = _make_pipeline_builder()
    rf, _runner = _make_runner_factory()

    adapter = AgentAdapter(
        ctx=_make_ctx(),
        api_keys=_api_keys(),
        catalog=_make_catalog(),
        bridge_client_factory=bcf,
        pipeline_builder=pb,
        runner_factory=rf,
    )
    await adapter.attach()
    task = adapter._pipeline_task

    await adapter.detach(reason="ended")

    bridge_proc.stop.assert_awaited_once()
    bridge_client.close.assert_awaited_once()
    assert task is not None and task.done()


async def test_detach_is_idempotent_and_swallows_errors() -> None:
    bcf, bridge_client = _make_bridge_client_factory()
    pb, bridge_proc = _make_pipeline_builder()
    rf, _runner = _make_runner_factory()
    bridge_proc.stop = AsyncMock(side_effect=RuntimeError("boom"))

    adapter = AgentAdapter(
        ctx=_make_ctx(),
        api_keys=_api_keys(),
        catalog=_make_catalog(),
        bridge_client_factory=bcf,
        pipeline_builder=pb,
        runner_factory=rf,
    )
    await adapter.attach()

    await adapter.detach()
    bridge_client.close.assert_awaited_once()

    # Second detach is a no-op (idempotent).
    await adapter.detach()
    bridge_client.close.assert_awaited_once()


async def test_wait_for_end_unblocks_after_pipeline_ends() -> None:
    bcf, _client = _make_bridge_client_factory()
    pb, _bridge = _make_pipeline_builder()
    rf, runner = _make_runner_factory()

    adapter = AgentAdapter(
        ctx=_make_ctx(),
        api_keys=_api_keys(),
        catalog=_make_catalog(),
        bridge_client_factory=bcf,
        pipeline_builder=pb,
        runner_factory=rf,
    )
    await adapter.attach()

    runner._run_event.set()
    await asyncio.wait_for(adapter.wait_for_end(), timeout=1.0)

    await adapter.detach()
