"""Tests for `LiveKitRoomEngine` using a mocked SDK."""

from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock

import pytest

from supervoice.orchestrator.room.engine import RoomOpts
from supervoice.orchestrator.room.livekit_engine import (
    LiveKitConfig,
    LiveKitRoomEngine,
)


def _make_mock_sdk() -> MagicMock:
    sdk = MagicMock()
    # Request / value classes -- accept any kwargs.
    sdk.CreateRoomRequest = MagicMock
    sdk.DeleteRoomRequest = MagicMock
    sdk.ListRoomsRequest = MagicMock
    sdk.RoomParticipantIdentity = MagicMock
    sdk.CreateSIPParticipantRequest = MagicMock
    sdk.VideoGrants = MagicMock

    room_service = MagicMock()
    room_service.create_room = AsyncMock(return_value=MagicMock())
    room_service.delete_room = AsyncMock(return_value=None)
    list_response = MagicMock()
    list_response.rooms = [MagicMock()]
    room_service.list_rooms = AsyncMock(return_value=list_response)
    room_service.remove_participant = AsyncMock(return_value=None)
    sdk.RoomService = MagicMock(return_value=room_service)

    sip_service = MagicMock()
    sip_info = MagicMock()
    sip_info.participant_id = "p-sip-1"
    sip_info.participant_identity = "sip-abc"
    sip_info.sdp_answer = "v=0\r\nfake"
    sip_service.create_sip_participant = AsyncMock(return_value=sip_info)
    sdk.SIPService = MagicMock(return_value=sip_service)

    # AccessToken: chainable builder returning fake-jwt.
    token = MagicMock()
    token.with_identity.return_value = token
    token.with_grants.return_value = token
    token.to_jwt.return_value = "fake-jwt"
    sdk.AccessToken = MagicMock(return_value=token)

    return sdk


async def test_create_room_calls_sdk():
    sdk = _make_mock_sdk()
    engine = LiveKitRoomEngine(
        LiveKitConfig(server_url="wss://test", api_key="k", api_secret="s"),
        sdk=sdk,
    )
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    assert room.engine_type == "livekit"
    assert room.room_id == "sv-s1"


async def test_add_sip_participant_returns_sdp_answer():
    sdk = _make_mock_sdk()
    engine = LiveKitRoomEngine(
        LiveKitConfig("wss://test", "k", "s"), sdk=sdk
    )
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    p = await engine.add_media_participant(
        room, "sip", {"to_number": "+91123", "sip_trunk_id": "tk-1"}
    )
    assert p.type == "sip"
    assert isinstance(p.engine_handle, dict)
    assert p.engine_handle["sdp_answer"] == "v=0\r\nfake"
    assert p.engine_handle["identity"] == "sip-abc"


async def test_add_webrtc_participant_mints_token():
    sdk = _make_mock_sdk()
    engine = LiveKitRoomEngine(
        LiveKitConfig("wss://test", "k", "s"), sdk=sdk
    )
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    p = await engine.add_media_participant(room, "webrtc", {})
    assert p.type == "webrtc"
    assert isinstance(p.engine_handle, dict)
    assert p.engine_handle["token"] == "fake-jwt"


async def test_remove_and_destroy_idempotent():
    sdk = _make_mock_sdk()
    engine = LiveKitRoomEngine(
        LiveKitConfig("wss://test", "k", "s"), sdk=sdk
    )
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    p = await engine.add_media_participant(room, "webrtc", {})
    await engine.remove_participant(room, p)
    await engine.destroy_room(room)


def test_engine_construction_without_sdk_raises():
    import supervoice.orchestrator.room.livekit_engine as mod

    original_avail = mod._LIVEKIT_AVAILABLE
    original_sdk = mod.livekit_api
    try:
        mod._LIVEKIT_AVAILABLE = False
        mod.livekit_api = None
        with pytest.raises(RuntimeError, match="not installed"):
            LiveKitRoomEngine(LiveKitConfig("wss://test", "k", "s"))
    finally:
        mod._LIVEKIT_AVAILABLE = original_avail
        mod.livekit_api = original_sdk


async def test_move_participants_round_trip():
    sdk = _make_mock_sdk()
    engine = LiveKitRoomEngine(
        LiveKitConfig("wss://test", "k", "s"), sdk=sdk
    )
    room_a = await engine.create_room(
        RoomOpts(session_id="s-a", metadata={})
    )
    room_b = await engine.create_room(
        RoomOpts(session_id="s-b", metadata={})
    )
    p = await engine.add_media_participant(room_a, "webrtc", {})
    moved = await engine.move_participants(room_a, room_b, [p])
    assert len(moved) == 1
    assert moved[0].type == "webrtc"


async def test_get_room_returns_none_when_not_found():
    sdk = _make_mock_sdk()
    # Override list_rooms to return empty list.
    empty = MagicMock()
    empty.rooms = []
    sdk.RoomService.return_value.list_rooms = AsyncMock(return_value=empty)
    engine = LiveKitRoomEngine(
        LiveKitConfig("wss://test", "k", "s"), sdk=sdk
    )
    assert (await engine.get_room("sv-missing")) is None
