"""Persistent WSS client to the remote Agent Bridge.

v1: single connection per call, no reconnect (Task 14 adds reconnect).
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

# Sentinel pushed onto the receive queue when the reader loop terminates,
# so that consumers of events() can exit cleanly instead of blocking on get().
_QUEUE_CLOSED: object = object()


class AgentBridgeClient:
    """Persistent WSS client to the remote Agent Bridge.

    The internal receive queue is bounded (maxsize=256) to apply
    backpressure: if downstream consumers cannot keep up with inbound
    bridge events, the reader task will await on queue.put() rather than
    buffering unbounded memory.
    """

    def __init__(self, url: str) -> None:
        self._url = url
        self._ws: Any = None
        self._recv_queue: asyncio.Queue[Any] = asyncio.Queue(maxsize=256)
        self._reader_task: asyncio.Task[None] | None = None

    async def connect(self) -> None:
        """Open the WSS connection and start the background reader.

        Idempotent: a second call while already connected is a no-op.
        """
        if self._ws is not None:
            return
        self._ws = await websockets.connect(self._url)
        self._reader_task = asyncio.create_task(self._read_loop())

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
            logger.info("bridge connection closed")
        finally:
            await self._recv_queue.put(_QUEUE_CLOSED)

    async def send(self, event: BridgeEvent) -> None:
        """Serialize and send a bridge event upstream."""
        if self._ws is None:
            raise RuntimeError("not connected")
        await self._ws.send(event.model_dump_json())

    async def events(self) -> AsyncIterator[BridgeEvent]:
        """Async iterator over downstream bridge events.

        Terminates cleanly when the underlying connection closes.
        """
        while True:
            evt = await self._recv_queue.get()
            if evt is _QUEUE_CLOSED:
                return
            yield evt

    async def close(self) -> None:
        """Cancel the reader task and close the WSS connection."""
        if self._reader_task is not None:
            self._reader_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._reader_task
        if self._ws is not None:
            await self._ws.close()
