from __future__ import annotations

import uuid
from dataclasses import dataclass, field
from typing import Any

from active_call_tester.clients.ws import WsCallConfig, WsClient
from active_call_tester.models import CallResult, Protocol


@dataclass
class SipCallConfig:
    """Configuration for a SIP call test."""

    scenario: str
    codec: str = "pcmu"
    asr_provider: str = "sensevoice"
    tts_provider: str = "supertonic"
    callee: str = "sip:test@localhost:5060"
    caller: str = "sip:tester@localhost"
    tts_text: str = "Hello, this is a SIP test call."
    call_hold_secs: float = 5.0
    session_id: str = field(default_factory=lambda: f"sip-test-{uuid.uuid4().hex[:8]}")
    # SIP-specific options
    username: str | None = None
    password: str | None = None
    realm: str | None = None
    enable_srtp: bool = False
    sip_headers: dict[str, str] = field(default_factory=dict)
    transport: str = "udp"  # udp, tcp, tls


class SipClient:
    """SIP client that wraps WsClient with SIP-specific options."""

    def __init__(self, base_url: str) -> None:
        ws_url = base_url.replace("http://", "ws://").replace("https://", "wss://")
        self._sip_ws_url = f"{ws_url.rstrip('/')}/call/sip"
        self._ws_client: WsClient | None = None

    async def connect(self, config: SipCallConfig) -> None:
        """Create WS connection to SIP endpoint."""
        self._ws_client = WsClient(self._sip_ws_url)
        ws_config = self._to_ws_config(config)
        await self._ws_client.connect(ws_config)

    def _build_sip_options(self, config: SipCallConfig) -> dict[str, Any]:
        """Build SIP-specific options for Invite command."""
        sip_opts: dict[str, Any] = {}
        if config.username:
            sip_opts["username"] = config.username
        if config.password:
            sip_opts["password"] = config.password
        if config.realm:
            sip_opts["realm"] = config.realm
        if config.enable_srtp:
            sip_opts["enable_srtp"] = True
        if config.sip_headers:
            sip_opts["headers"] = config.sip_headers

        result: dict[str, Any] = {}
        if sip_opts:
            result["sip"] = sip_opts
        if config.caller:
            result["caller"] = config.caller
        return result

    def _to_ws_config(self, config: SipCallConfig) -> WsCallConfig:
        """Convert SipCallConfig to WsCallConfig."""
        return WsCallConfig(
            scenario=config.scenario,
            codec=config.codec,
            asr_provider=config.asr_provider,
            tts_provider=config.tts_provider,
            callee=config.callee,
            tts_text=config.tts_text,
            call_hold_secs=config.call_hold_secs,
            session_id=config.session_id,
            extra_options=self._build_sip_options(config),
        )

    async def execute_call(self, config: SipCallConfig) -> CallResult:
        """Execute SIP call lifecycle."""
        if self._ws_client is None:
            raise RuntimeError("Not connected. Call connect() first.")
        ws_config = self._to_ws_config(config)
        result = await self._ws_client.execute_call(ws_config)
        # Override protocol to SIP
        result.protocol = Protocol.SIP
        return result

    async def disconnect(self) -> None:
        """Disconnect underlying WS client."""
        if self._ws_client:
            await self._ws_client.disconnect()
            self._ws_client = None
