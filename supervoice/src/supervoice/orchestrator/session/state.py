"""V2 orchestrator Session model + state machine.

The ``SessionState`` Literal here is distinct from the V1 per-call mutable
``SessionState`` dataclass at ``supervoice.session.state``; this one names
the high-level lifecycle states the orchestrator drives a Session through.

Time source: ``time.monotonic`` is used for ``created_at`` and history
timestamps so durations are immune to wall-clock drift / NTP slews.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Literal


SessionState = Literal[
    "incoming",
    "ringing",
    "connected",
    "rejected",
    "timed_out",
    "failed",
    "ended",
]


_TRANSITIONS: dict[SessionState, set[SessionState]] = {
    "incoming": {"ringing", "rejected", "failed", "ended"},
    "ringing": {"connected", "rejected", "timed_out", "failed", "ended"},
    "connected": {"failed", "ended"},
    "rejected": set(),  # terminal
    "timed_out": set(),  # terminal
    "failed": set(),  # terminal
    "ended": set(),  # terminal
}


@dataclass
class Session:
    """Orchestrator-side session record.

    Holds identity, lifecycle state, and references to the engine/job
    handles that adjacent components own. ``state_history`` is appended to
    on every successful transition (including the initial state) using a
    monotonic clock.
    """

    session_id: str
    tenant_id: str
    metadata: dict
    state: SessionState = "incoming"
    external_call_id: str | None = None
    callback_url: str | None = None
    room_handle: object | None = None
    job_id: str | None = None
    created_at: float = field(default_factory=time.monotonic)
    state_history: list[tuple[SessionState, float]] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.state_history.append((self.state, self.created_at))

    def transition(self, new_state: SessionState) -> None:
        """Move to ``new_state`` if the transition is permitted.

        Raises ``ValueError`` if the current state is terminal or the
        target is not in the allowed set for the current state.
        """
        valid = _TRANSITIONS.get(self.state, set())
        if not valid:
            raise ValueError(
                f"cannot transition from terminal state {self.state!r}"
            )
        if new_state not in valid:
            raise ValueError(
                f"invalid transition {self.state!r} -> {new_state!r} "
                f"(valid: {sorted(valid)})"
            )
        self.state = new_state
        self.state_history.append((new_state, time.monotonic()))
