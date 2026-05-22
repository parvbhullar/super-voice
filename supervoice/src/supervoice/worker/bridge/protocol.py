"""Bridge wire protocol v1 + v2 handshake.

Defines the event types exchanged with the remote Agent Bridge:
two upstream (user -> bridge) and two downstream (bridge -> user),
plus the v2 hello/hello.ack handshake frames for version negotiation.
"""

from __future__ import annotations

from typing import Any, Literal, Union

from pydantic import BaseModel, Field

# ── v1 events ──────────────────────────────────────────────

V1_EVENTS = frozenset({"user.text", "user.interrupted"})
V1_VERBS = frozenset({"agent.text.delta", "agent.text.end"})


class UserTextEvent(BaseModel):
    """Upstream: user transcript turn sent to the agent bridge."""

    event: Literal["user.text"] = "user.text"
    turn_id: int
    text: str = Field(max_length=65536)
    final: bool = True


class UserInterruptEvent(BaseModel):
    """Upstream: user interrupted the current agent turn."""

    event: Literal["user.interrupted"] = "user.interrupted"
    turn_id: int


class AgentTextDeltaEvent(BaseModel):
    """Downstream: incremental agent text chunk for a turn."""

    event: Literal["agent.text.delta"] = "agent.text.delta"
    turn_id: int
    text: str = Field(max_length=4096)


class AgentTextEndEvent(BaseModel):
    """Downstream: agent has finished emitting text for a turn."""

    event: Literal["agent.text.end"] = "agent.text.end"
    turn_id: int


# ── v2 events ─────────────────────────────────────────────


class ErrorEvent(BaseModel):
    """Upstream: reports an error condition to the bridge."""

    event: Literal["error"] = "error"
    call_id: str
    severity: Literal["warn", "error", "fatal"]
    source: Literal["stt", "tts", "transport", "internal"]
    code: str
    message: str
    retriable: bool = False


class MetricEvent(BaseModel):
    """Upstream: periodic metric snapshot."""

    event: Literal["metric"] = "metric"
    call_id: str
    ttfa_ms: float | None = None
    asr_p95_ms: float | None = None
    tts_p95_ms: float | None = None
    turns: int = 0
    cost_usd_so_far: float | None = None


# ── v2 verbs (runner -> worker) ───────────────────────────


class AgentSayVerb(BaseModel):
    """Runner verb: speak verbatim text via TTS."""

    event: Literal["agent.say"] = "agent.say"
    text: str
    interrupt_current: bool = False


class AgentTransferVerb(BaseModel):
    """Runner verb: transfer the call."""

    event: Literal["agent.transfer"] = "agent.transfer"
    remove: dict | None = None
    add: dict
    mode: Literal["cold", "warm"] = "cold"
    warm_handoff_ms: int | None = None


class AgentEndCallVerb(BaseModel):
    """Runner verb: end the call."""

    event: Literal["agent.end_call"] = "agent.end_call"
    reason: str | None = None


class AgentDispatchVerb(BaseModel):
    """Runner verb: dispatch to another runner."""

    event: Literal["agent.dispatch"] = "agent.dispatch"
    runner_url: str
    voice_profile_id: str
    metadata: dict = Field(default_factory=dict)


class AgentAddParticipantVerb(BaseModel):
    """Runner verb: add a participant to the room."""

    event: Literal["agent.add_participant"] = "agent.add_participant"
    type: str
    config: dict = Field(default_factory=dict)


class AgentRemoveParticipantVerb(BaseModel):
    """Runner verb: remove a participant from the room."""

    event: Literal["agent.remove_participant"] = "agent.remove_participant"
    participant_id: str


class AgentMergeVerb(BaseModel):
    """Runner verb: merge sessions."""

    event: Literal["agent.merge"] = "agent.merge"
    secondary_session_ids: list[str]
    drop_participants: list[str] = Field(default_factory=list)


# ── v2 handshake frames ───────────────────────────────────


class HelloEvent(BaseModel):
    """Runner -> worker: advertises supported protocol version
    + events/verbs."""

    event: Literal["hello"] = "hello"
    protocol_version: int
    supported_events: list[str] = Field(default_factory=list)
    supported_verbs: list[str] = Field(default_factory=list)


class HelloAckEvent(BaseModel):
    """Worker -> runner: confirms version, provides call context."""

    event: Literal["hello.ack"] = "hello.ack"
    protocol_version: int
    negotiated_events: list[str] = Field(default_factory=list)
    negotiated_verbs: list[str] = Field(default_factory=list)
    call_id: str
    session_id: str
    job_id: str
    room_id: str


# ── union + dispatch ──────────────────────────────────────

BridgeEvent = Union[
    UserTextEvent,
    UserInterruptEvent,
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    ErrorEvent,
    MetricEvent,
    AgentSayVerb,
    AgentTransferVerb,
    AgentEndCallVerb,
    AgentDispatchVerb,
    AgentAddParticipantVerb,
    AgentRemoveParticipantVerb,
    AgentMergeVerb,
    HelloEvent,
    HelloAckEvent,
]

_TYPE_MAP: dict[str, type[BaseModel]] = {
    "user.text": UserTextEvent,
    "user.interrupted": UserInterruptEvent,
    "agent.text.delta": AgentTextDeltaEvent,
    "agent.text.end": AgentTextEndEvent,
    "error": ErrorEvent,
    "metric": MetricEvent,
    "agent.say": AgentSayVerb,
    "agent.transfer": AgentTransferVerb,
    "agent.end_call": AgentEndCallVerb,
    "agent.dispatch": AgentDispatchVerb,
    "agent.add_participant": AgentAddParticipantVerb,
    "agent.remove_participant": AgentRemoveParticipantVerb,
    "agent.merge": AgentMergeVerb,
    "hello": HelloEvent,
    "hello.ack": HelloAckEvent,
}


def parse_event(raw: dict[str, Any]) -> BridgeEvent:
    """Parse a raw decoded JSON dict into a typed bridge event.

    Raises ValueError if the event type is unknown.
    """
    et = raw.get("event")
    cls = _TYPE_MAP.get(et) if isinstance(et, str) else None
    if cls is None:
        raise ValueError(f"unknown event type: {et}")
    return cls.model_validate(raw)  # type: ignore[return-value]
