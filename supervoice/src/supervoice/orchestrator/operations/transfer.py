"""Transfer operation — add a new participant, optionally drop the worker.

See design.md §2. Supports cold (drop immediately) and warm (overlap for
``warm_handoff_ms``) handoff modes. The actual SIP/agent media handling
is delegated to the room engine.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import Any, Literal

from supervoice.orchestrator.room.engine import (
    ParticipantHandle,
    RoomEngine,
    RoomHandle,
)


Mode = Literal["cold", "warm"]
TargetType = Literal["sip", "agent", "webrtc", "livekit"]


@dataclass(frozen=True)
class TransferTarget:
    """Destination spec for a transfer."""

    type: TargetType
    config: dict[str, Any]


@dataclass(frozen=True)
class TransferResult:
    """Outcome of a single transfer."""

    added_participant_id: str
    mode: Mode
    removed_participant_id: str | None = None


async def perform_transfer(
    *,
    engine: RoomEngine,
    room: RoomHandle,
    target: TransferTarget,
    mode: Mode = "cold",
    warm_handoff_ms: int = 0,
    drop_participant: ParticipantHandle | None = None,
) -> TransferResult:
    """Execute a transfer on ``room``.

    - Adds a new participant matching ``target``.
    - In warm mode, sleeps ``warm_handoff_ms`` before dropping the source.
    - If ``drop_participant`` is provided, it is removed after the handoff.
    """
    engine_target_type = "sip" if target.type == "agent" else target.type
    added = await engine.add_media_participant(
        room, engine_target_type, target.config
    )

    if mode == "warm" and warm_handoff_ms > 0:
        await asyncio.sleep(warm_handoff_ms / 1000.0)

    removed_id: str | None = None
    if drop_participant is not None:
        await engine.remove_participant(room, drop_participant)
        removed_id = drop_participant.participant_id

    return TransferResult(
        added_participant_id=added.participant_id,
        mode=mode,
        removed_participant_id=removed_id,
    )


__all__ = [
    "Mode",
    "TargetType",
    "TransferResult",
    "TransferTarget",
    "perform_transfer",
]
