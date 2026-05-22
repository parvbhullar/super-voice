"""Tests for MetricEvent upstream (Task 25)."""

from __future__ import annotations

import asyncio
import json
from unittest.mock import AsyncMock

import pytest

from supervoice.worker.bridge.client import BridgeContext
from supervoice.worker.bridge.processor import AgentBridgeProcessor
from supervoice.worker.bridge.protocol import (
    MetricEvent,
    parse_event,
)


def test_metric_event_roundtrip():
    """MetricEvent serializes and parses correctly."""
    evt = MetricEvent(
        call_id="sess-1",
        ttfa_ms=120.5,
        asr_p95_ms=45.0,
        tts_p95_ms=80.0,
        turns=3,
        cost_usd_so_far=0.02,
    )
    raw = json.loads(evt.model_dump_json())
    assert raw["event"] == "metric"
    parsed = parse_event(raw)
    assert isinstance(parsed, MetricEvent)
    assert parsed.ttfa_ms == 120.5
    assert parsed.turns == 3
    assert parsed.cost_usd_so_far == 0.02


def test_metric_event_defaults():
    """MetricEvent fields default to None/0."""
    evt = MetricEvent(call_id="c1")
    assert evt.ttfa_ms is None
    assert evt.asr_p95_ms is None
    assert evt.tts_p95_ms is None
    assert evt.turns == 0
    assert evt.cost_usd_so_far is None


@pytest.mark.anyio
async def test_metric_periodic_emission():
    """Processor emits >=2 MetricEvents in 0.3s with 0.1s interval."""
    mock_client = AsyncMock()
    mock_client.negotiated_events = frozenset(
        {"user.text", "user.interrupted", "metric"}
    )
    mock_client._context = BridgeContext(
        session_id="sess-1",
        job_id="job-1",
        room_id="room-1",
    )

    sent_events: list = []

    async def mock_send(evt):  # pyrefly: ignore[bad-argument-type]
        sent_events.append(evt)

    mock_client.send = mock_send

    # Use events() that never yields (no bridge events to consume)
    async def empty_events():  # pyrefly: ignore[bad-argument-type]
        await asyncio.sleep(999)
        # Make this an async generator
        return
        yield  # type: ignore[misc]

    mock_client.events = empty_events

    proc = AgentBridgeProcessor(
        echo_mode=False,
        client=mock_client,
        metric_interval_s=0.1,
    )
    proc.push_frame = AsyncMock()

    await proc.start()
    try:
        await asyncio.sleep(0.35)
    finally:
        await proc.stop()

    metric_events = [
        e for e in sent_events if isinstance(e, MetricEvent)
    ]
    assert len(metric_events) >= 2
    # All should have the correct call_id
    for m in metric_events:
        assert m.call_id == "sess-1"
        assert m.event == "metric"


@pytest.mark.anyio
async def test_metric_not_emitted_when_not_negotiated():
    """If 'metric' is not in negotiated_events, no MetricEvent sent."""
    mock_client = AsyncMock()
    mock_client.negotiated_events = frozenset(
        {"user.text", "user.interrupted"}
    )
    mock_client._context = BridgeContext(
        session_id="sess-1",
        job_id="job-1",
        room_id="room-1",
    )

    sent_events: list = []

    async def mock_send(evt):  # pyrefly: ignore[bad-argument-type]
        sent_events.append(evt)

    mock_client.send = mock_send

    async def empty_events():  # pyrefly: ignore[bad-argument-type]
        await asyncio.sleep(999)
        return
        yield  # type: ignore[misc]

    mock_client.events = empty_events

    proc = AgentBridgeProcessor(
        echo_mode=False,
        client=mock_client,
        metric_interval_s=0.1,
    )
    proc.push_frame = AsyncMock()

    await proc.start()
    try:
        await asyncio.sleep(0.35)
    finally:
        await proc.stop()

    metric_events = [
        e for e in sent_events if isinstance(e, MetricEvent)
    ]
    assert len(metric_events) == 0
