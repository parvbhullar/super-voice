import pytest

from supervoice.orchestrator.room import InProcessRoomEngine, RoomOpts


async def test_create_and_get_room():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    assert room.engine_type == "in_process"
    fetched = await engine.get_room(room.room_id)
    assert fetched is not None
    assert fetched.room_id == room.room_id


async def test_get_unknown_room_returns_none():
    engine = InProcessRoomEngine()
    assert (await engine.get_room("nonexistent")) is None


async def test_add_two_participants_ok():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    p1 = await engine.add_media_participant(room, "sip", {})
    p2 = await engine.add_media_participant(room, "webrtc", {})
    assert p1.participant_id != p2.participant_id


async def test_add_participant_over_capacity_raises():
    engine = InProcessRoomEngine()
    room = await engine.create_room(
        RoomOpts(session_id="s1", metadata={}, max_participants=2)
    )
    await engine.add_media_participant(room, "sip", {})
    await engine.add_media_participant(room, "webrtc", {})
    with pytest.raises(RuntimeError, match="max participants"):
        await engine.add_media_participant(room, "livekit", {})


async def test_remove_participant():
    engine = InProcessRoomEngine()
    room = await engine.create_room(
        RoomOpts(session_id="s1", metadata={}, max_participants=2)
    )
    p1 = await engine.add_media_participant(room, "sip", {})
    await engine.add_media_participant(room, "webrtc", {})
    await engine.remove_participant(room, p1)
    # After remove, can add another up to capacity
    await engine.add_media_participant(room, "livekit", {})


async def test_mute_participant_no_op():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    p = await engine.add_media_participant(room, "sip", {})
    await engine.mute_participant(room, p, True)
    await engine.mute_participant(room, p, False)


async def test_destroy_room():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    await engine.destroy_room(room, graceful=True)
    assert (await engine.get_room(room.room_id)) is None


async def test_move_participants_raises_notimplemented():
    engine = InProcessRoomEngine()
    room_a = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    room_b = await engine.create_room(RoomOpts(session_id="s2", metadata={}))
    p = await engine.add_media_participant(room_a, "sip", {})
    with pytest.raises(NotImplementedError):
        await engine.move_participants(room_a, room_b, [p])
