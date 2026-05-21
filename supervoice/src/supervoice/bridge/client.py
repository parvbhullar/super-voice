"""Persistent WSS client to the remote Agent Bridge with reconnect.

Task 14: exponential-backoff reconnect supervisor.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
from collections.abc import AsyncIterator
from typing import Any

import websockets
from loguru import logger

from .protocol import BridgeEvent, parse_event

# Sentinel pushed onto the receive queue when the supervisor exits,
# so that consumers of events() can exit cleanly instead of blocking on get().
_QUEUE_CLOSED: object = object()


class AgentBridgeClient:
    """Persistent WSS client with exponential-backoff reconnect.

    The supervisor task connects, runs the read loop, and on disconnect
    waits ``reconnect_initial_delay_ms * 2^(attempt-1)`` ms before
    retrying. After ``reconnect_max_attempts`` consecutive failures it
    gives up and the client is closed.

    ``_recv_queue`` has bounded size 256 — slow consumers apply
    backpressure. ``send()`` waits for the supervisor to establish a
    connection before delivering the frame.
    """

    def __init__(
        self,
        url: str,
        reconnect_max_attempts: int = 5,
        reconnect_initial_delay_ms: int = 200,
        reconnect_max_delay_ms: int = 30000,
    ) -> None:
        self._url = url
        self._ws: Any = None
        self._recv_queue: asyncio.Queue[Any] = asyncio.Queue(maxsize=256)
        self._supervisor_task: asyncio.Task[None] | None = None
        self._closed = False
        self._reconnect_max = reconnect_max_attempts
        self._reconnect_initial_ms = reconnect_initial_delay_ms
        self._reconnect_max_delay_ms = reconnect_max_delay_ms
        self._connected = asyncio.Event()

    async def connect(self) -> None:
        """Start the supervisor task and wait for the first connection.

        Returns once the first connection is established OR the
        supervisor has exhausted its retries and exited.
        Idempotent: a second call while supervisor is running is a no-op.
        """
        if self._supervisor_task is not None:
            return
        self._supervisor_task = asyncio.create_task(self._supervise())
        connect_or_giveup = asyncio.create_task(self._connected.wait())
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
                    self._ws = await websockets.connect(self._url)
                    self._connected.set()
                    attempt = 0
                    await self._read_loop()
                except (OSError, websockets.WebSocketException) as e:
                    logger.warning(f"bridge connect failed: {e}")
                if self._closed:
                    return
                attempt += 1
                if attempt > self._reconnect_max:
                    logger.error("bridge reconnect exhausted; giving up")
                    return
                delay_ms = self._reconnect_initial_ms * (2 ** (attempt - 1))
                delay_ms = min(delay_ms, self._reconnect_max_delay_ms)
                self._connected.clear()
                await asyncio.sleep(delay_ms / 1000.0)
        finally:
            # Mark closed so send() raises immediately instead of hanging on
            # _connected.wait(). If close() was already called this is a no-op.
            self._closed = True
            # Unblock anyone awaiting connection — they'll observe _closed.
            self._connected.set()
            # Ensure events() consumers unblock.
            with contextlib.suppress(asyncio.QueueFull):
                self._recv_queue.put_nowait(_QUEUE_CLOSED)

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
            logger.info("bridge connection closed; will reconnect")

    async def send(self, event: BridgeEvent) -> None:
        """Serialize and send a bridge event upstream.

        Waits until the supervisor has established a live connection.
        """
        if self._closed:
            raise RuntimeError("client is closed")
        await self._connected.wait()
        if self._closed:
            # Supervisor exited (give-up or close) while we were waiting.
            raise RuntimeError("client is closed")
        if self._ws is None:
            raise RuntimeError("not connected")
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        """Async iterator over downstream bridge events.

        Terminates cleanly when the supervisor exits (give-up or close).
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
