"""Tests for v2 bridge verbs (Task 26)."""

from __future__ import annotations

import asyncio
import json
from unittest.mock import AsyncMock

import pytest

from supervoice.worker.bridge.protocol import (
    AgentAddParticipantVerb,
    AgentDispatchVerb,
    AgentEndCallVerb,
    AgentMergeVerb,
    AgentRemoveParticipantVerb,
    AgentSayVerb,
    AgentTransferVerb,
    parse_event,
)


# ── roundtrip tests ──────────────────────────────────────


def test_agent_say_verb_roundtrip():
    """AgentSayVerb serializes and parses correctly."""
    verb = AgentSayVerb(text="Hello there", interrupt_current=True)
    raw = json.loads(verb.model_dump_json())
    assert raw["event"] == "agent.say"
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentSayVerb)
    assert parsed.text == "Hello there"
    assert parsed.interrupt_current is True


def test_agent_transfer_verb_roundtrip():
    """AgentTransferVerb serializes and parses correctly."""
    verb = AgentTransferVerb(
        add={"type": "sip", "config": {"uri": "sip:+1@example"}},
        mode="warm",
        warm_handoff_ms=5000,
    )
    raw = json.loads(verb.model_dump_json())
    assert raw["event"] == "agent.transfer"
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentTransferVerb)
    assert parsed.mode == "warm"
    assert parsed.warm_handoff_ms == 5000


def test_agent_end_call_verb_roundtrip():
    """AgentEndCallVerb serializes and parses correctly."""
    verb = AgentEndCallVerb(reason="user requested")
    raw = json.loads(verb.model_dump_json())
    assert raw["event"] == "agent.end_call"
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentEndCallVerb)
    assert parsed.reason == "user requested"


def test_agent_dispatch_verb_roundtrip():
    """AgentDispatchVerb serializes and parses correctly."""
    verb = AgentDispatchVerb(
        runner_url="ws://other/agent",
        voice_profile_id="en-female",
        metadata={"key": "val"},
    )
    raw = json.loads(verb.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentDispatchVerb)
    assert parsed.runner_url == "ws://other/agent"


def test_agent_merge_verb_roundtrip():
    """AgentMergeVerb serializes and parses correctly."""
    verb = AgentMergeVerb(
        secondary_session_ids=["s2", "s3"],
        drop_participants=["p1"],
    )
    raw = json.loads(verb.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentMergeVerb)
    assert parsed.secondary_session_ids == ["s2", "s3"]
    assert parsed.drop_participants == ["p1"]


def test_agent_add_participant_roundtrip():
    """AgentAddParticipantVerb serializes and parses correctly."""
    verb = AgentAddParticipantVerb(
        type="sip",
        config={"uri": "sip:+1@example"},
    )
    raw = json.loads(verb.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentAddParticipantVerb)
    assert parsed.type == "sip"


def test_agent_remove_participant_roundtrip():
    """AgentRemoveParticipantVerb serializes and parses correctly."""
    verb = AgentRemoveParticipantVerb(participant_id="p-42")
    raw = json.loads(verb.model_dump_json())
    parsed = parse_event(raw)
    assert isinstance(parsed, AgentRemoveParticipantVerb)
    assert parsed.participant_id == "p-42"


# ── processor integration tests ──────────────────────────


@pytest.mark.anyio
async def test_agent_say_pushes_text_frames():
    """AgentSayVerb produces LLMFullResponse frames with text."""
    from pipecat.frames.frames import (
        LLMFullResponseEndFrame,
        LLMFullResponseStartFrame,
        TextFrame,
    )

    from supervoice.worker.bridge.processor import (
        AgentBridgeProcessor,
    )

    pushed: list = []
    original_push = AsyncMock(side_effect=lambda f, *a, **k: pushed.append(f))

    proc = AgentBridgeProcessor(echo_mode=False)
    proc.push_frame = original_push

    say_verb = AgentSayVerb(text="Greetings!")
    await proc._handle_agent_say(say_verb)

    assert len(pushed) == 3
    assert isinstance(pushed[0], LLMFullResponseStartFrame)
    assert isinstance(pushed[1], TextFrame)
    assert pushed[1].text == "Greetings!"
    assert isinstance(pushed[2], LLMFullResponseEndFrame)


@pytest.mark.anyio
async def test_agent_end_call_triggers_end():
    """AgentEndCallVerb sets flag and pushes EndFrame."""
    from pipecat.frames.frames import EndFrame

    from supervoice.worker.bridge.processor import (
        AgentBridgeProcessor,
    )

    pushed: list = []
    original_push = AsyncMock(side_effect=lambda f, *a, **k: pushed.append(f))

    proc = AgentBridgeProcessor(echo_mode=False)
    proc.push_frame = original_push

    assert proc.end_call_requested is False

    end_verb = AgentEndCallVerb(reason="done")
    await proc._handle_agent_end_call(end_verb)

    assert proc.end_call_requested is True
    assert len(pushed) == 1
    assert isinstance(pushed[0], EndFrame)


@pytest.mark.anyio
async def test_unrecognized_verb_logged_not_crash():
    """An unknown event type in _consume_bridge doesn't crash."""
    from supervoice.worker.bridge.client import BridgeContext
    from supervoice.worker.bridge.processor import (
        AgentBridgeProcessor,
    )
    from supervoice.worker.bridge.protocol import (
        ErrorEvent,
    )

    # ErrorEvent is not a verb the consumer expects to handle,
    # so it falls through to the else branch (unrecognized).
    mock_client = AsyncMock()
    mock_client.negotiated_events = frozenset(
        {"user.text", "error"}
    )
    mock_client._context = BridgeContext(
        session_id="s1", job_id="j1", room_id="r1"
    )

    # Feed an ErrorEvent through the consumer as if the runner
    # sent it (which would be unusual — errors are upstream).
    err_evt = ErrorEvent(
        call_id="s1",
        severity="warn",
        source="internal",
        code="test.unknown",
        message="test",
    )

    events_yielded = False

    async def mock_events():  # pyrefly: ignore[bad-argument-type]
        nonlocal events_yielded
        yield err_evt
        events_yielded = True

    mock_client.events = mock_events

    proc = AgentBridgeProcessor(echo_mode=False, client=mock_client)
    proc.push_frame = AsyncMock()

    # Run the consumer — should not crash
    await proc._consume_bridge()
    assert events_yielded is True
