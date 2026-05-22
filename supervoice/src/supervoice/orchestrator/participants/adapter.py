"""ParticipantAdapter Protocol.

See design.md §1.3. Adapters bridge a leg (SIP/WebRTC/LiveKit participant)
to a ``RoomEngine``. The orchestrator owns adapters one-per-participant and
calls ``attach`` / ``detach`` around the participant lifecycle.
"""

from __future__ import annotations

from typing import Protocol

from supervoice.orchestrator.room import (
    ParticipantHandle,
    ParticipantType,
    RoomEngine,
    RoomHandle,
)


class ParticipantAdapter(Protocol):
    """Protocol for participant adapters.

    Implementations MUST:
    - Expose ``type`` (matches ``ParticipantType``) as a class attribute.
    - Expose ``participant_id`` (caller-supplied logical id).
    - Be idempotent on ``participant_id`` where the underlying engine
      supports it (V1 engines are not — attach twice yields two engine
      participants; callers must avoid double-attach).
    """

    type: ParticipantType
    participant_id: str

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        """Open the leg + plug into the Room."""
        ...

    async def detach(self) -> None:
        """Close cleanly. MUST NOT raise; log errors and swallow."""
        ...
