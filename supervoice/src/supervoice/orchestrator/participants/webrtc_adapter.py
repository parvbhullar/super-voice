"""WebRTC participant adapter (V1 stub-grade).

V1 wraps the existing WebRTC transport path but does not perform actual
SDP signaling here — that's coordinated at a higher integration level
(FastAPI WS endpoint, lands in a later task). For Phase 1, ``attach``
delegates to the ``RoomEngine`` and returns the resulting
``ParticipantHandle``. Audio actually flowing through is a Phase 5
concern.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import ClassVar

from supervoice.orchestrator.room import (
    ParticipantHandle,
    ParticipantType,
    RoomEngine,
    RoomHandle,
)


@dataclass
class WebRtcAdapter:
    """V1 stub-grade WebRTC adapter — engine delegation only."""

    participant_id: str
    config: dict
    type: ClassVar[ParticipantType] = "webrtc"
    _attached_handle: ParticipantHandle | None = field(
        default=None, init=False, repr=False
    )

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        handle = await engine.add_media_participant(room, "webrtc", self.config)
        self._attached_handle = handle
        return handle

    async def detach(self) -> None:
        # Best-effort; real impl will close the WebRTC peer.
        # Swallow errors per ParticipantAdapter contract.
        self._attached_handle = None
        return
