"""Merge operation — fold N secondary sessions' participants into a primary.

See design.md §2 and §8.3. Returns a per-secondary outcome list so the
HTTP layer can serialize a 207 Multi-Status response.

For V1 the in-process engine raises ``NotImplementedError`` on
``move_participants`` — the merge logic surfaces a structured failure
rather than crashing, so the caller can map it to a 409.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

from supervoice.orchestrator.room.engine import RoomEngine, RoomHandle
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session


@dataclass(frozen=True)
class SecondaryOutcome:
    """Result for one secondary session in a merge operation."""

    session_id: str
    status: Literal["merged", "failed", "not_found"]
    moved_participant_ids: list[str]
    error: str | None = None


@dataclass(frozen=True)
class MergeResult:
    """Aggregate outcome of a merge."""

    primary_session_id: str
    outcomes: list[SecondaryOutcome]


async def perform_merge(
    *,
    engine: RoomEngine,
    registry: SessionRegistry,
    tenant_id: str,
    primary_session_id: str,
    secondary_session_ids: list[str],
    drop_participants: list[dict[str, Any]] | None = None,
) -> MergeResult:
    """Move participants from secondaries into the primary session's room.

    Drops any participants matched by ``drop_participants`` (list of
    ``{"session_id": ..., "participant_id": ...}`` entries).
    """
    drop_set: set[tuple[str, str]] = {
        (d["session_id"], d["participant_id"])
        for d in (drop_participants or [])
        if "session_id" in d and "participant_id" in d
    }

    primary: Session | None = await registry.get(
        primary_session_id, tenant_id=tenant_id
    )
    if primary is None or primary.room_handle is None:
        raise LookupError(f"primary session not found: {primary_session_id}")
    primary_room: RoomHandle = primary.room_handle  # type: ignore[assignment]

    outcomes: list[SecondaryOutcome] = []
    for sid in secondary_session_ids:
        secondary = await registry.get(sid, tenant_id=tenant_id)
        if secondary is None or secondary.room_handle is None:
            outcomes.append(
                SecondaryOutcome(
                    session_id=sid,
                    status="not_found",
                    moved_participant_ids=[],
                    error="session_not_found",
                )
            )
            continue
        secondary_room: RoomHandle = secondary.room_handle  # type: ignore[assignment]
        # Pull participant list off the engine_handle (engine-specific).
        candidates = list(
            getattr(secondary_room.engine_handle, "participants", []) or []
        )
        movable = [
            p
            for p in candidates
            if (sid, p.participant_id) not in drop_set
        ]
        try:
            moved = await engine.move_participants(
                secondary_room, primary_room, movable
            )
        except NotImplementedError:
            outcomes.append(
                SecondaryOutcome(
                    session_id=sid,
                    status="failed",
                    moved_participant_ids=[],
                    error="move_not_supported",
                )
            )
            continue
        except Exception as e:  # noqa: BLE001 — propagate as structured outcome
            outcomes.append(
                SecondaryOutcome(
                    session_id=sid,
                    status="failed",
                    moved_participant_ids=[],
                    error=str(e),
                )
            )
            continue

        await engine.destroy_room(secondary_room, graceful=True)
        if secondary.state not in {
            "ended",
            "rejected",
            "timed_out",
            "failed",
        }:
            secondary.transition("ended")
        outcomes.append(
            SecondaryOutcome(
                session_id=sid,
                status="merged",
                moved_participant_ids=[p.participant_id for p in moved],
            )
        )

    return MergeResult(
        primary_session_id=primary_session_id, outcomes=outcomes
    )


__all__ = ["MergeResult", "SecondaryOutcome", "perform_merge"]
