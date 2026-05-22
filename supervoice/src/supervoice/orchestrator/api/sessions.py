"""Session-scoped REST endpoints: GET, end, transfer, merge.

See tasks.md Tasks 17, 18, 19 and design.md §2, §6, §8.
"""

from __future__ import annotations

from typing import Any, Literal

from fastapi import APIRouter, Depends, HTTPException, Request, status
from loguru import logger
from pydantic import BaseModel, ConfigDict, Field

from supervoice.orchestrator.api.auth import AuthContext, get_auth_context
from supervoice.orchestrator.operations.merge import perform_merge
from supervoice.orchestrator.operations.transfer import (
    TransferTarget,
    perform_transfer,
)
from supervoice.orchestrator.room.engine import RoomEngine, RoomHandle
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session

from .dependencies import (
    get_room_engine,
    get_session_registry,
)


router = APIRouter(prefix="/v1", tags=["sessions"])


# --- Schemas --------------------------------------------------------------


class ParticipantInfo(BaseModel):
    participant_id: str
    type: str


class SessionResponse(BaseModel):
    session_id: str
    tenant_id: str
    state: str
    external_call_id: str | None = None
    job_id: str | None = None
    room_id: str | None = None
    participants: list[ParticipantInfo] = Field(default_factory=list)


class EndResponse(BaseModel):
    session_id: str
    state: str


class TransferTargetBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    type: Literal["sip", "agent", "webrtc", "livekit"]
    config: dict[str, Any] = Field(default_factory=dict)


class TransferRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    to: TransferTargetBody
    mode: Literal["cold", "warm"] = "cold"
    warm_handoff_ms: int = Field(default=0, ge=0)
    drop_participant_id: str | None = None


class TransferResponse(BaseModel):
    session_id: str
    added_participant_id: str
    removed_participant_id: str | None = None
    mode: str


class MergeRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    primary_session_id: str = Field(min_length=1)
    secondary_session_ids: list[str] = Field(min_length=1)
    drop_participants: list[dict[str, Any]] | None = None


class MergeOutcomeBody(BaseModel):
    session_id: str
    status: str
    moved_participant_ids: list[str]
    error: str | None = None


class MergeResponse(BaseModel):
    primary_session_id: str
    outcomes: list[MergeOutcomeBody]


# --- Helpers --------------------------------------------------------------


async def _get_session_or_404(
    registry: SessionRegistry, *, session_id: str, tenant_id: str
) -> Session:
    session = await registry.get(session_id, tenant_id=tenant_id)
    if session is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="session_not_found",
        )
    return session


def _participants_of(session: Session) -> list[ParticipantInfo]:
    room = session.room_handle
    if room is None:
        return []
    engine_handle = getattr(room, "engine_handle", None)
    parts = getattr(engine_handle, "participants", None) or []
    return [
        ParticipantInfo(participant_id=p.participant_id, type=str(p.type))
        for p in parts
    ]


# --- Routes ---------------------------------------------------------------


@router.get(
    "/sessions/{session_id}",
    response_model=SessionResponse,
)
async def get_session(
    session_id: str,
    auth: AuthContext = Depends(get_auth_context),
    registry: SessionRegistry = Depends(get_session_registry),
) -> SessionResponse:
    """Return the current session snapshot for the authenticated tenant."""
    session = await _get_session_or_404(
        registry, session_id=session_id, tenant_id=auth.tenant_id
    )
    room_id = (
        getattr(session.room_handle, "room_id", None)
        if session.room_handle is not None
        else None
    )
    return SessionResponse(
        session_id=session.session_id,
        tenant_id=session.tenant_id,
        state=session.state,
        external_call_id=session.external_call_id,
        job_id=session.job_id,
        room_id=room_id,
        participants=_participants_of(session),
    )


@router.post(
    "/sessions/{session_id}/end",
    response_model=EndResponse,
)
async def end_session(
    session_id: str,
    auth: AuthContext = Depends(get_auth_context),
    registry: SessionRegistry = Depends(get_session_registry),
    room_engine: RoomEngine = Depends(get_room_engine),
) -> EndResponse:
    """Mark a session as draining and tear down its room.

    V1: the worker is not signalled directly — it observes the drain via
    its existing job-completion path or times out. Task 21 wires a real
    ``end_job`` on the dispatcher.
    """
    session = await _get_session_or_404(
        registry, session_id=session_id, tenant_id=auth.tenant_id
    )
    if session.state not in {
        "ended",
        "rejected",
        "timed_out",
        "failed",
    }:
        try:
            session.transition("ended")
        except ValueError as e:
            logger.warning("end_session: invalid transition: %s", e)
    if session.room_handle is not None:
        try:
            await room_engine.destroy_room(
                session.room_handle,  # type: ignore[arg-type]
                graceful=True,
            )
        except Exception as e:  # noqa: BLE001 — best-effort cleanup
            logger.warning("destroy_room during end failed: %s", e)
    await registry.mark_draining(session_id, tenant_id=auth.tenant_id)
    return EndResponse(session_id=session_id, state=session.state)


@router.post(
    "/sessions/{session_id}/transfer",
    response_model=TransferResponse,
)
async def transfer_session(
    session_id: str,
    body: TransferRequest,
    auth: AuthContext = Depends(get_auth_context),
    registry: SessionRegistry = Depends(get_session_registry),
    room_engine: RoomEngine = Depends(get_room_engine),
) -> TransferResponse:
    """Add a new participant to the session's room (cold/warm handoff)."""
    session = await _get_session_or_404(
        registry, session_id=session_id, tenant_id=auth.tenant_id
    )
    if session.room_handle is None:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="session_has_no_room",
        )
    room: RoomHandle = session.room_handle  # type: ignore[assignment]

    drop_participant = None
    if body.drop_participant_id is not None:
        engine_handle = getattr(room, "engine_handle", None)
        for p in getattr(engine_handle, "participants", None) or []:
            if p.participant_id == body.drop_participant_id:
                drop_participant = p
                break
        if drop_participant is None:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail="drop_participant_not_found",
            )

    try:
        result = await perform_transfer(
            engine=room_engine,
            room=room,
            target=TransferTarget(type=body.to.type, config=body.to.config),
            mode=body.mode,
            warm_handoff_ms=body.warm_handoff_ms,
            drop_participant=drop_participant,
        )
    except NotImplementedError as e:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail=str(e) or "transfer_not_supported",
        ) from e

    return TransferResponse(
        session_id=session.session_id,
        added_participant_id=result.added_participant_id,
        removed_participant_id=result.removed_participant_id,
        mode=result.mode,
    )


@router.post(
    "/sessions/merge",
    response_model=MergeResponse,
    status_code=status.HTTP_207_MULTI_STATUS,
)
async def merge_sessions(
    body: MergeRequest,
    request: Request,
    auth: AuthContext = Depends(get_auth_context),
    registry: SessionRegistry = Depends(get_session_registry),
    room_engine: RoomEngine = Depends(get_room_engine),
) -> MergeResponse:
    """Move participants from secondaries into the primary's room."""
    try:
        result = await perform_merge(
            engine=room_engine,
            registry=registry,
            tenant_id=auth.tenant_id,
            primary_session_id=body.primary_session_id,
            secondary_session_ids=body.secondary_session_ids,
            drop_participants=body.drop_participants,
        )
    except LookupError as e:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=str(e),
        ) from e

    # If every outcome failed with move-not-supported, surface a clean
    # 409 so callers don't have to peek inside the body.
    if result.outcomes and all(
        o.status == "failed" and o.error == "move_not_supported"
        for o in result.outcomes
    ):
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="merge_not_supported_by_engine",
        )

    return MergeResponse(
        primary_session_id=result.primary_session_id,
        outcomes=[
            MergeOutcomeBody(
                session_id=o.session_id,
                status=o.status,
                moved_participant_ids=list(o.moved_participant_ids),
                error=o.error,
            )
            for o in result.outcomes
        ],
    )


__all__ = [
    "EndResponse",
    "MergeRequest",
    "MergeResponse",
    "SessionResponse",
    "TransferRequest",
    "TransferResponse",
    "router",
]
