"""POST /v1/dispatch — entry point for incoming/outgoing call dispatch.

See design.md §6 and tasks.md Task 15. The endpoint:

1. Resolves the auth context (tenant_id) via the auth middleware.
2. Honors ``Idempotency-Key`` by replaying the previous response body.
3. Looks up the per-tenant agent config keyed by ``to_number``.
4. Creates a session, allocates a room, attaches the SIP participant,
   and dispatches a worker via :class:`WorkerDispatcher`.
5. Returns the room join info + a synthesized SDP answer.
"""

from __future__ import annotations

import uuid
from typing import Any, Literal

from fastapi import APIRouter, Depends, HTTPException, Request, status
from loguru import logger
from pydantic import BaseModel, ConfigDict, Field

from supervoice.orchestrator.api.auth import AuthContext, get_auth_context
from supervoice.orchestrator.room.engine import RoomEngine, RoomOpts
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session
from supervoice.orchestrator.worker_registry.dispatch import WorkerDispatcher

from .dependencies import (
    AgentConfig,
    NumberMappingCache,
    get_idempotency_key,
    get_mapping_cache,
    get_room_engine,
    get_session_registry,
    get_worker_dispatcher,
)


router = APIRouter(prefix="/v1", tags=["dispatch"])


Direction = Literal["inbound", "outbound"]


class DispatchRequest(BaseModel):
    """Body schema for POST /v1/dispatch."""

    model_config = ConfigDict(extra="forbid")

    direction: Direction
    from_number: str = Field(min_length=1)
    to_number: str = Field(min_length=1)
    sdp_offer: str = Field(min_length=1)
    metadata: dict[str, Any] = Field(default_factory=dict)
    external_call_id: str | None = None
    callback_url: str | None = None
    credentials: dict[str, Any] | None = None


class RoomJoin(BaseModel):
    """Room connection info returned to the SIP front-end."""

    url: str
    token: str
    name: str


class DispatchResponse(BaseModel):
    """Body schema for the successful 201 response."""

    session_id: str
    state: str
    room: RoomJoin
    sdp_answer: str
    state_url: str
    external_call_id: str | None = None


def _room_join_from_handle(room_handle: object) -> RoomJoin:
    """Synthesize a :class:`RoomJoin` from a generic room handle.

    For the in-process engine there is no real URL/token; the LiveKit
    engine populates these on ``engine_handle``. Stream G provides the
    concrete shape — for V1 we surface whatever is present, falling back
    to placeholders.
    """
    room_id = getattr(room_handle, "room_id", "unknown")
    engine_handle = getattr(room_handle, "engine_handle", None)
    url = getattr(engine_handle, "url", None) or f"in-process://{room_id}"
    token = getattr(engine_handle, "token", None) or "stub-token"
    name = getattr(engine_handle, "name", None) or room_id
    return RoomJoin(url=url, token=token, name=name)


def _synthesize_sdp_answer(participant: object) -> str:
    """Return the SDP answer published by the engine, or a stub.

    The LiveKit-backed SIP integration returns a real answer via the
    participant's ``engine_handle``; the in-process engine has no media
    plane and returns a deterministic placeholder.
    """
    engine_handle = getattr(participant, "engine_handle", None)
    if isinstance(engine_handle, dict):
        ans = engine_handle.get("sdp_answer")
        if isinstance(ans, str) and ans:
            return ans
    return "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n"


@router.post(
    "/dispatch",
    response_model=DispatchResponse,
    status_code=status.HTTP_201_CREATED,
)
async def handle_dispatch(
    body: DispatchRequest,
    request: Request,
    auth: AuthContext = Depends(get_auth_context),
    cache: NumberMappingCache = Depends(get_mapping_cache),
    room_engine: RoomEngine = Depends(get_room_engine),
    dispatcher: WorkerDispatcher = Depends(get_worker_dispatcher),
    registry: SessionRegistry = Depends(get_session_registry),
    idempotency_key: str | None = Depends(get_idempotency_key),
) -> DispatchResponse:
    """Dispatch a new call: room + SIP participant + worker."""
    # Step 1: Idempotency replay (best-effort in-memory store)
    if not hasattr(request.app.state, "idempotency"):
        request.app.state.idempotency = {}
    idem_store: dict[tuple[str, str], tuple[dict, DispatchResponse]] = (
        request.app.state.idempotency
    )

    if idempotency_key is not None:
        key = (auth.tenant_id, idempotency_key)
        prior = idem_store.get(key)
        if prior is not None:
            prior_body, prior_response = prior
            if prior_body == body.model_dump():
                return prior_response
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail="idempotency_key_conflict",
            )

    # Step 2: Mapping lookup
    agent: AgentConfig | None = await cache.get(
        tenant_id=auth.tenant_id, to_number=body.to_number
    )
    if agent is None:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="no_agent_configured_for_number",
        )

    # Step 3: Create session
    session_id = f"s-{uuid.uuid4().hex[:16]}"
    combined_metadata: dict[str, Any] = {
        **(agent.metadata or {}),
        **body.metadata,
        "from_number": body.from_number,
        "to_number": body.to_number,
        "direction": body.direction,
    }
    session = Session(
        session_id=session_id,
        tenant_id=auth.tenant_id,
        metadata=combined_metadata,
        external_call_id=body.external_call_id,
        callback_url=body.callback_url,
    )
    await registry.register(session)

    # Step 4: Allocate room
    room_handle = await room_engine.create_room(
        RoomOpts(session_id=session_id, metadata=combined_metadata)
    )
    session.room_handle = room_handle

    # Step 5: Attach SIP participant
    sip_participant = await room_engine.add_media_participant(
        room_handle,
        "sip",
        {
            "sdp_offer": body.sdp_offer,
            "direction": body.direction,
            "from_number": body.from_number,
            "to_number": body.to_number,
        },
    )
    sdp_answer = _synthesize_sdp_answer(sip_participant)
    room_join = _room_join_from_handle(room_handle)

    # Step 6: Dispatch worker
    dispatch_result = await dispatcher.dispatch(
        session_id=session_id,
        room=room_join.model_dump(),
        voice_profile_id=agent.voice_profile_id,
        runner_url=agent.runner_url,
        agent_secret=agent.agent_secret,
        metadata=combined_metadata,
        pool="default",
    )

    if not dispatch_result.accepted:
        session.transition("rejected")
        logger.info(
            "dispatch rejected session_id=%s reason=%s",
            session_id,
            dispatch_result.reason,
        )
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail=dispatch_result.reason or "no_worker_available",
        )

    session.job_id = dispatch_result.job_id
    session.transition("ringing")

    response = DispatchResponse(
        session_id=session_id,
        state=session.state,
        room=room_join,
        sdp_answer=sdp_answer,
        state_url=f"/v1/sessions/{session_id}",
        external_call_id=body.external_call_id,
    )

    if idempotency_key is not None:
        idem_store[(auth.tenant_id, idempotency_key)] = (
            body.model_dump(),
            response,
        )

    return response


__all__ = ["DispatchRequest", "DispatchResponse", "RoomJoin", "router"]
