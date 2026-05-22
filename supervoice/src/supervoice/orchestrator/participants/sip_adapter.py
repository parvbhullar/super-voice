"""SIP participant adapter (stub).

Real implementation lands in Phase 5 (Task 28) using LiveKit-SIP for
inbound/outbound SIP trunks. For now this is a placeholder that satisfies
the ``ParticipantAdapter`` Protocol so the orchestrator wiring can be
exercised in tests.
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
class SipAdapter:
    """Stub SIP adapter — raises on ``attach``, no-ops on ``detach``."""

    participant_id: str
    config: dict
    type: ClassVar[ParticipantType] = "sip"

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        raise NotImplementedError("SipAdapter.attach lands in Phase 5 / Task 28")

    async def detach(self) -> None:
        # No-op for stub; real impl will close the SIP leg.
        return
