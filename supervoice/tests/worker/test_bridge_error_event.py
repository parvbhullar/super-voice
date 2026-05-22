"""Tests for ErrorEvent upstream (Task 24)."""

from __future__ import annotations

import json
from unittest.mock import AsyncMock

import pytest

from supervoice.worker.bridge.protocol import (
    ErrorEvent,
    parse_event,
)


def test_error_event_roundtrip():
    """ErrorEvent serializes and parses correctly."""
    evt = ErrorEvent(
        call_id="sess-1",
        severity="error",
        source="stt",
        code="stt.timeout",
        message="STT timed out after 30s",
        retriable=True,
    )
    raw = json.loads(evt.model_dump_json())
    assert raw["event"] == "error"
    parsed = parse_event(raw)
    assert isinstance(parsed, ErrorEvent)
    assert parsed.severity == "error"
    assert parsed.source == "stt"
    assert parsed.code == "stt.timeout"
    assert parsed.retriable is True


def test_error_event_defaults():
    """ErrorEvent retriable defaults to False."""
    evt = ErrorEvent(
        call_id="c1",
        severity="warn",
        source="tts",
        code="tts.degraded",
        message="fallback voice used",
    )
    assert evt.retriable is False


@pytest.mark.anyio
async def test_processor_emits_error_on_stt_failure():
    """When STT processing raises, an ErrorEvent is sent upstream."""
    from supervoice.worker.bridge.client import BridgeContext
    from supervoice.worker.bridge.processor import AgentBridgeProcessor

    # Build a mock client that records sent events
    mock_client = AsyncMock()
    mock_client.negotiated_events = frozenset(
        {"user.text", "user.interrupted", "error"}
    )
    mock_client._context = BridgeContext(
        session_id="sess-1",
        job_id="job-1",
        room_id="room-1",
    )
    # Make send raise on the UserTextEvent (simulating transport
    # error), but succeed on ErrorEvent
    call_count = 0
    sent_events: list = []

    async def mock_send(evt):  # pyrefly: ignore[bad-argument-type]
        nonlocal call_count
        call_count += 1
        if call_count == 1:
            raise RuntimeError("simulated send failure")
        sent_events.append(evt)

    mock_client.send = mock_send

    proc = AgentBridgeProcessor(echo_mode=False, client=mock_client)

    # Import frame types
    from pipecat.frames.frames import TranscriptionFrame
    from pipecat.processors.frame_processor import FrameDirection

    # Monkey-patch push_frame to be a no-op
    proc.push_frame = AsyncMock()

    frame = TranscriptionFrame(text="hello", user_id="u1", timestamp="t1")
    await proc.process_frame(frame, FrameDirection.DOWNSTREAM)

    # The error event should have been sent
    assert len(sent_events) == 1
    assert isinstance(sent_events[0], ErrorEvent)
    assert sent_events[0].source == "stt"
    assert sent_events[0].code == "stt.processing_failed"
    assert sent_events[0].severity == "error"
