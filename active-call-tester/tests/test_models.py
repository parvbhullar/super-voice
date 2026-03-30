from __future__ import annotations

from active_call_tester.models import (
    CallResult,
    Protocol,
    Tier,
    TierResult,
    TimestampedEvent,
)


class TestProtocolEnum:
    def test_websocket_value(self) -> None:
        assert Protocol.WEBSOCKET.value == "websocket"

    def test_sip_value(self) -> None:
        assert Protocol.SIP.value == "sip"

    def test_webrtc_aiortc_value(self) -> None:
        assert Protocol.WEBRTC_AIORTC.value == "webrtc_aiortc"

    def test_webrtc_browser_value(self) -> None:
        assert Protocol.WEBRTC_BROWSER.value == "webrtc_browser"

    def test_protocol_from_string(self) -> None:
        assert Protocol("websocket") is Protocol.WEBSOCKET


class TestTierEnum:
    def test_smoke_value(self) -> None:
        assert Tier.SMOKE.value == "smoke"

    def test_load_value(self) -> None:
        assert Tier.LOAD.value == "load"

    def test_stress_value(self) -> None:
        assert Tier.STRESS.value == "stress"

    def test_tier_from_string(self) -> None:
        assert Tier("load") is Tier.LOAD


class TestTimestampedEvent:
    def test_create_minimal(self) -> None:
        event = TimestampedEvent(name="connected", timestamp_ms=100.0)
        assert event.name == "connected"
        assert event.timestamp_ms == 100.0
        assert event.data == {}

    def test_create_with_data(self) -> None:
        event = TimestampedEvent(
            name="tts_start",
            timestamp_ms=250.5,
            data={"provider": "elevenlabs", "latency": 12.3},
        )
        assert event.data["provider"] == "elevenlabs"
        assert event.data["latency"] == 12.3


class TestCallResult:
    def test_create_successful(self) -> None:
        result = CallResult(
            scenario="greeting",
            protocol=Protocol.WEBSOCKET,
            codec="opus",
            asr_provider="deepgram",
            tts_provider="elevenlabs",
            setup_latency_ms=120.0,
            first_tts_byte_ms=200.0,
            first_asr_result_ms=350.0,
            teardown_ms=50.0,
            total_duration_ms=5000.0,
            success=True,
        )
        assert result.success is True
        assert result.error is None
        assert result.events == []
        assert result.protocol is Protocol.WEBSOCKET

    def test_create_failed(self) -> None:
        result = CallResult(
            scenario="greeting",
            protocol=Protocol.SIP,
            codec="pcmu",
            asr_provider="google",
            tts_provider="google",
            setup_latency_ms=0.0,
            first_tts_byte_ms=0.0,
            first_asr_result_ms=0.0,
            teardown_ms=10.0,
            total_duration_ms=10.0,
            success=False,
            error="Connection refused",
        )
        assert result.success is False
        assert result.error == "Connection refused"

    def test_create_with_events(self) -> None:
        event = TimestampedEvent(name="rtp_start", timestamp_ms=100.0)
        result = CallResult(
            scenario="ivr",
            protocol=Protocol.WEBRTC_AIORTC,
            codec="opus",
            asr_provider="deepgram",
            tts_provider="aws",
            setup_latency_ms=150.0,
            first_tts_byte_ms=300.0,
            first_asr_result_ms=500.0,
            teardown_ms=40.0,
            total_duration_ms=3000.0,
            success=True,
            events=[event],
        )
        assert len(result.events) == 1
        assert result.events[0].name == "rtp_start"


class TestTierResult:
    def test_create_minimal(self) -> None:
        tier_result = TierResult(
            tier=Tier.SMOKE,
            scenario="greeting",
            concurrency=1,
            total_calls=10,
            successful_calls=9,
            failed_calls=1,
        )
        assert tier_result.tier is Tier.SMOKE
        assert tier_result.total_calls == 10
        assert tier_result.results == []
        assert tier_result.passed_thresholds is True

    def test_default_percentiles_are_zero(self) -> None:
        tier_result = TierResult(
            tier=Tier.LOAD,
            scenario="ivr",
            concurrency=50,
            total_calls=1000,
            successful_calls=990,
            failed_calls=10,
        )
        assert tier_result.p50_setup_ms == 0.0
        assert tier_result.p95_setup_ms == 0.0
        assert tier_result.p99_setup_ms == 0.0
        assert tier_result.p50_tts_ms == 0.0
        assert tier_result.p95_tts_ms == 0.0
        assert tier_result.p99_tts_ms == 0.0
        assert tier_result.p50_asr_ms == 0.0
        assert tier_result.p95_asr_ms == 0.0
        assert tier_result.p99_asr_ms == 0.0

    def test_with_custom_percentiles(self) -> None:
        tier_result = TierResult(
            tier=Tier.STRESS,
            scenario="full_conversation",
            concurrency=200,
            total_calls=5000,
            successful_calls=4800,
            failed_calls=200,
            p50_setup_ms=120.0,
            p95_setup_ms=350.0,
            p99_setup_ms=800.0,
            passed_thresholds=False,
        )
        assert tier_result.p50_setup_ms == 120.0
        assert tier_result.p95_setup_ms == 350.0
        assert tier_result.passed_thresholds is False
