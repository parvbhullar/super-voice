"""SIP participant adapter via RoomEngine delegation.

Delegates to ``engine.add_media_participant(room, "sip", config)`` so the
concrete engine (LiveKit-SIP in production, in-process for dev) handles the
actual SIP leg creation. The adapter stores the returned
``ParticipantHandle`` and tears it down on ``detach``.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import ClassVar

from supervoice.orchestrator.room import (
    ParticipantHandle,
    ParticipantType,
    RoomEngine,
    RoomHandle,
)

logger = logging.getLogger(__name__)


@dataclass
class SipAdapter:
    """SIP adapter — delegates attach/detach to the RoomEngine."""

    participant_id: str
    config: dict
    type: ClassVar[ParticipantType] = "sip"

    _attached_handle: ParticipantHandle | None = field(
        default=None, init=False, repr=False
    )
    _room: RoomHandle | None = field(default=None, init=False, repr=False)
    _engine: RoomEngine | None = field(default=None, init=False, repr=False)

    async def attach(self, room: RoomHandle, engine: RoomEngine) -> ParticipantHandle:
        """Add a SIP media participant to the room via the engine.

        Stores the room/engine references so ``detach`` can remove the
        participant without requiring them to be passed again.
        """
        self._room = room
        self._engine = engine
        handle = await engine.add_media_participant(room, "sip", self.config)
        self._attached_handle = handle
        return handle

    async def detach(self) -> None:
        """Remove the SIP participant from the room (best-effort).

        Safe to call even if ``attach`` was never invoked — the method
        is a no-op in that case. Exceptions from the engine are logged
        and suppressed so detach never raises.
        """
        if self._attached_handle is None:
            return
        if self._room is not None and self._engine is not None:
            try:
                await self._engine.remove_participant(self._room, self._attached_handle)
            except Exception:
                logger.warning(
                    "SipAdapter.detach: failed to remove participant %s from room %s",
                    self._attached_handle.participant_id,
                    self._room.room_id,
                    exc_info=True,
                )
        self._attached_handle = None
