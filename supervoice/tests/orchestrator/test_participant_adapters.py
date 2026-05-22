"""Tests for ParticipantAdapter Protocol + sip/livekit/webrtc adapters."""

from supervoice.orchestrator.participants import (
    LiveKitAdapter,
    ParticipantAdapter,
    SipAdapter,
    WebRtcAdapter,
)


def test_protocol_definition_imports():
    assert ParticipantAdapter is not None


def test_sip_adapter_has_required_methods():
    adapter = SipAdapter(participant_id="p1", config={})
    assert hasattr(adapter, "attach")
    assert hasattr(adapter, "detach")
    assert adapter.type == "sip"


def test_livekit_adapter_stub():
    adapter = LiveKitAdapter(participant_id="p1", config={})
    assert adapter.type == "livekit"
    assert hasattr(adapter, "attach")
    assert hasattr(adapter, "detach")


def test_webrtc_adapter_has_required_methods():
    adapter = WebRtcAdapter(participant_id="p1", config={})
    assert adapter.type == "webrtc"
    assert hasattr(adapter, "attach")
    assert hasattr(adapter, "detach")


def test_all_adapters_exported_from_package():
    # Final exports smoke test for the package surface.
    for cls in (ParticipantAdapter, SipAdapter, WebRtcAdapter, LiveKitAdapter):
        assert cls is not None
