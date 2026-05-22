"""Worker-side registration loop.

Maintains the WSS link to the orchestrator. On open: send ``Register``;
expect ``Registered``; then run two concurrent loops:

* Heartbeat — every ``heartbeat_interval_s`` send a ``Heartbeat`` with
  the current active-jobs count.
* Recv loop — read ``Dispatch`` frames, hand them to the supplied
  ``dispatch_handler`` and reply with ``DispatchAck``.

Reconnects are bounded with a short delay; supervisor exits cleanly on
``close()``.

The WSS layer is injectable via a ``link_factory`` so unit tests can
exercise the protocol logic using in-memory queues without spinning up
a real WebSocket server.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
from typing import Any, Awaitable, Callable, Protocol

import websockets
from loguru import logger

from supervoice.shared.dispatch_protocol import (
    Dispatch,
    DispatchAck,
    Heartbeat,
    Register,
    Registered,
    WorkerCapabilities,
    parse_frame,
)


SendFrame = Callable[[dict[str, Any]], Awaitable[None]]
RecvFrame = Callable[[], Awaitable[dict[str, Any]]]
DispatchHandler = Callable[[Dispatch], Awaitable[bool]]


class WorkerLink(Protocol):
    """Bidirectional frame channel — abstracts away the WSS transport."""

    async def send(self, frame: dict[str, Any]) -> None: ...
    async def recv(self) -> dict[str, Any]: ...
    async def close(self) -> None: ...


class _WebsocketLink:
    """Default ``WorkerLink`` backed by ``websockets``."""

    def __init__(self, ws: Any) -> None:
        self._ws = ws

    async def send(self, frame: dict[str, Any]) -> None:
        await self._ws.send(json.dumps(frame))

    async def recv(self) -> dict[str, Any]:
        raw = await self._ws.recv()
        return json.loads(raw)

    async def close(self) -> None:
        with contextlib.suppress(Exception):
            await self._ws.close()


LinkFactory = Callable[[str, str], Awaitable[WorkerLink]]


async def _default_link_factory(url: str, secret: str) -> WorkerLink:
    ws = await websockets.connect(
        url,
        additional_headers={"Authorization": f"Bearer {secret}"},
    )
    return _WebsocketLink(ws)


class WorkerRegistration:
    """Worker-side registration / dispatch-acceptance loop.

    Lifecycle:
        run()   -> repeatedly connect + serve until close().
        close() -> mark closed and tear down current link.

    A single ``serve_once()`` iteration:
        1. Open link via ``link_factory(url, secret)``.
        2. Send ``Register``.
        3. Receive ``Registered`` — adopt ``heartbeat_interval_s``.
        4. Run heartbeat + recv loops concurrently until either ends.

    The ``active_jobs_counter`` callable MUST be cheap and non-blocking
    (it runs on every heartbeat) and MUST NOT acquire the JobRunner
    lock — see Hazards in the implementation plan.
    """

    def __init__(
        self,
        *,
        orchestrator_url: str,
        shared_secret: str,
        worker_id: str,
        pool: str,
        capabilities: WorkerCapabilities,
        dispatch_handler: DispatchHandler,
        active_jobs_counter: Callable[[], int],
        link_factory: LinkFactory = _default_link_factory,
        reconnect_delay_s: float = 2.0,
    ) -> None:
        self._url = orchestrator_url
        self._secret = shared_secret
        self._worker_id = worker_id
        self._pool = pool
        self._capabilities = capabilities
        self._dispatch_handler = dispatch_handler
        self._active_jobs_counter = active_jobs_counter
        self._link_factory = link_factory
        self._reconnect_delay_s = reconnect_delay_s
        self._heartbeat_interval_s: int = 10
        self._closed = False
        self._link: WorkerLink | None = None

    async def run(self) -> None:
        """Outer loop — reconnect until ``close()`` is called."""
        while not self._closed:
            try:
                await self.serve_once()
            except Exception as e:
                logger.warning(f"worker registration loop error: {e}")
            if self._closed:
                return
            await asyncio.sleep(self._reconnect_delay_s)

    async def serve_once(self) -> None:
        """Open the link, register, and serve heartbeats + dispatches."""
        link = await self._link_factory(self._url, self._secret)
        self._link = link
        try:
            await self._handshake(link)
            hb_task = asyncio.create_task(self._heartbeat_loop(link))
            recv_task = asyncio.create_task(self._recv_loop(link))
            try:
                _done, pending = await asyncio.wait(
                    {hb_task, recv_task},
                    return_when=asyncio.FIRST_COMPLETED,
                )
                for t in pending:
                    t.cancel()
                    with contextlib.suppress(asyncio.CancelledError):
                        await t
            finally:
                for t in (hb_task, recv_task):
                    if not t.done():
                        t.cancel()
        finally:
            await link.close()
            self._link = None

    async def _handshake(self, link: WorkerLink) -> None:
        register = Register(
            worker_id=self._worker_id,
            pool=self._pool,
            capabilities=self._capabilities,
        )
        await link.send(register.model_dump())
        raw = await link.recv()
        frame = parse_frame(raw)
        if not isinstance(frame, Registered):
            raise RuntimeError(
                f"expected Registered, got {type(frame).__name__}"
            )
        self._heartbeat_interval_s = frame.heartbeat_interval_s
        logger.info(
            "worker registered worker_id={} pool={} heartbeat={}s",
            self._worker_id,
            self._pool,
            self._heartbeat_interval_s,
        )

    async def _heartbeat_loop(self, link: WorkerLink) -> None:
        while not self._closed:
            await asyncio.sleep(self._heartbeat_interval_s)
            if self._closed:
                return
            hb = Heartbeat(active_jobs=self._active_jobs_counter())
            await link.send(hb.model_dump())

    async def _recv_loop(self, link: WorkerLink) -> None:
        while not self._closed:
            try:
                raw = await link.recv()
            except (ConnectionError, OSError) as e:
                logger.info(f"orchestrator link closed: {e}")
                return
            try:
                frame = parse_frame(raw)
            except (ValueError, json.JSONDecodeError) as e:
                logger.warning(f"invalid frame from orchestrator: {e}")
                continue
            if isinstance(frame, Dispatch):
                await self._handle_dispatch(link, frame)
            else:
                logger.debug(
                    "ignoring unexpected frame type from orchestrator: {}",
                    type(frame).__name__,
                )

    async def _handle_dispatch(self, link: WorkerLink, frame: Dispatch) -> None:
        try:
            accepted = await self._dispatch_handler(frame)
            reason: str | None = None if accepted else "no_slot"
        except Exception as e:
            logger.exception(f"dispatch handler raised for {frame.job_id}: {e}")
            accepted = False
            reason = "handler_error"
        ack = DispatchAck(
            job_id=frame.job_id,
            status="accepted" if accepted else "rejected",
            reason=reason,
        )
        await link.send(ack.model_dump())

    async def close(self) -> None:
        self._closed = True
        if self._link is not None:
            await self._link.close()


__all__ = [
    "DispatchHandler",
    "LinkFactory",
    "WorkerLink",
    "WorkerRegistration",
]
