"""Tests for the WebRtcAdapter — V1 stub-grade engine delegation."""

from supervoice.orchestrator.participants import WebRtcAdapter
from supervoice.orchestrator.room import InProcessRoomEngine, RoomOpts


async def test_webrtc_adapter_attaches_to_room():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    adapter = WebRtcAdapter(participant_id="p1", config={"sdp_offer": "v=0..."})
    handle = await adapter.attach(room, engine)
    assert handle.type == "webrtc"
    assert handle.participant_id  # engine assigned an id


async def test_webrtc_adapter_detach_is_noop():
    adapter = WebRtcAdapter(participant_id="p1", config={})
    # should not raise even if attach wasn't called
    await adapter.detach()


async def test_webrtc_adapter_attach_then_detach():
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s2", metadata={}))
    adapter = WebRtcAdapter(participant_id="p2", config={})
    await adapter.attach(room, engine)
    await adapter.detach()
    # _attached_handle cleared after detach
    assert adapter._attached_handle is None
