"""RoomEngine Protocol and related value types.

See design.md §1.2 for the contract. The Protocol abstracts the media
backend (in-process for dev, LiveKit for production multi-party) so the
session orchestrator can swap engines without leaking implementation
details.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, Protocol

ParticipantType = Literal["sip", "webrtc", "livekit"]


@dataclass(frozen=True)
class RoomOpts:
    """Options for creating a new room."""

    session_id: str
    metadata: dict
    max_participants: int = 16
    empty_timeout_s: int = 30


@dataclass(frozen=True)
class RoomHandle:
    """Opaque handle to a room owned by a RoomEngine."""

    room_id: str
    engine_type: str
    engine_handle: object


@dataclass(frozen=True)
class ParticipantHandle:
    """Opaque handle to a participant within a room."""

    participant_id: str
    type: ParticipantType
    engine_handle: object


class RoomEngine(Protocol):
    """Abstract media room backend.

    Implementations: ``InProcessRoomEngine`` (dev, 1:1), and a future
    ``LiveKitRoomEngine`` (production, multi-party).
    """

    async def create_room(self, opts: RoomOpts) -> RoomHandle: ...

    async def get_room(self, room_id: str) -> RoomHandle | None: ...

    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None: ...

    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle: ...

    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None: ...

    async def mute_participant(
        self, room: RoomHandle, participant: ParticipantHandle, muted: bool
    ) -> None: ...

    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]: ...
