"""LiveKit participant adapter (stub).

Real implementation lands when multi-party LiveKit support is wired in.
For Phase 1 this is a placeholder that satisfies the
``ParticipantAdapter`` Protocol.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import ClassVar

from supervoice.orchestrator.room import (
    ParticipantHandle,
    ParticipantType,
    RoomEngine,
    RoomHandle,
)


@dataclass
class LiveKitAdapter:
    """Stub LiveKit adapter — raises on ``attach``, no-ops on ``detach``."""

    participant_id: str
    config: dict
    type: ClassVar[ParticipantType] = "livekit"

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        raise NotImplementedError(
            "LiveKitAdapter.attach lands with multi-party LiveKit support"
        )

    async def detach(self) -> None:
        # No-op for stub; real impl will close the LiveKit participant.
        return
