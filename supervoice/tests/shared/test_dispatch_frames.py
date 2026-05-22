"""Tests for the worker dispatch protocol frames."""

from __future__ import annotations

import json

import pytest
from pydantic import ValidationError

from supervoice.shared.dispatch_protocol import (
    Dispatch,
    DispatchAck,
    Heartbeat,
    JobCompleted,
    Register,
    Registered,
    StateChanged,
    WorkerCapabilities,
    parse_frame,
)


def test_register_roundtrip() -> None:
    original = Register(
        worker_id="w-1",
        pool="default",
        capabilities=WorkerCapabilities(
            voice_profiles=["alpha", "beta"], max_concurrent=4
        ),
    )
    raw = json.loads(original.model_dump_json())
    parsed = parse_frame(raw)
    assert isinstance(parsed, Register)
    assert parsed == original


def test_registered_parse() -> None:
    parsed = parse_frame({"type": "registered", "heartbeat_interval_s": 30})
    assert isinstance(parsed, Registered)
    assert parsed.heartbeat_interval_s == 30


def test_dispatch_parse() -> None:
    raw = {
        "type": "dispatch",
        "job_id": "j-1",
        "session_id": "s-1",
        "room": {"url": "wss://r", "token": "tok", "name": "room-1"},
        "voice_profile_id": "vp-1",
        "runner_url": "https://runner.example",
        "agent_secret": "shh",
        "metadata": {"caller": "+15555550100"},
    }
    parsed = parse_frame(raw)
    assert isinstance(parsed, Dispatch)
    assert parsed.job_id == "j-1"
    assert parsed.room["name"] == "room-1"
    assert parsed.metadata == {"caller": "+15555550100"}


def test_heartbeat_parse() -> None:
    parsed = parse_frame({"type": "heartbeat", "active_jobs": 12})
    assert isinstance(parsed, Heartbeat)
    assert parsed.active_jobs == 12


def test_dispatch_ack_accepted() -> None:
    parsed = parse_frame(
        {"type": "dispatch.ack", "job_id": "j-1", "status": "accepted"}
    )
    assert isinstance(parsed, DispatchAck)
    assert parsed.status == "accepted"
    assert parsed.reason is None


def test_dispatch_ack_rejected_with_reason() -> None:
    parsed = parse_frame(
        {
            "type": "dispatch.ack",
            "job_id": "j-2",
            "status": "rejected",
            "reason": "no_slot",
        }
    )
    assert isinstance(parsed, DispatchAck)
    assert parsed.status == "rejected"
    assert parsed.reason == "no_slot"


def test_state_changed_connected() -> None:
    parsed = parse_frame(
        {"type": "state_changed", "job_id": "j-1", "state": "connected"}
    )
    assert isinstance(parsed, StateChanged)
    assert parsed.state == "connected"
    assert parsed.details is None


def test_job_completed_terminal_state() -> None:
    parsed = parse_frame(
        {
            "type": "job.completed",
            "job_id": "j-1",
            "duration_s": 42.5,
            "final_state": "ended",
        }
    )
    assert isinstance(parsed, JobCompleted)
    assert parsed.final_state == "ended"
    assert parsed.duration_s == 42.5


def test_job_completed_non_terminal_rejected() -> None:
    with pytest.raises(ValidationError):
        JobCompleted(
            job_id="j-1",
            duration_s=1.0,
            final_state="connected",  # type: ignore[arg-type]
        )


def test_unknown_type_raises() -> None:
    with pytest.raises(ValueError, match="unknown dispatch frame type"):
        parse_frame({"type": "acme"})


def test_missing_type_raises() -> None:
    with pytest.raises(ValueError, match="missing or non-string type field"):
        parse_frame({})


def test_capabilities_min_max_validation() -> None:
    with pytest.raises(ValidationError):
        WorkerCapabilities(voice_profiles=[], max_concurrent=0)  # pyrefly: ignore[bad-argument-type]
    ok = WorkerCapabilities(voice_profiles=[], max_concurrent=10_000)
    assert ok.max_concurrent == 10_000
    with pytest.raises(ValidationError):
        WorkerCapabilities(voice_profiles=[], max_concurrent=10_001)  # pyrefly: ignore[bad-argument-type]
