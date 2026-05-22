"""Tests for the V2 orchestrator Session model + state machine."""

from __future__ import annotations

import pytest

from supervoice.orchestrator.session.state import Session


def test_session_initial_state() -> None:
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    assert s.state == "incoming"
    assert s.external_call_id is None


def test_valid_transitions() -> None:
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    assert s.state == "ringing"
    s.transition("connected")
    assert s.state == "connected"
    s.transition("ended")
    assert s.state == "ended"


def test_invalid_transition_raises() -> None:
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    with pytest.raises(ValueError, match="invalid transition"):
        s.transition("connected")  # incoming -> connected (must go via ringing)


def test_terminal_state_blocks_further_transitions() -> None:
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    s.transition("ended")
    with pytest.raises(ValueError, match="terminal"):
        s.transition("connected")


def test_state_history_recorded() -> None:
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    s.transition("connected")
    assert [t[0] for t in s.state_history] == ["incoming", "ringing", "connected"]
