from __future__ import annotations

import asyncio
import json
from typing import Any
from unittest.mock import AsyncMock, patch

import pytest

from active_call_tester.clients.sip import (
    SipCallConfig,
    SipClient,
)
from active_call_tester.models import Protocol


# ==================================================================
# SipCallConfig dataclass tests
# ==================================================================


class TestSipCallConfig:
    """Tests for SipCallConfig defaults and custom values."""

    def test_defaults(self) -> None:
        cfg = SipCallConfig(scenario="smoke")
        assert cfg.scenario == "smoke"
        assert cfg.codec == "pcmu"
        assert cfg.asr_provider == "sensevoice"
        assert cfg.tts_provider == "supertonic"
        assert cfg.callee == "sip:test@localhost:5060"
        assert cfg.caller == "sip:tester@localhost"
        assert cfg.tts_text == "Hello, this is a SIP test call."
        assert cfg.call_hold_secs == 5.0
        assert cfg.username is None
        assert cfg.password is None
        assert cfg.realm is None
        assert cfg.enable_srtp is False
        assert cfg.sip_headers == {}
        assert cfg.transport == "udp"

    def test_session_id_generated(self) -> None:
        cfg = SipCallConfig(scenario="smoke")
        assert cfg.session_id.startswith("sip-test-")
        assert len(cfg.session_id) == 17  # "sip-test-" + 8 hex

    def test_unique_session_ids(self) -> None:
        a = SipCallConfig(scenario="a")
        b = SipCallConfig(scenario="b")
        assert a.session_id != b.session_id

    def test_custom_values(self) -> None:
        cfg = SipCallConfig(
            scenario="load",
            codec="opus",
            asr_provider="whisper",
            tts_provider="elevenlabs",
            callee="sip:prod@example.com",
            caller="sip:agent@example.com",
            tts_text="Custom SIP text",
            call_hold_secs=10.0,
            session_id="fixed-sip-id",
            username="user1",
            password="pass1",
            realm="example.com",
            enable_srtp=True,
            sip_headers={"X-Custom": "value"},
            transport="tls",
        )
        assert cfg.codec == "opus"
        assert cfg.asr_provider == "whisper"
        assert cfg.caller == "sip:agent@example.com"
        assert cfg.session_id == "fixed-sip-id"
        assert cfg.username == "user1"
        assert cfg.password == "pass1"
        assert cfg.realm == "example.com"
        assert cfg.enable_srtp is True
        assert cfg.sip_headers == {"X-Custom": "value"}
        assert cfg.transport == "tls"


# ==================================================================
# SipClient URL construction
# ==================================================================


class TestSipClientInit:
    """Tests for SipClient constructor and URL rewriting."""

    def test_http_to_ws(self) -> None:
        client = SipClient("http://localhost:8080")
        assert client._sip_ws_url == ("ws://localhost:8080/call/sip")

    def test_https_to_wss(self) -> None:
        client = SipClient("https://example.com")
        assert client._sip_ws_url == ("wss://example.com/call/sip")

    def test_trailing_slash_stripped(self) -> None:
        client = SipClient("http://host:9000/")
        assert client._sip_ws_url == ("ws://host:9000/call/sip")

    def test_ws_client_starts_none(self) -> None:
        client = SipClient("http://x")
        assert client._ws_client is None


# ==================================================================
# _build_sip_options
# ==================================================================


class TestBuildSipOptions:
    """Tests for _build_sip_options."""

    def test_empty_when_no_sip_specifics(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(
            scenario="test",
            username=None,
            password=None,
            realm=None,
            enable_srtp=False,
            sip_headers={},
            caller="",
        )
        opts = client._build_sip_options(cfg)
        # No sip key, no caller (empty string is falsy)
        assert "sip" not in opts
        assert "caller" not in opts

    def test_caller_included(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(
            scenario="test",
            caller="sip:me@host",
        )
        opts = client._build_sip_options(cfg)
        assert opts["caller"] == "sip:me@host"

    def test_sip_auth_options(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(
            scenario="test",
            username="user",
            password="pass",
            realm="realm.com",
        )
        opts = client._build_sip_options(cfg)
        assert opts["sip"]["username"] == "user"
        assert opts["sip"]["password"] == "pass"
        assert opts["sip"]["realm"] == "realm.com"

    def test_srtp_flag(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(scenario="test", enable_srtp=True)
        opts = client._build_sip_options(cfg)
        assert opts["sip"]["enable_srtp"] is True

    def test_sip_headers_included(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(
            scenario="test",
            sip_headers={"X-Foo": "bar"},
        )
        opts = client._build_sip_options(cfg)
        assert opts["sip"]["headers"] == {"X-Foo": "bar"}

    def test_all_options_combined(self) -> None:
        client = SipClient("http://x")
        cfg = SipCallConfig(
            scenario="test",
            caller="sip:a@b",
            username="u",
            password="p",
            realm="r",
            enable_srtp=True,
            sip_headers={"H": "V"},
        )
        opts = client._build_sip_options(cfg)
        assert opts["caller"] == "sip:a@b"
        assert opts["sip"]["username"] == "u"
        assert opts["sip"]["enable_srtp"] is True
        assert opts["sip"]["headers"] == {"H": "V"}


# ==================================================================
# Fake WS for SIP lifecycle tests
# ==================================================================


class _FakeWs:
    """Fake websocket that yields pre-configured events."""

    def __init__(self, events: list[dict[str, Any]]) -> None:
        self._events = events
        self.send = AsyncMock()
        self.close = AsyncMock()

    def __aiter__(self) -> _FakeWs:
        self._iter = iter(self._events)
        return self

    async def __anext__(self) -> str:
        try:
            return json.dumps(next(self._iter))
        except StopIteration:
            raise StopAsyncIteration


# ==================================================================
# execute_call
# ==================================================================


class TestExecuteCall:
    """Tests for execute_call protocol override."""

    @pytest.mark.asyncio
    async def test_execute_call_sets_sip_protocol(
        self,
    ) -> None:
        """Result protocol is overridden to SIP."""
        client = SipClient("http://localhost:8080")
        config = SipCallConfig(
            scenario="smoke",
            call_hold_secs=0.1,
            session_id="sip-test-abc",
        )

        events = [
            {"event": "Answer", "track_id": "t1"},
            {"event": "TrackStart", "track_id": "t1"},
            {
                "event": "Hangup",
                "track_id": "t1",
                "reason": "ok",
            },
        ]
        mock_ws = _FakeWs(events)

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)
            await asyncio.sleep(0.05)

            # Speed up wait_for_event timeouts
            ws_client = client._ws_client
            assert ws_client is not None
            original_wait = ws_client._wait_for_event

            async def fast_wait(
                event_name: str, timeout: float = 30.0
            ) -> dict[str, Any] | None:
                return await original_wait(event_name, timeout=min(timeout, 0.3))

            ws_client._wait_for_event = fast_wait  # type: ignore[assignment]

            result = await client.execute_call(config)

        assert result.protocol == Protocol.SIP
        assert result.scenario == "smoke"
        assert result.total_duration_ms > 0

    @pytest.mark.asyncio
    async def test_execute_call_without_connect_raises(
        self,
    ) -> None:
        """Calling execute_call before connect raises."""
        client = SipClient("http://localhost:8080")
        config = SipCallConfig(scenario="test")

        with pytest.raises(RuntimeError, match="Not connected"):
            await client.execute_call(config)


# ==================================================================
# disconnect
# ==================================================================


class TestDisconnect:
    """Tests for disconnect cleanup."""

    @pytest.mark.asyncio
    async def test_disconnect_cleans_up(self) -> None:
        client = SipClient("http://x")
        mock_ws_client = AsyncMock()
        client._ws_client = mock_ws_client

        await client.disconnect()
        mock_ws_client.disconnect.assert_awaited_once()
        assert client._ws_client is None

    @pytest.mark.asyncio
    async def test_disconnect_when_not_connected(
        self,
    ) -> None:
        client = SipClient("http://x")
        await client.disconnect()  # should not raise
        assert client._ws_client is None
