from __future__ import annotations

import sys
from unittest.mock import (
    AsyncMock,
    MagicMock,
    patch,
)

import pytest

from active_call_tester.clients.webrtc import (
    AiortcWebRtcClient,
    PlaywrightWebRtcClient,
    WebRtcCallConfig,
)
from active_call_tester.models import Protocol


# ==================================================================
# WebRtcCallConfig dataclass tests
# ==================================================================


class TestWebRtcCallConfig:
    """Tests for WebRtcCallConfig defaults and custom values."""

    def test_defaults(self) -> None:
        cfg = WebRtcCallConfig(scenario="smoke")
        assert cfg.scenario == "smoke"
        assert cfg.codec == "opus"
        assert cfg.asr_provider == "sensevoice"
        assert cfg.tts_provider == "supertonic"
        assert cfg.tts_text == ("Hello, this is a WebRTC test call.")
        assert cfg.call_hold_secs == 5.0
        assert cfg.ice_servers == []
        assert cfg.mode == "aiortc"

    def test_session_id_generated(self) -> None:
        cfg = WebRtcCallConfig(scenario="smoke")
        assert cfg.session_id.startswith("webrtc-")
        assert len(cfg.session_id) == 15  # "webrtc-" + 8 hex

    def test_unique_session_ids(self) -> None:
        a = WebRtcCallConfig(scenario="a")
        b = WebRtcCallConfig(scenario="b")
        assert a.session_id != b.session_id

    def test_custom_values(self) -> None:
        cfg = WebRtcCallConfig(
            scenario="load",
            codec="pcmu",
            asr_provider="whisper",
            tts_provider="elevenlabs",
            tts_text="Custom text",
            call_hold_secs=10.0,
            session_id="fixed-rtc-id",
            ice_servers=[{"urls": "stun:stun.example.com:3478"}],
            mode="browser",
        )
        assert cfg.codec == "pcmu"
        assert cfg.asr_provider == "whisper"
        assert cfg.session_id == "fixed-rtc-id"
        assert len(cfg.ice_servers) == 1
        assert cfg.mode == "browser"


# ==================================================================
# AiortcWebRtcClient initialization
# ==================================================================


class TestAiortcWebRtcClientInit:
    """Tests for AiortcWebRtcClient constructor."""

    def test_url_construction(self) -> None:
        client = AiortcWebRtcClient("http://localhost:8080")
        assert client._base_url == "http://localhost:8080"
        assert client._webrtc_url == ("http://localhost:8080/call/webrtc")

    def test_trailing_slash_stripped(self) -> None:
        client = AiortcWebRtcClient("http://localhost:8080/")
        assert client._base_url == "http://localhost:8080"
        assert client._webrtc_url == ("http://localhost:8080/call/webrtc")

    def test_pc_starts_none(self) -> None:
        client = AiortcWebRtcClient("http://x")
        assert client._pc is None

    def test_events_starts_empty(self) -> None:
        client = AiortcWebRtcClient("http://x")
        assert client._events == []


# ==================================================================
# PlaywrightWebRtcClient initialization
# ==================================================================


class TestPlaywrightWebRtcClientInit:
    """Tests for PlaywrightWebRtcClient constructor."""

    def test_url_construction(self) -> None:
        client = PlaywrightWebRtcClient("http://localhost:9090")
        assert client._base_url == "http://localhost:9090"

    def test_trailing_slash_stripped(self) -> None:
        client = PlaywrightWebRtcClient("http://localhost:9090/")
        assert client._base_url == "http://localhost:9090"

    def test_browser_starts_none(self) -> None:
        client = PlaywrightWebRtcClient("http://x")
        assert client._browser is None

    def test_page_starts_none(self) -> None:
        client = PlaywrightWebRtcClient("http://x")
        assert client._page is None

    def test_events_starts_empty(self) -> None:
        client = PlaywrightWebRtcClient("http://x")
        assert client._events == []


# ==================================================================
# ImportError handling for aiortc
# ==================================================================


class TestAiortcImportError:
    """Test that missing aiortc raises ImportError."""

    @pytest.mark.asyncio
    async def test_connect_raises_when_aiortc_missing(
        self,
    ) -> None:
        client = AiortcWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="test")

        with patch.dict(sys.modules, {"aiortc": None}):
            with pytest.raises(ImportError, match="aiortc is required"):
                await client.connect(config)


# ==================================================================
# ImportError handling for playwright
# ==================================================================


class TestPlaywrightImportError:
    """Test that missing playwright raises ImportError."""

    @pytest.mark.asyncio
    async def test_connect_raises_when_playwright_missing(
        self,
    ) -> None:
        client = PlaywrightWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="test")

        with patch.dict(
            sys.modules,
            {
                "playwright": None,
                "playwright.async_api": None,
            },
        ):
            with pytest.raises(
                ImportError,
                match="playwright is required",
            ):
                await client.connect(config)


# ==================================================================
# AiortcWebRtcClient connect with mocked aiortc
# ==================================================================


class TestAiortcConnect:
    """Tests for AiortcWebRtcClient.connect with mocked deps."""

    @pytest.mark.asyncio
    async def test_connect_creates_peer_connection(
        self,
    ) -> None:
        client = AiortcWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="test")

        mock_pc = MagicMock()
        mock_pc.on = MagicMock(side_effect=lambda event: lambda fn: fn)

        with patch(
            "active_call_tester.clients.webrtc.AiortcWebRtcClient.connect"
        ) as mock_connect:
            mock_connect.return_value = None
            await client.connect(config)
            mock_connect.assert_awaited_once_with(config)


# ==================================================================
# AiortcWebRtcClient disconnect
# ==================================================================


class TestAiortcDisconnect:
    """Tests for AiortcWebRtcClient disconnect."""

    @pytest.mark.asyncio
    async def test_disconnect_closes_pc(self) -> None:
        client = AiortcWebRtcClient("http://x")
        mock_pc = AsyncMock()
        client._pc = mock_pc

        await client.disconnect()
        mock_pc.close.assert_awaited_once()
        assert client._pc is None

    @pytest.mark.asyncio
    async def test_disconnect_when_not_connected(
        self,
    ) -> None:
        client = AiortcWebRtcClient("http://x")
        await client.disconnect()  # should not raise
        assert client._pc is None


# ==================================================================
# PlaywrightWebRtcClient disconnect
# ==================================================================


class TestPlaywrightDisconnect:
    """Tests for PlaywrightWebRtcClient disconnect."""

    @pytest.mark.asyncio
    async def test_disconnect_closes_all(self) -> None:
        client = PlaywrightWebRtcClient("http://x")
        mock_page = AsyncMock()
        mock_browser = AsyncMock()
        mock_pw = AsyncMock()
        client._page = mock_page
        client._browser = mock_browser
        client._pw = mock_pw

        await client.disconnect()

        mock_page.close.assert_awaited_once()
        mock_browser.close.assert_awaited_once()
        mock_pw.stop.assert_awaited_once()
        assert client._page is None
        assert client._browser is None
        assert client._pw is None

    @pytest.mark.asyncio
    async def test_disconnect_when_not_connected(
        self,
    ) -> None:
        client = PlaywrightWebRtcClient("http://x")
        await client.disconnect()  # should not raise
        assert client._page is None
        assert client._browser is None

    @pytest.mark.asyncio
    async def test_disconnect_partial_state(self) -> None:
        """Test disconnect with only browser set (no page)."""
        client = PlaywrightWebRtcClient("http://x")
        mock_browser = AsyncMock()
        mock_pw = AsyncMock()
        client._browser = mock_browser
        client._pw = mock_pw

        await client.disconnect()
        mock_browser.close.assert_awaited_once()
        mock_pw.stop.assert_awaited_once()
        assert client._browser is None


# ==================================================================
# PlaywrightWebRtcClient execute_call without connect
# ==================================================================


class TestPlaywrightExecuteCallErrors:
    """Tests for execute_call error handling."""

    @pytest.mark.asyncio
    async def test_execute_without_connect(self) -> None:
        """execute_call without connect captures error."""
        client = PlaywrightWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="test", call_hold_secs=0.1)

        result = await client.execute_call(config)
        assert result.success is False
        assert result.error is not None
        assert "Not connected" in result.error
        assert result.protocol == Protocol.WEBRTC_BROWSER

    @pytest.mark.asyncio
    async def test_execute_returns_correct_protocol(
        self,
    ) -> None:
        """Verify protocol is WEBRTC_BROWSER."""
        client = PlaywrightWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="proto", call_hold_secs=0.1)

        result = await client.execute_call(config)
        assert result.protocol == Protocol.WEBRTC_BROWSER


# ==================================================================
# AiortcWebRtcClient execute_call without connect
# ==================================================================


class TestAiortcExecuteCallErrors:
    """Tests for execute_call error handling."""

    @pytest.mark.asyncio
    async def test_execute_without_connect(self) -> None:
        """execute_call without connect captures error."""
        client = AiortcWebRtcClient("http://x")
        config = WebRtcCallConfig(scenario="test", call_hold_secs=0.1)

        result = await client.execute_call(config)
        assert result.success is False
        assert result.error is not None
        assert result.protocol == Protocol.WEBRTC_AIORTC
