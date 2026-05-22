from supervoice.orchestrator.room.engine import (
    ParticipantHandle,
    ParticipantType,
    RoomEngine,
    RoomHandle,
    RoomOpts,
)


def test_room_opts_is_frozen():
    import dataclasses

    import pytest

    opts = RoomOpts(session_id="s1", metadata={})
    with pytest.raises(dataclasses.FrozenInstanceError):
        opts.session_id = "s2"  # type: ignore[misc]


def test_room_opts_defaults():
    opts = RoomOpts(session_id="s1", metadata={})
    assert opts.max_participants == 16
    assert opts.empty_timeout_s == 30


def test_participant_type_literal_values():
    values: list[ParticipantType] = ["sip", "webrtc", "livekit"]
    assert values == ["sip", "webrtc", "livekit"]


def test_room_handle_and_participant_handle_construct():
    rh = RoomHandle(room_id="r1", engine_type="in_process", engine_handle=object())
    assert rh.room_id == "r1"
    ph = ParticipantHandle(participant_id="p1", type="sip", engine_handle=object())
    assert ph.type == "sip"


def test_stub_satisfies_protocol_structurally():
    class StubEngine:
        async def create_room(self, opts):
            ...

        async def get_room(self, room_id):
            ...

        async def destroy_room(self, room, *, graceful=True):
            ...

        async def add_media_participant(self, room, type, config):
            ...

        async def remove_participant(self, room, participant):
            ...

        async def mute_participant(self, room, participant, muted):
            ...

        async def move_participants(self, from_room, to_room, participants):
            ...

    _: RoomEngine = StubEngine()  # type: ignore[assignment]
    assert True
