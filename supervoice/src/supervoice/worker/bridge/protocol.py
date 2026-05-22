"""Bridge wire protocol v1.

Defines the four event types exchanged with the remote Agent Bridge:
two upstream (user -> bridge) and two downstream (bridge -> user).

This wire format is frozen; any change requires a version bump.
"""

from __future__ import annotations

from typing import Any, Literal, Union

from pydantic import BaseModel, Field


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


BridgeEvent = Union[
    UserTextEvent,
    UserInterruptEvent,
    AgentTextDeltaEvent,
    AgentTextEndEvent,
]

_TYPE_MAP: dict[str, type[BaseModel]] = {
    "user.text": UserTextEvent,
    "user.interrupted": UserInterruptEvent,
    "agent.text.delta": AgentTextDeltaEvent,
    "agent.text.end": AgentTextEndEvent,
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
