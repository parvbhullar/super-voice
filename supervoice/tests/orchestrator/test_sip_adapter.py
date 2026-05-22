"""Tests for SipAdapter (Task 28).

Validates that SipAdapter delegates to the RoomEngine for attach/detach
and handles edge cases (detach-before-attach, type attribute).
"""

from __future__ import annotations

import pytest

from supervoice.orchestrator.participants.sip_adapter import SipAdapter
from supervoice.orchestrator.room.engine import RoomOpts
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine


@pytest.mark.asyncio
async def test_sip_attach_delegates_to_engine() -> None:
    """attach() calls engine.add_media_participant with type='sip'."""
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    config = {"sdp_offer": "v=0\r\nfake", "from_number": "+1"}
    adapter = SipAdapter(participant_id="p-sip-1", config=config)

    handle = await adapter.attach(room, engine)

    assert handle.type == "sip"
    assert handle.participant_id  # non-empty
    # The engine should have exactly one participant in the room.
    internal = engine._rooms[room.room_id]
    assert len(internal.participants) == 1
    assert internal.participants[0].type == "sip"


@pytest.mark.asyncio
async def test_sip_detach_removes_participant() -> None:
    """attach then detach leaves zero participants in the room."""
    engine = InProcessRoomEngine()
    room = await engine.create_room(RoomOpts(session_id="s1", metadata={}))
    adapter = SipAdapter(participant_id="p-sip-1", config={})
    await adapter.attach(room, engine)

    internal = engine._rooms[room.room_id]
    assert len(internal.participants) == 1

    await adapter.detach()

    assert len(internal.participants) == 0


@pytest.mark.asyncio
async def test_sip_detach_without_attach_is_noop() -> None:
    """Calling detach before attach must not raise."""
    adapter = SipAdapter(participant_id="p-sip-1", config={})
    # Should complete without error.
    await adapter.detach()


def test_sip_adapter_type_is_sip() -> None:
    """The class-level type attribute must be 'sip'."""
    adapter = SipAdapter(participant_id="p-sip-1", config={})
    assert adapter.type == "sip"
