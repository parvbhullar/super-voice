"""LiveKit-backed `RoomEngine` implementation.

The LiveKit Python SDK (`livekit-api`) is an optional dependency. The module
imports cleanly even when the SDK is absent; instantiating
``LiveKitRoomEngine`` without either the installed SDK or a test-injected
``sdk`` argument raises a clear ``RuntimeError``.

See design.md §1.2 (RoomEngine) and §2.2 (room move latency budget).
"""

from __future__ import annotations

import uuid
from dataclasses import dataclass
from typing import Any

from loguru import logger

from .engine import ParticipantHandle, ParticipantType, RoomHandle, RoomOpts

# Attempt to import LiveKit's server SDK. If unavailable, the engine class
# raises a clear error on instantiation rather than at import time.
try:
    from livekit import api as livekit_api  # type: ignore[import-not-found]

    _LIVEKIT_AVAILABLE = True
except ImportError:  # pragma: no cover - exercised via test-time monkeypatch
    livekit_api = None  # type: ignore[assignment]
    _LIVEKIT_AVAILABLE = False


@dataclass(frozen=True)
class LiveKitConfig:
    """Connection parameters for a LiveKit server."""

    server_url: str
    api_key: str
    api_secret: str


class LiveKitRoomEngine:
    """`RoomEngine` implementation against a LiveKit server.

    V1 scope:
      - ``create_room`` -> ``RoomService.create_room``
      - ``destroy_room`` -> ``RoomService.delete_room``
      - ``add_media_participant(type="sip", ...)`` ->
        ``SIPService.create_sip_participant`` -- returns a
        ``ParticipantHandle`` whose ``engine_handle`` holds the LiveKit
        participant identity and SDP answer.
      - ``add_media_participant(type="webrtc"|"livekit", ...)`` mints a
        token for the caller; caller joins out-of-band. The
        ``engine_handle`` carries ``{identity, token}``.
      - ``remove_participant`` -> ``RoomService.remove_participant``.
      - ``mute_participant`` -- V1 stub (needs track_sid lookup).
      - ``move_participants`` -- remove-from-A + add-to-B sequence.

    Tests inject a mocked SDK so we don't need a real LiveKit server.
    """

    def __init__(
        self, config: LiveKitConfig, *, sdk: Any | None = None
    ) -> None:
        if not _LIVEKIT_AVAILABLE and sdk is None:
            raise RuntimeError(
                "LiveKit SDK is not installed; "
                "install `livekit-api` or pass `sdk` for testing"
            )
        self._config = config
        self._sdk: Any = sdk if sdk is not None else livekit_api
        self._room_service: Any | None = None
        self._sip_service: Any | None = None

    def _room_client(self) -> Any:
        if self._room_service is None:
            self._room_service = self._sdk.RoomService(
                self._config.server_url,
                self._config.api_key,
                self._config.api_secret,
            )
        return self._room_service

    def _sip_client(self) -> Any:
        if self._sip_service is None:
            self._sip_service = self._sdk.SIPService(
                self._config.server_url,
                self._config.api_key,
                self._config.api_secret,
            )
        return self._sip_service

    async def create_room(self, opts: RoomOpts) -> RoomHandle:
        client = self._room_client()
        room_name = f"sv-{opts.session_id}"
        await client.create_room(
            self._sdk.CreateRoomRequest(
                name=room_name,
                empty_timeout=opts.empty_timeout_s,
                max_participants=opts.max_participants,
            )
        )
        return RoomHandle(
            room_id=room_name,
            engine_type="livekit",
            engine_handle={"name": room_name},
        )

    async def get_room(self, room_id: str) -> RoomHandle | None:
        client = self._room_client()
        try:
            rooms = await client.list_rooms(
                self._sdk.ListRoomsRequest(names=[room_id])
            )
            if not rooms.rooms:
                return None
            return RoomHandle(
                room_id=room_id,
                engine_type="livekit",
                engine_handle={"name": room_id},
            )
        except Exception as e:
            logger.warning(f"livekit get_room failed: {e}")
            return None

    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None:
        client = self._room_client()
        try:
            await client.delete_room(
                self._sdk.DeleteRoomRequest(room=room.room_id)
            )
        except Exception as e:
            logger.warning(f"livekit destroy_room failed: {e}")

    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle:
        if type == "sip":
            client = self._sip_client()
            req = self._sdk.CreateSIPParticipantRequest(
                room_name=room.room_id,
                participant_identity=config.get(
                    "participant_identity",
                    f"sip-{uuid.uuid4().hex[:8]}",
                ),
                sip_trunk_id=config.get("sip_trunk_id", ""),
                sip_call_to=config.get("to_number", ""),
            )
            info = await client.create_sip_participant(req)
            return ParticipantHandle(
                participant_id=info.participant_id,
                type="sip",
                engine_handle={
                    "identity": info.participant_identity,
                    "sdp_answer": getattr(info, "sdp_answer", "v=0\r\n"),
                },
            )
        elif type in ("webrtc", "livekit"):
            identity = config.get(
                "participant_identity",
                f"{type}-{uuid.uuid4().hex[:8]}",
            )
            token = self._mint_token(room.room_id, identity)
            return ParticipantHandle(
                participant_id=identity,
                type=type,
                engine_handle={"identity": identity, "token": token},
            )
        else:
            raise ValueError(f"unknown participant type: {type!r}")

    def _mint_token(self, room_name: str, identity: str) -> str:
        token = self._sdk.AccessToken(
            self._config.api_key, self._config.api_secret
        )
        token.with_identity(identity).with_grants(
            self._sdk.VideoGrants(
                room_join=True,
                room=room_name,
                can_publish=True,
                can_subscribe=True,
            )
        )
        return token.to_jwt()

    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None:
        client = self._room_client()
        handle = participant.engine_handle
        identity = (
            handle.get("identity") if isinstance(handle, dict) else None
        )
        try:
            await client.remove_participant(
                self._sdk.RoomParticipantIdentity(
                    room=room.room_id, identity=identity,
                )
            )
        except Exception as e:
            logger.warning(f"remove_participant failed: {e}")

    async def mute_participant(
        self,
        room: RoomHandle,
        participant: ParticipantHandle,
        muted: bool,
    ) -> None:
        # MutePublishedTrack requires a track_sid; participants may have
        # many. V1 stub -- real impl needs a list_participants lookup to
        # resolve the audio track sid.
        logger.info(
            f"mute_participant {participant.participant_id}={muted} -- "
            "V1 stub; real impl needs track_sid lookup"
        )

    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]:
        new_handles: list[ParticipantHandle] = []
        for p in participants:
            await self.remove_participant(from_room, p)
            handle = p.engine_handle
            identity = (
                handle.get("identity") if isinstance(handle, dict) else None
            )
            cfg: dict[str, Any] = {}
            if identity is not None:
                cfg["participant_identity"] = identity
            new_handles.append(
                await self.add_media_participant(to_room, p.type, cfg)
            )
        return new_handles
