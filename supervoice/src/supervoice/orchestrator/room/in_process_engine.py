"""In-process RoomEngine implementation for dev/test.

Limits:
- 1:1 rooms only (max_participants defaults to 16 via RoomOpts but the engine
  is primarily used for caller<->agent pairs).
- No audio mixing/muting — ``mute_participant`` records state only.
- ``move_participants`` is not supported (use LiveKit engine for multi-party).
"""

from __future__ import annotations

import logging
import uuid
from dataclasses import dataclass, field

from .engine import (
    ParticipantHandle,
    ParticipantType,
    RoomHandle,
    RoomOpts,
)

logger = logging.getLogger(__name__)

_ENGINE_TYPE = "in_process"


@dataclass
class _Room:
    """Private mutable room state held inside ``RoomHandle.engine_handle``."""

    room_id: str
    opts: RoomOpts
    participants: list[ParticipantHandle] = field(default_factory=list)
    muted: dict[str, bool] = field(default_factory=dict)


class InProcessRoomEngine:
    """In-memory RoomEngine for development and unit tests."""

    def __init__(self) -> None:
        self._rooms: dict[str, _Room] = {}

    async def create_room(self, opts: RoomOpts) -> RoomHandle:
        room_id = str(uuid.uuid4())
        room = _Room(room_id=room_id, opts=opts)
        self._rooms[room_id] = room
        return RoomHandle(
            room_id=room_id, engine_type=_ENGINE_TYPE, engine_handle=room
        )

    async def get_room(self, room_id: str) -> RoomHandle | None:
        room = self._rooms.get(room_id)
        if room is None:
            return None
        return RoomHandle(
            room_id=room_id, engine_type=_ENGINE_TYPE, engine_handle=room
        )

    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None:
        logger.info(
            "destroy_room room_id=%s graceful=%s", room.room_id, graceful
        )
        self._rooms.pop(room.room_id, None)

    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle:
        internal = self._rooms.get(room.room_id)
        if internal is None:
            raise RuntimeError(f"unknown room: {room.room_id}")
        if len(internal.participants) >= internal.opts.max_participants:
            raise RuntimeError("max participants exceeded")
        participant = ParticipantHandle(
            participant_id=str(uuid.uuid4()),
            type=type,
            engine_handle=config,
        )
        internal.participants.append(participant)
        internal.muted[participant.participant_id] = False
        return participant

    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None:
        internal = self._rooms.get(room.room_id)
        if internal is None:
            return
        internal.participants = [
            p
            for p in internal.participants
            if p.participant_id != participant.participant_id
        ]
        internal.muted.pop(participant.participant_id, None)

    async def mute_participant(
        self,
        room: RoomHandle,
        participant: ParticipantHandle,
        muted: bool,
    ) -> None:
        internal = self._rooms.get(room.room_id)
        if internal is None:
            return
        internal.muted[participant.participant_id] = muted

    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]:
        raise NotImplementedError(
            "in_process engine is 1:1; use livekit engine for multi-party"
        )
