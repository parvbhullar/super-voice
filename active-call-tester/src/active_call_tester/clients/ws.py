from __future__ import annotations

import asyncio
import json
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

import websockets

from active_call_tester.models import (
    CallResult,
    Protocol,
    TimestampedEvent,
)


@dataclass
class WsCallConfig:
    """Configuration for a single WebSocket call."""

    scenario: str
    codec: str = "pcmu"
    asr_provider: str = "sensevoice"
    tts_provider: str = "supertonic"
    callee: str = "sip:test@localhost"
    tts_text: str = "Hello, this is a test call."
    call_hold_secs: float = 5.0
    session_id: str = field(default_factory=lambda: f"test-{uuid.uuid4().hex[:8]}")
    ping_interval: int = 20
    extra_options: dict[str, Any] = field(default_factory=dict)


class WsClient:
    """WebSocket client for Active Call lifecycle testing."""

    def __init__(self, ws_url: str) -> None:
        self._ws_url = ws_url
        self._ws: Any = None
        self._events: list[TimestampedEvent] = []
        self._event_listener: asyncio.Task[None] | None = None
        self._event_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

    async def connect(self, config: WsCallConfig) -> None:
        """Open WebSocket connection with session params."""
        url = (
            f"{self._ws_url}"
            f"?id={config.session_id}"
            f"&ping_interval={config.ping_interval}"
            f"&dump_events=true"
        )
        self._ws = await websockets.connect(url)
        self._events.clear()
        self._event_listener = asyncio.create_task(self._listen_events())

    async def _listen_events(self) -> None:
        """Background task to receive and queue events."""
        try:
            async for message in self._ws:
                if isinstance(message, str):
                    event: dict[str, Any] = json.loads(message)
                    ts = TimestampedEvent(
                        name=event.get("event", "unknown"),
                        timestamp_ms=time.monotonic() * 1000,
                        data=event,
                    )
                    self._events.append(ts)
                    await self._event_queue.put(event)
        except websockets.ConnectionClosed:
            pass

    async def _send_command(self, command: dict[str, Any]) -> float:
        """Send JSON command, return send timestamp in ms."""
        ts = time.monotonic() * 1000
        await self._ws.send(json.dumps(command))
        return ts

    async def _wait_for_event(
        self, event_name: str, timeout: float = 30.0
    ) -> dict[str, Any] | None:
        """Wait for a specific event type.

        Returns event data or None on timeout.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                remaining = deadline - time.monotonic()
                event = await asyncio.wait_for(
                    self._event_queue.get(),
                    timeout=max(0.1, remaining),
                )
                if event.get("event") == event_name:
                    return event
            except asyncio.TimeoutError:
                break
        return None

    async def execute_call(self, config: WsCallConfig) -> CallResult:
        """Execute full call lifecycle and measure latency."""
        call_start = time.monotonic() * 1000
        error: str | None = None
        setup_latency = 0.0
        tts_first_byte = 0.0
        asr_first_result = 0.0
        teardown = 0.0

        try:
            # 1. Send Invite
            invite_ts = await self._send_command(
                {
                    "command": "Invite",
                    "option": {
                        "callee": config.callee,
                        "asr": {"provider": config.asr_provider},
                        "tts": {"provider": config.tts_provider},
                        "codec": config.codec,
                        **config.extra_options,
                    },
                }
            )

            # 2. Wait for Answer
            answer = await self._wait_for_event("Answer", timeout=15.0)
            if answer:
                setup_latency = time.monotonic() * 1000 - invite_ts
            else:
                error = "timeout_waiting_for_answer"

            # 3. Send TTS
            if not error:
                tts_ts = await self._send_command(
                    {
                        "command": "Tts",
                        "text": config.tts_text,
                        "speaker": "default",
                    }
                )

                # 4. Wait for TrackStart (first TTS byte)
                track_start = await self._wait_for_event("TrackStart", timeout=10.0)
                if track_start:
                    tts_first_byte = time.monotonic() * 1000 - tts_ts

                # 5. Hold and listen for ASR
                asr_listen_start = time.monotonic() * 1000
                asr_event = await self._wait_for_event(
                    "AsrFinal",
                    timeout=config.call_hold_secs,
                )
                if asr_event:
                    asr_first_result = time.monotonic() * 1000 - asr_listen_start

            # 6. Hangup
            hangup_ts = await self._send_command(
                {
                    "command": "Hangup",
                    "reason": "test_complete",
                }
            )
            hangup_event = await self._wait_for_event("Hangup", timeout=5.0)
            if hangup_event:
                teardown = time.monotonic() * 1000 - hangup_ts

        except Exception as e:
            error = str(e)

        total_duration = time.monotonic() * 1000 - call_start

        return CallResult(
            scenario=config.scenario,
            protocol=Protocol.WEBSOCKET,
            codec=config.codec,
            asr_provider=config.asr_provider,
            tts_provider=config.tts_provider,
            setup_latency_ms=setup_latency,
            first_tts_byte_ms=tts_first_byte,
            first_asr_result_ms=asr_first_result,
            teardown_ms=teardown,
            total_duration_ms=total_duration,
            success=error is None,
            error=error,
            events=list(self._events),
        )

    async def disconnect(self) -> None:
        """Close WebSocket and stop listener."""
        if self._event_listener and not self._event_listener.done():
            self._event_listener.cancel()
            try:
                await self._event_listener
            except asyncio.CancelledError:
                pass
        if self._ws:
            await self._ws.close()
            self._ws = None
