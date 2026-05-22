"""Agent bridge processor.

Replaces Pipecat's in-process LLM service in the pipeline.

echo_mode=True: echoes a user's transcript back as agent text (v0).
echo_mode=False: ships transcripts via WSS to a remote Agent Bridge and
    streams the agent's text back through LLM-equivalent frames (Task 15).

The processor is handshake-aware (Task 22): after the client completes
the v2 hello/hello.ack exchange, only negotiated events are emitted and
only negotiated verbs are processed.
"""

from __future__ import annotations

import asyncio

from loguru import logger
from pipecat.frames.frames import (
    Frame,
    InterruptionFrame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    TextFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import (
    FrameDirection,
    FrameProcessor,
)

from .client import AgentBridgeClient
from .protocol import (
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    HelloAckEvent,
    HelloEvent,
    UserInterruptEvent,
    UserTextEvent,
)


class AgentBridgeProcessor(FrameProcessor):
    """Pipecat processor that owns the LLM boundary.

    Args:
        echo_mode: When ``True`` (v0), transcripts are echoed back as
            agent text. When ``False`` (v1+), the processor ships
            transcripts to a remote Agent Bridge over WSS and streams
            agent text back.
        client: The WSS bridge client. Required when ``echo_mode`` is
            ``False``. May be passed at construction or via
            :meth:`attach_client`.

    NOTE: ``TranscriptionFrame`` is a subclass of ``TextFrame`` in
    Pipecat 1.2.1 -- code that branches on frame type must check
    ``isinstance(frame, TranscriptionFrame)`` BEFORE plain
    ``TextFrame``, otherwise both branches will fire.
    """

    def __init__(
        self,
        echo_mode: bool = False,
        client: AgentBridgeClient | None = None,
    ) -> None:
        super().__init__()
        self._echo_mode = echo_mode
        self._client = client
        self._turn_id = 0
        self._consumer_task: asyncio.Task[None] | None = None
        self._response_started = False

    def attach_client(self, client: AgentBridgeClient) -> None:
        """Inject the bridge client after construction."""
        self._client = client

    async def start(self) -> None:
        """Start the bridge-consumer task in WSS mode."""
        if not self._echo_mode and self._client is None:
            raise RuntimeError(
                "AgentBridgeProcessor in WSS mode requires a "
                "client; pass one via constructor or "
                "attach_client()."
            )
        if self._client is not None:
            self._consumer_task = asyncio.create_task(
                self._consume_bridge(),
            )

    async def stop(self) -> None:
        """Cancel and await the bridge-consumer task."""
        if self._consumer_task is not None:
            self._consumer_task.cancel()
            try:
                await self._consumer_task
            except asyncio.CancelledError:
                pass

    async def _consume_bridge(self) -> None:
        assert self._client is not None
        try:
            async for evt in self._client.events():
                # Skip handshake frames in the consumer loop
                # (they are handled by the client internally).
                if isinstance(evt, (HelloEvent, HelloAckEvent)):
                    continue
                if isinstance(evt, AgentTextDeltaEvent):
                    if not self._response_started:
                        await self.push_frame(
                            LLMFullResponseStartFrame(),
                        )
                        self._response_started = True
                    await self.push_frame(TextFrame(evt.text))
                elif isinstance(evt, AgentTextEndEvent):
                    if self._response_started:
                        await self.push_frame(
                            LLMFullResponseEndFrame(),
                        )
                        self._response_started = False
        except asyncio.CancelledError:
            return

    async def process_frame(
        self,
        frame: Frame,
        direction: FrameDirection,
    ) -> None:
        await super().process_frame(frame, direction)

        if isinstance(frame, TranscriptionFrame):
            if self._echo_mode:
                self._turn_id += 1
                await self.push_frame(
                    LLMFullResponseStartFrame(),
                )
                await self.push_frame(
                    TextFrame(f"You said: {frame.text}"),
                )
                await self.push_frame(
                    LLMFullResponseEndFrame(),
                )
                return
            if self._client is not None:
                self._turn_id += 1
                await self._client.send(
                    UserTextEvent(
                        turn_id=self._turn_id,
                        text=frame.text,
                        final=True,
                    )
                )
                return
            # No client attached and not echo: pass through.
            await self.push_frame(frame, direction)
            return

        if isinstance(frame, InterruptionFrame) and self._client is not None:
            try:
                await self._client.send(
                    UserInterruptEvent(turn_id=self._turn_id),
                )
            except RuntimeError as e:
                logger.debug(f"interrupt send skipped: {e}")

        # Pass-through for all other frames.
        await self.push_frame(frame, direction)
