"""Persistent WSS client to the remote Agent Bridge with reconnect.

Task 14: exponential-backoff reconnect supervisor.
Task 22: v2 handshake with version negotiation.
Task 23: HMAC-signed runner connection.
"""

from __future__ import annotations

import asyncio
import base64
import contextlib
import hashlib
import hmac
import json
import os
import time
from collections.abc import AsyncIterator
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlencode, urlparse, urlunparse

import websockets
from loguru import logger

from .protocol import (
    BridgeEvent,
    HelloAckEvent,
    HelloEvent,
    V1_EVENTS,
    V1_VERBS,
    parse_event,
)

# Sentinel pushed onto the receive queue when the supervisor exits,
# so that consumers of events() can exit cleanly instead of blocking
# on get().
_QUEUE_CLOSED: object = object()

# The full v2 event/verb sets that this worker understands.
_WORKER_SUPPORTED_EVENTS: frozenset[str] = frozenset(
    {
        "user.text",
        "user.interrupted",
        "error",
        "metric",
        "call.started",
        "call.ended",
        "call.migrated_to",
        "call.merged_in",
    }
)
_WORKER_SUPPORTED_VERBS: frozenset[str] = frozenset(
    {
        "agent.text.delta",
        "agent.text.end",
        "agent.say",
        "agent.transfer",
        "agent.dispatch",
        "agent.add_participant",
        "agent.remove_participant",
        "agent.merge",
        "agent.end_call",
    }
)


@dataclass
class BridgeContext:
    """Identifiers populated before connect, sent in hello.ack."""

    session_id: str
    job_id: str
    room_id: str
    agent_secret: str = ""


class AgentBridgeClient:
    """Persistent WSS client with exponential-backoff reconnect.

    The supervisor task connects, runs the read loop, and on disconnect
    waits ``reconnect_initial_delay_ms * 2^(attempt-1)`` ms before
    retrying. After ``reconnect_max_attempts`` consecutive failures it
    gives up and the client is closed.

    ``_recv_queue`` has bounded size 256 -- slow consumers apply
    backpressure. ``send()`` waits for the supervisor to establish a
    connection before delivering the frame.
    """

    def __init__(
        self,
        url: str,
        reconnect_max_attempts: int = 5,
        reconnect_initial_delay_ms: int = 200,
        reconnect_max_delay_ms: int = 30000,
        context: BridgeContext | None = None,
    ) -> None:
        self._url = url
        self._ws: Any = None
        self._recv_queue: asyncio.Queue[Any] = asyncio.Queue(
            maxsize=256,
        )
        self._supervisor_task: asyncio.Task[None] | None = None
        self._closed = False
        self._reconnect_max = reconnect_max_attempts
        self._reconnect_initial_ms = reconnect_initial_delay_ms
        self._reconnect_max_delay_ms = reconnect_max_delay_ms
        self._connected = asyncio.Event()

        # v2 handshake state
        self._context: BridgeContext | None = context
        self._protocol_version: int = 1
        self._negotiated_events: frozenset[str] = V1_EVENTS
        self._negotiated_verbs: frozenset[str] = V1_VERBS
        self._handshake_done = False

    # ── public properties ──────────────────────────────────

    @property
    def protocol_version(self) -> int:
        """Negotiated protocol version (1 until handshake completes)."""
        return self._protocol_version

    @property
    def negotiated_events(self) -> frozenset[str]:
        """Events the runner accepts from us."""
        return self._negotiated_events

    @property
    def negotiated_verbs(self) -> frozenset[str]:
        """Verbs the runner may send to us."""
        return self._negotiated_verbs

    def set_context(
        self,
        session_id: str,
        job_id: str,
        room_id: str,
        agent_secret: str = "",
    ) -> None:
        """Set call context before connect (required for hello.ack)."""
        self._context = BridgeContext(
            session_id=session_id,
            job_id=job_id,
            room_id=room_id,
            agent_secret=agent_secret,
        )

    # ── HMAC URL signing ────────────────────────────────────

    def _build_signed_url(self) -> str:
        """Append HMAC query params to the base URL if agent_secret
        is available. Returns the original URL unchanged when no
        secret is configured (backward compat)."""
        ctx = self._context
        if ctx is None or not ctx.agent_secret:
            return self._url

        nonce = base64.b64encode(os.urandom(16)).decode()
        ts = str(int(time.time() * 1000))
        msg = f"{ctx.session_id}|{ctx.job_id}|{nonce}|{ts}"
        sig = base64.b64encode(
            hmac.new(
                ctx.agent_secret.encode(),
                msg.encode(),
                hashlib.sha256,
            ).digest()
        ).decode()

        params = urlencode(
            {
                "session_id": ctx.session_id,
                "job_id": ctx.job_id,
                "nonce": nonce,
                "ts": ts,
                "signature": sig,
            }
        )
        parsed = urlparse(self._url)
        # Preserve any existing query params
        existing_q = parsed.query
        new_q = f"{existing_q}&{params}" if existing_q else params
        return urlunparse(parsed._replace(query=new_q))

    # ── lifecycle ──────────────────────────────────────────

    async def connect(self) -> None:
        """Start the supervisor task and wait for the first connection.

        Returns once the first connection is established OR the
        supervisor has exhausted its retries and exited.
        Idempotent: a second call while supervisor is running is a
        no-op.
        """
        if self._supervisor_task is not None:
            return
        self._supervisor_task = asyncio.create_task(self._supervise())
        connect_or_giveup = asyncio.create_task(
            self._connected.wait(),
        )
        _, pending = await asyncio.wait(
            {connect_or_giveup, self._supervisor_task},
            return_when=asyncio.FIRST_COMPLETED,
        )
        for t in pending:
            if t is connect_or_giveup:
                t.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await t

    async def _supervise(self) -> None:
        attempt = 0
        try:
            while not self._closed:
                try:
                    self._ws = await websockets.connect(
                        self._build_signed_url(),
                    )
                    self._connected.set()
                    attempt = 0
                    await self._handshake()
                    await self._read_loop()
                except (
                    OSError,
                    websockets.WebSocketException,
                ) as e:
                    logger.warning(f"bridge connect failed: {e}")
                if self._closed:
                    return
                attempt += 1
                if attempt > self._reconnect_max:
                    logger.error(
                        "bridge reconnect exhausted; giving up",
                    )
                    return
                delay_ms = self._reconnect_initial_ms * (2 ** (attempt - 1))
                delay_ms = min(delay_ms, self._reconnect_max_delay_ms)
                self._connected.clear()
                await asyncio.sleep(delay_ms / 1000.0)
        finally:
            self._closed = True
            self._connected.set()
            with contextlib.suppress(asyncio.QueueFull):
                self._recv_queue.put_nowait(_QUEUE_CLOSED)

    # ── v2 handshake ───────────────────────────────────────

    async def _handshake(self) -> None:
        """Perform the v2 hello/hello.ack handshake.

        If the first frame is not a hello event, treat the connection
        as v1-compatible and enqueue the frame for normal processing.
        """
        if self._ws is None:
            return

        try:
            raw_msg = await asyncio.wait_for(
                self._ws.recv(),
                timeout=5.0,
            )
        except asyncio.TimeoutError:
            logger.warning(
                "bridge handshake timeout; assuming v1 compat",
            )
            self._set_v1_defaults()
            return

        try:
            raw = json.loads(raw_msg)
        except json.JSONDecodeError as e:
            logger.warning(f"invalid bridge frame during handshake: {e}")
            self._set_v1_defaults()
            return

        if raw.get("event") != "hello":
            # v1 runner that doesn't know about handshakes.
            logger.warning(
                "first bridge frame is not hello; assuming v1 compat (event=%s)",
                raw.get("event"),
            )
            self._set_v1_defaults()
            # Enqueue the frame so it's not lost.
            try:
                evt = parse_event(raw)
                await self._recv_queue.put(evt)
            except ValueError as e:
                logger.warning(f"invalid bridge frame: {e}")
            return

        hello = HelloEvent.model_validate(raw)
        self._negotiate(hello)

        # Send hello.ack
        ctx = self._context
        ack = HelloAckEvent(
            protocol_version=self._protocol_version,
            negotiated_events=sorted(self._negotiated_events),
            negotiated_verbs=sorted(self._negotiated_verbs),
            call_id=ctx.session_id if ctx else "",
            session_id=ctx.session_id if ctx else "",
            job_id=ctx.job_id if ctx else "",
            room_id=ctx.room_id if ctx else "",
        )
        await self._ws.send(ack.model_dump_json())
        self._handshake_done = True
        logger.info(
            "bridge handshake complete: v%d, %d events, %d verbs",
            self._protocol_version,
            len(self._negotiated_events),
            len(self._negotiated_verbs),
        )

    def _negotiate(self, hello: HelloEvent) -> None:
        """Compute negotiated capabilities from a hello frame."""
        if hello.protocol_version <= 1:
            self._set_v1_defaults()
            return

        self._protocol_version = hello.protocol_version
        runner_events = frozenset(hello.supported_events)
        runner_verbs = frozenset(hello.supported_verbs)
        self._negotiated_events = _WORKER_SUPPORTED_EVENTS & runner_events
        self._negotiated_verbs = _WORKER_SUPPORTED_VERBS & runner_verbs

    def _set_v1_defaults(self) -> None:
        """Reset to v1 4-event set."""
        self._protocol_version = 1
        self._negotiated_events = V1_EVENTS
        self._negotiated_verbs = V1_VERBS
        self._handshake_done = False

    # ── read loop ──────────────────────────────────────────

    async def _read_loop(self) -> None:
        if self._ws is None:
            return
        try:
            async for raw in self._ws:
                try:
                    evt = parse_event(json.loads(raw))
                except (ValueError, json.JSONDecodeError) as e:
                    logger.warning(f"invalid bridge frame: {e}")
                    continue
                await self._recv_queue.put(evt)
        except websockets.ConnectionClosed:
            logger.info(
                "bridge connection closed; will reconnect",
            )

    # ── send / receive ─────────────────────────────────────

    async def send(self, event: BridgeEvent) -> None:
        """Serialize and send a bridge event upstream.

        Waits until the supervisor has established a live connection.
        """
        if self._closed:
            raise RuntimeError("client is closed")
        await self._connected.wait()
        if self._closed:
            raise RuntimeError("client is closed")
        if self._ws is None:
            raise RuntimeError("not connected")
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        """Async iterator over downstream bridge events.

        Terminates cleanly when the supervisor exits (give-up or
        close).
        """
        while True:
            evt = await self._recv_queue.get()
            if evt is _QUEUE_CLOSED:
                return
            yield evt

    async def close(self) -> None:
        """Cancel the supervisor and close the WSS connection."""
        self._closed = True
        if self._ws is not None:
            with contextlib.suppress(Exception):
                await self._ws.close()
        if self._supervisor_task is not None:
            self._supervisor_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._supervisor_task
