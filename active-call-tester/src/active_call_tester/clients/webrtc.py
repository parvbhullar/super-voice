from __future__ import annotations

import asyncio
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

from active_call_tester.models import (
    CallResult,
    Protocol,
    TimestampedEvent,
)


@dataclass
class WebRtcCallConfig:
    """Configuration for a WebRTC call test."""

    scenario: str
    codec: str = "opus"
    asr_provider: str = "sensevoice"
    tts_provider: str = "supertonic"
    tts_text: str = "Hello, this is a WebRTC test call."
    call_hold_secs: float = 5.0
    session_id: str = field(default_factory=lambda: f"webrtc-{uuid.uuid4().hex[:8]}")
    ice_servers: list[dict[str, str]] = field(default_factory=list)
    mode: str = "aiortc"  # "aiortc" or "browser"


class AiortcWebRtcClient:
    """WebRTC client using aiortc for headless bulk testing."""

    def __init__(self, base_url: str) -> None:
        self._base_url = base_url.rstrip("/")
        self._webrtc_url = f"{self._base_url}/call/webrtc"
        self._pc: Any = None  # RTCPeerConnection
        self._events: list[TimestampedEvent] = []

    async def connect(self, config: WebRtcCallConfig) -> None:
        """Set up WebRTC peer connection."""
        try:
            from aiortc import (  # type: ignore[import-untyped]
                RTCConfiguration,
                RTCIceServer,
                RTCPeerConnection,
            )
        except ImportError:
            raise ImportError(
                "aiortc is required for WebRTC testing. Install with: uv add aiortc"
            )

        default_servers = [{"urls": "stun:stun.l.google.com:19302"}]
        servers = config.ice_servers or default_servers
        ice_config = RTCConfiguration(
            iceServers=[
                RTCIceServer(
                    urls=s.get(
                        "urls",
                        "stun:stun.l.google.com:19302",
                    )
                )
                for s in servers
            ]
        )
        self._pc = RTCPeerConnection(configuration=ice_config)
        self._events.clear()
        self._config = config

        pc = self._pc

        @pc.on("iceconnectionstatechange")
        async def on_ice_state_change() -> None:
            self._events.append(
                TimestampedEvent(
                    name=f"ice_{pc.iceConnectionState}",
                    timestamp_ms=time.monotonic() * 1000,
                )
            )

        @pc.on("track")
        async def on_track(track: Any) -> None:
            self._events.append(
                TimestampedEvent(
                    name="track_received",
                    timestamp_ms=time.monotonic() * 1000,
                    data={"kind": track.kind},
                )
            )

    async def execute_call(self, config: WebRtcCallConfig) -> CallResult:
        """Execute WebRTC call via aiortc."""
        call_start = time.monotonic() * 1000
        error: str | None = None
        setup_latency = 0.0
        tts_first_byte = 0.0
        asr_first_result = 0.0
        teardown = 0.0

        try:
            import aiohttp
            from aiortc import (  # type: ignore[import-untyped]
                RTCSessionDescription,
            )

            if self._pc is None:
                raise RuntimeError("Not connected. Call connect() first.")

            # Add audio transceiver
            self._pc.addTransceiver("audio", direction="sendrecv")

            # Create offer
            offer = await self._pc.createOffer()
            await self._pc.setLocalDescription(offer)

            # Send offer to server via HTTP
            offer_ts = time.monotonic() * 1000
            async with aiohttp.ClientSession() as session:
                url = (
                    f"{self._webrtc_url}"
                    f"?id={config.session_id}"
                    f"&codec={config.codec}"
                    f"&asr_provider={config.asr_provider}"
                    f"&tts_provider={config.tts_provider}"
                )
                async with session.post(
                    url,
                    json={
                        "sdp": self._pc.localDescription.sdp,
                        "type": (self._pc.localDescription.type),
                    },
                ) as resp:
                    if resp.status != 200:
                        error = f"offer_rejected_{resp.status}"
                    else:
                        answer_data = await resp.json()
                        answer = RTCSessionDescription(
                            sdp=answer_data["sdp"],
                            type=answer_data["type"],
                        )
                        await self._pc.setRemoteDescription(answer)
                        setup_latency = time.monotonic() * 1000 - offer_ts

            if not error:
                # Wait for ICE connected
                ice_start = time.monotonic()
                while (
                    self._pc.iceConnectionState not in ("connected", "completed")
                    and time.monotonic() - ice_start < 10.0
                ):
                    await asyncio.sleep(0.1)

                if self._pc.iceConnectionState not in (
                    "connected",
                    "completed",
                ):
                    error = "ice_connection_timeout"
                else:
                    # Hold call for configured duration
                    await asyncio.sleep(config.call_hold_secs)

            # Teardown
            teardown_start = time.monotonic() * 1000
            await self._pc.close()
            teardown = time.monotonic() * 1000 - teardown_start

        except Exception as e:
            error = str(e)

        total_duration = time.monotonic() * 1000 - call_start

        return CallResult(
            scenario=config.scenario,
            protocol=Protocol.WEBRTC_AIORTC,
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
        """Close peer connection."""
        if self._pc:
            await self._pc.close()
            self._pc = None


class PlaywrightWebRtcClient:
    """WebRTC client using Playwright for browser smoke tests."""

    def __init__(self, base_url: str) -> None:
        self._base_url = base_url.rstrip("/")
        self._browser: Any = None
        self._page: Any = None
        self._pw: Any = None
        self._events: list[TimestampedEvent] = []

    async def connect(self, config: WebRtcCallConfig) -> None:
        """Launch headless browser."""
        try:
            from playwright.async_api import (
                async_playwright,
            )
        except ImportError:
            raise ImportError(
                "playwright is required for browser "
                "WebRTC testing. Install with: "
                "uv add playwright && "
                "playwright install chromium"
            )

        self._pw = await async_playwright().start()
        self._browser = await self._pw.chromium.launch(
            headless=True,
            args=[
                "--use-fake-device-for-media-stream",
                "--use-fake-ui-for-media-stream",
                "--allow-file-access-from-files",
            ],
        )
        self._page = await self._browser.new_page()
        self._events.clear()

        # Capture console logs for timing
        self._page.on(
            "console",
            lambda msg: self._events.append(
                TimestampedEvent(
                    name="console",
                    timestamp_ms=time.monotonic() * 1000,
                    data={
                        "text": msg.text,
                        "type": msg.type,
                    },
                )
            ),
        )

    async def execute_call(self, config: WebRtcCallConfig) -> CallResult:
        """Execute WebRTC call via real browser."""
        call_start = time.monotonic() * 1000
        error: str | None = None
        setup_latency = 0.0

        try:
            if self._page is None:
                raise RuntimeError("Not connected. Call connect() first.")
            url = f"{self._base_url}/call/webrtc?id={config.session_id}"
            nav_start = time.monotonic() * 1000
            await self._page.goto(url, wait_until="networkidle")
            setup_latency = time.monotonic() * 1000 - nav_start

            # Hold for configured duration
            await asyncio.sleep(config.call_hold_secs)

        except Exception as e:
            error = str(e)

        total_duration = time.monotonic() * 1000 - call_start

        return CallResult(
            scenario=config.scenario,
            protocol=Protocol.WEBRTC_BROWSER,
            codec=config.codec,
            asr_provider=config.asr_provider,
            tts_provider=config.tts_provider,
            setup_latency_ms=setup_latency,
            first_tts_byte_ms=0.0,
            first_asr_result_ms=0.0,
            teardown_ms=0.0,
            total_duration_ms=total_duration,
            success=error is None,
            error=error,
            events=list(self._events),
        )

    async def disconnect(self) -> None:
        """Close browser and Playwright."""
        if self._page:
            await self._page.close()
            self._page = None
        if self._browser:
            await self._browser.close()
            self._browser = None
        if self._pw:
            await self._pw.stop()
            self._pw = None
