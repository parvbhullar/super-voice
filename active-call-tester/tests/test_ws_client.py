from __future__ import annotations

import asyncio
import json
from typing import Any
from unittest.mock import AsyncMock, patch

import pytest

from active_call_tester.clients.ws import WsCallConfig, WsClient
from active_call_tester.models import Protocol


# ==================================================================
# WsCallConfig dataclass tests
# ==================================================================


class TestWsCallConfig:
    """Tests for WsCallConfig defaults and custom values."""

    def test_defaults(self) -> None:
        cfg = WsCallConfig(scenario="smoke")
        assert cfg.scenario == "smoke"
        assert cfg.codec == "pcmu"
        assert cfg.asr_provider == "sensevoice"
        assert cfg.tts_provider == "supertonic"
        assert cfg.callee == "sip:test@localhost"
        assert cfg.tts_text == "Hello, this is a test call."
        assert cfg.call_hold_secs == 5.0
        assert cfg.ping_interval == 20
        assert cfg.extra_options == {}

    def test_session_id_generated(self) -> None:
        cfg = WsCallConfig(scenario="smoke")
        assert cfg.session_id.startswith("test-")
        assert len(cfg.session_id) == 13  # "test-" + 8 hex

    def test_unique_session_ids(self) -> None:
        a = WsCallConfig(scenario="a")
        b = WsCallConfig(scenario="b")
        assert a.session_id != b.session_id

    def test_custom_values(self) -> None:
        cfg = WsCallConfig(
            scenario="load",
            codec="opus",
            asr_provider="whisper",
            tts_provider="elevenlabs",
            callee="sip:prod@example.com",
            tts_text="Custom text",
            call_hold_secs=10.0,
            session_id="fixed-id",
            ping_interval=30,
            extra_options={"key": "val"},
        )
        assert cfg.codec == "opus"
        assert cfg.asr_provider == "whisper"
        assert cfg.tts_provider == "elevenlabs"
        assert cfg.callee == "sip:prod@example.com"
        assert cfg.tts_text == "Custom text"
        assert cfg.call_hold_secs == 10.0
        assert cfg.session_id == "fixed-id"
        assert cfg.ping_interval == 30
        assert cfg.extra_options == {"key": "val"}


# ==================================================================
# WsClient initialisation
# ==================================================================


class TestWsClientInit:
    """Tests for WsClient constructor."""

    def test_ws_url_stored(self) -> None:
        client = WsClient("ws://localhost:8080/call")
        assert client._ws_url == "ws://localhost:8080/call"

    def test_ws_starts_none(self) -> None:
        client = WsClient("ws://x")
        assert client._ws is None

    def test_events_starts_empty(self) -> None:
        client = WsClient("ws://x")
        assert client._events == []

    def test_listener_starts_none(self) -> None:
        client = WsClient("ws://x")
        assert client._event_listener is None


# ==================================================================
# _send_command
# ==================================================================


class TestSendCommand:
    """Tests for _send_command JSON serialization."""

    @pytest.mark.asyncio
    async def test_send_command_serializes_json(self) -> None:
        client = WsClient("ws://x")
        mock_ws = AsyncMock()
        client._ws = mock_ws

        cmd = {"command": "Invite", "option": {"callee": "x"}}
        ts = await client._send_command(cmd)

        mock_ws.send.assert_awaited_once_with(json.dumps(cmd))
        assert ts > 0

    @pytest.mark.asyncio
    async def test_send_command_returns_timestamp(
        self,
    ) -> None:
        client = WsClient("ws://x")
        client._ws = AsyncMock()

        ts = await client._send_command({"command": "Hangup"})
        assert isinstance(ts, float)
        assert ts > 0


# ==================================================================
# _wait_for_event
# ==================================================================


class TestWaitForEvent:
    """Tests for _wait_for_event matching and timeout."""

    @pytest.mark.asyncio
    async def test_returns_matching_event(self) -> None:
        client = WsClient("ws://x")
        event = {"event": "Answer", "track_id": "t1"}
        await client._event_queue.put(event)

        result = await client._wait_for_event("Answer", timeout=1.0)
        assert result == event

    @pytest.mark.asyncio
    async def test_skips_non_matching_events(self) -> None:
        client = WsClient("ws://x")
        await client._event_queue.put({"event": "Ping", "timestamp": 1})
        await client._event_queue.put({"event": "Answer", "track_id": "t1"})

        result = await client._wait_for_event("Answer", timeout=1.0)
        assert result is not None
        assert result["event"] == "Answer"

    @pytest.mark.asyncio
    async def test_returns_none_on_timeout(self) -> None:
        client = WsClient("ws://x")
        result = await client._wait_for_event("Answer", timeout=0.2)
        assert result is None

    @pytest.mark.asyncio
    async def test_returns_none_when_only_wrong_events(
        self,
    ) -> None:
        client = WsClient("ws://x")
        await client._event_queue.put({"event": "Ping"})

        result = await client._wait_for_event("Answer", timeout=0.3)
        assert result is None


# ==================================================================
# Helper to build a mock WS that feeds events
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


def _make_mock_ws(
    events: list[dict[str, Any]],
) -> _FakeWs:
    """Create a fake websocket that yields events."""
    return _FakeWs(events)


# ==================================================================
# execute_call lifecycle
# ==================================================================


class TestExecuteCall:
    """Tests for execute_call full lifecycle."""

    @pytest.mark.asyncio
    async def test_full_lifecycle_success(self) -> None:
        """Simulate Answer -> TrackStart -> AsrFinal -> Hangup."""
        client = WsClient("ws://localhost/call")
        config = WsCallConfig(
            scenario="smoke",
            call_hold_secs=2.0,
            session_id="test-abc",
        )

        events = [
            {"event": "Answer", "track_id": "t1"},
            {"event": "TrackStart", "track_id": "t1"},
            {"event": "AsrFinal", "track_id": "t1", "text": "hi"},
            {"event": "Hangup", "track_id": "t1", "reason": "ok"},
        ]
        mock_ws = _make_mock_ws(events)

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)
            # Give the listener task a moment to queue events
            await asyncio.sleep(0.05)

            result = await client.execute_call(config)

        assert result.success is True
        assert result.error is None
        assert result.scenario == "smoke"
        assert result.protocol == Protocol.WEBSOCKET
        assert result.codec == "pcmu"
        assert result.asr_provider == "sensevoice"
        assert result.tts_provider == "supertonic"
        assert result.setup_latency_ms >= 0
        assert result.first_tts_byte_ms >= 0
        assert result.total_duration_ms > 0

    @pytest.mark.asyncio
    async def test_timeout_no_answer(self) -> None:
        """When no Answer event arrives, error is set."""
        client = WsClient("ws://localhost/call")
        config = WsCallConfig(
            scenario="timeout_test",
            call_hold_secs=0.1,
            session_id="test-timeout",
        )

        # Only send Ping and Hangup (no Answer)
        events = [
            {"event": "Ping", "timestamp": 1},
            {"event": "Hangup", "track_id": "t1", "reason": "ok"},
        ]
        mock_ws = _make_mock_ws(events)

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)
            await asyncio.sleep(0.05)

            # Override _wait_for_event to speed up timeout
            original_wait = client._wait_for_event

            async def fast_wait(
                event_name: str, timeout: float = 30.0
            ) -> dict[str, Any] | None:
                return await original_wait(event_name, timeout=min(timeout, 0.2))

            client._wait_for_event = fast_wait  # type: ignore[assignment]
            result = await client.execute_call(config)

        assert result.success is False
        assert result.error == "timeout_waiting_for_answer"
        assert result.setup_latency_ms == 0.0

    @pytest.mark.asyncio
    async def test_connection_error(self) -> None:
        """When ws send raises, error is captured."""
        client = WsClient("ws://localhost/call")
        config = WsCallConfig(
            scenario="error_test",
            session_id="test-err",
        )

        mock_ws = AsyncMock()
        mock_ws.send = AsyncMock(side_effect=ConnectionError("refused"))
        mock_ws.close = AsyncMock()
        mock_ws.__aiter__ = AsyncMock(
            return_value=AsyncMock(__anext__=AsyncMock(side_effect=StopAsyncIteration))
        )

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)
            await asyncio.sleep(0.05)
            result = await client.execute_call(config)

        assert result.success is False
        assert "refused" in (result.error or "")

    @pytest.mark.asyncio
    async def test_events_collected_during_call(self) -> None:
        """Events list is populated from WS messages."""
        client = WsClient("ws://localhost/call")
        config = WsCallConfig(
            scenario="events_test",
            call_hold_secs=0.1,
            session_id="test-events",
        )

        events = [
            {"event": "Answer", "track_id": "t1"},
            {"event": "TrackStart", "track_id": "t1"},
            {"event": "Hangup", "track_id": "t1", "reason": "ok"},
        ]
        mock_ws = _make_mock_ws(events)

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)
            await asyncio.sleep(0.05)

            # Events should have been collected by listener
            assert len(client._events) == len(events)
            event_names = [e.name for e in client._events]
            assert "Answer" in event_names
            assert "TrackStart" in event_names
            assert "Hangup" in event_names

            # Each event has a positive timestamp
            for evt in client._events:
                assert evt.timestamp_ms > 0


# ==================================================================
# disconnect
# ==================================================================


class TestDisconnect:
    """Tests for disconnect cleanup."""

    @pytest.mark.asyncio
    async def test_disconnect_closes_ws(self) -> None:
        client = WsClient("ws://x")
        mock_ws = AsyncMock()
        client._ws = mock_ws

        await client.disconnect()
        mock_ws.close.assert_awaited_once()
        assert client._ws is None

    @pytest.mark.asyncio
    async def test_disconnect_cancels_listener(self) -> None:
        client = WsClient("ws://x")
        mock_ws = AsyncMock()
        client._ws = mock_ws

        # Create a long-running task as listener
        async def long_task() -> None:
            await asyncio.sleep(100)

        client._event_listener = asyncio.create_task(long_task())
        await client.disconnect()

        assert client._event_listener.done()
        assert client._ws is None

    @pytest.mark.asyncio
    async def test_disconnect_when_no_connection(self) -> None:
        client = WsClient("ws://x")
        await client.disconnect()  # should not raise
        assert client._ws is None

    @pytest.mark.asyncio
    async def test_disconnect_with_done_listener(self) -> None:
        client = WsClient("ws://x")
        mock_ws = AsyncMock()
        client._ws = mock_ws

        # Already-done task
        async def instant() -> None:
            return

        task = asyncio.create_task(instant())
        await task  # let it finish
        client._event_listener = task

        await client.disconnect()
        assert client._ws is None


# ==================================================================
# connect
# ==================================================================


class TestConnect:
    """Tests for connect URL construction."""

    @pytest.mark.asyncio
    async def test_connect_builds_url(self) -> None:
        client = WsClient("ws://host:8080/call")
        config = WsCallConfig(
            scenario="test",
            session_id="sess-1",
            ping_interval=15,
        )
        mock_ws = _make_mock_ws([])

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ) as mock_connect:
            await client.connect(config)

            expected_url = (
                "ws://host:8080/call?id=sess-1&ping_interval=15&dump_events=true"
            )
            mock_connect.assert_awaited_once_with(expected_url)

    @pytest.mark.asyncio
    async def test_connect_clears_events(self) -> None:
        client = WsClient("ws://x")
        # Pre-populate events
        from active_call_tester.models import TimestampedEvent

        client._events.append(TimestampedEvent(name="old", timestamp_ms=0, data={}))
        config = WsCallConfig(scenario="test", session_id="s1")
        mock_ws = _make_mock_ws([])

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)

        assert len(client._events) == 0

    @pytest.mark.asyncio
    async def test_connect_starts_listener_task(self) -> None:
        client = WsClient("ws://x")
        config = WsCallConfig(scenario="test", session_id="s2")
        mock_ws = _make_mock_ws([])

        with patch(
            "active_call_tester.clients.ws.websockets.connect",
            new_callable=AsyncMock,
            return_value=mock_ws,
        ):
            await client.connect(config)

        assert client._event_listener is not None
        assert isinstance(client._event_listener, asyncio.Task)
