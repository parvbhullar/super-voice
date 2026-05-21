import json

import pytest
from pydantic import ValidationError

from supervoice.bridge.protocol import (
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    UserInterruptEvent,
    UserTextEvent,
    parse_event,
)


def test_user_text_event_roundtrip():
    evt = UserTextEvent(turn_id=1, text="hello", final=True)
    raw = evt.model_dump_json()
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, UserTextEvent)
    assert parsed.turn_id == 1
    assert parsed.text == "hello"
    assert parsed.final is True


def test_agent_text_delta_parses():
    raw = '{"event":"agent.text.delta","turn_id":1,"text":"hi"}'
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, AgentTextDeltaEvent)
    assert parsed.text == "hi"


def test_agent_text_end_parses():
    raw = '{"event":"agent.text.end","turn_id":1}'
    parsed = parse_event(json.loads(raw))
    assert isinstance(parsed, AgentTextEndEvent)


def test_user_interrupt_parses():
    parsed = parse_event({"event": "user.interrupted", "turn_id": 2})
    assert isinstance(parsed, UserInterruptEvent)
    assert parsed.turn_id == 2


def test_unknown_event_raises():
    try:
        parse_event({"event": "acme"})
    except ValueError:
        return
    raise AssertionError("expected ValueError")


def test_user_text_too_long_rejected():
    """64 KiB+1 chars should fail validation."""
    with pytest.raises(ValidationError):
        UserTextEvent(turn_id=1, text="a" * 65537)


def test_agent_delta_too_long_rejected():
    with pytest.raises(ValidationError):
        AgentTextDeltaEvent(turn_id=1, text="b" * 4097)


def test_parse_event_missing_turn_id_raises():
    """Pydantic should raise ValidationError if turn_id is missing."""
    with pytest.raises(ValidationError):
        parse_event({"event": "user.text", "text": "hi"})


def test_parse_event_no_event_key_raises():
    """parse_event must raise ValueError when 'event' key is absent."""
    try:
        parse_event({"text": "no event field"})
    except ValueError:
        return
    raise AssertionError("expected ValueError")
