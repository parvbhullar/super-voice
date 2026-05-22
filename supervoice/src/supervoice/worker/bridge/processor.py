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
    EndFrame,
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

from supervoice.shared.observability.metrics import CallMetrics

from .client import AgentBridgeClient
from .protocol import (
    AgentAddParticipantVerb,
    AgentDispatchVerb,
    AgentEndCallVerb,
    AgentMergeVerb,
    AgentRemoveParticipantVerb,
    AgentSayVerb,
    AgentTextDeltaEvent,
    AgentTextEndEvent,
    AgentTransferVerb,
    ErrorEvent,
    HelloAckEvent,
    HelloEvent,
    MetricEvent,
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
        metric_interval_s: float = 10.0,
        call_metrics: CallMetrics | None = None,
    ) -> None:
        super().__init__()
        self._echo_mode = echo_mode
        self._client = client
        self._turn_id = 0
        self._consumer_task: asyncio.Task[None] | None = None
        self._metric_task: asyncio.Task[None] | None = None
        self._metric_interval_s = metric_interval_s
        self._call_metrics = call_metrics
        self._response_started = False
        self._end_call_requested = False

    @property
    def end_call_requested(self) -> bool:
        """True after an agent.end_call verb has been processed."""
        return self._end_call_requested

    def attach_client(self, client: AgentBridgeClient) -> None:
        """Inject the bridge client after construction."""
        self._client = client

    # ── error emission ────────────────────────────────────

    async def _emit_error(
        self,
        severity: str,
        source: str,
        code: str,
        message: str,
        retriable: bool = False,
    ) -> None:
        """Push an ErrorEvent upstream if negotiated."""
        if self._client is None:
            return
        if "error" not in self._client.negotiated_events:
            return
        ctx = self._client._context
        call_id = ctx.session_id if ctx else ""
        evt = ErrorEvent(
            call_id=call_id,
            severity=severity,  # type: ignore[arg-type]
            source=source,  # type: ignore[arg-type]
            code=code,
            message=message,
            retriable=retriable,
        )
        try:
            await self._client.send(evt)
        except RuntimeError as e:
            logger.debug(f"error event send skipped: {e}")

    async def start(self) -> None:
        """Start the bridge-consumer and metric-emission tasks."""
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
            self._metric_task = asyncio.create_task(
                self._emit_metrics_loop(),
            )

    async def stop(self) -> None:
        """Cancel and await the bridge-consumer and metric tasks."""
        for task in (self._consumer_task, self._metric_task):
            if task is not None:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
        self._consumer_task = None
        self._metric_task = None

    # ── periodic metric emission ──────────────────────────

    async def _emit_metrics_loop(self) -> None:
        """Emit MetricEvent snapshots at a configurable interval."""
        try:
            while True:
                await asyncio.sleep(self._metric_interval_s)
                await self._emit_metric()
        except asyncio.CancelledError:
            return

    async def _emit_metric(self) -> None:
        """Push a single MetricEvent upstream if negotiated."""
        if self._client is None:
            return
        if "metric" not in self._client.negotiated_events:
            return
        ctx = self._client._context
        call_id = ctx.session_id if ctx else ""
        snap = (
            self._call_metrics.snapshot()
            if self._call_metrics
            else {}
        )
        evt = MetricEvent(
            call_id=call_id,
            ttfa_ms=snap.get("ttfa_ms"),
            asr_p95_ms=snap.get("asr_final_ms"),
            turns=self._turn_id,
        )
        try:
            await self._client.send(evt)
        except RuntimeError as e:
            logger.debug(f"metric event send skipped: {e}")

    async def _consume_bridge(self) -> None:
        assert self._client is not None
        try:
            async for evt in self._client.events():
                # Skip handshake frames in the consumer loop
                # (they are handled by the client internally).
                if isinstance(evt, (HelloEvent, HelloAckEvent)):
                    continue

                # ── v1 events ─────────────────────────────
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

                # ── v2 verbs ──────────────────────────────
                elif isinstance(evt, AgentSayVerb):
                    await self._handle_agent_say(evt)

                elif isinstance(evt, AgentEndCallVerb):
                    await self._handle_agent_end_call(evt)

                elif isinstance(
                    evt,
                    (
                        AgentTransferVerb,
                        AgentDispatchVerb,
                        AgentAddParticipantVerb,
                        AgentRemoveParticipantVerb,
                        AgentMergeVerb,
                    ),
                ):
                    # TODO(phase-5): wire to orchestrator REST API
                    logger.info(
                        "verb %s received; ack only (not wired)",
                        evt.event,
                    )

                else:
                    logger.warning(
                        "unrecognized bridge event: %s",
                        getattr(evt, "event", type(evt).__name__),
                    )
        except asyncio.CancelledError:
            return

    # ── verb handlers ─────────────────────────────────────

    async def _handle_agent_say(self, verb: AgentSayVerb) -> None:
        """Push verbatim text through the TTS pipeline."""
        await self.push_frame(LLMFullResponseStartFrame())
        await self.push_frame(TextFrame(verb.text))
        await self.push_frame(LLMFullResponseEndFrame())

    async def _handle_agent_end_call(
        self, verb: AgentEndCallVerb
    ) -> None:
        """Signal the pipeline to terminate."""
        self._end_call_requested = True
        logger.info(
            "agent.end_call received (reason=%s); pushing EndFrame",
            verb.reason,
        )
        await self.push_frame(EndFrame())

    async def process_frame(
        self,
        frame: Frame,
        direction: FrameDirection,
    ) -> None:
        await super().process_frame(frame, direction)

        if isinstance(frame, TranscriptionFrame):
            try:
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
            except Exception as e:
                logger.error(f"transcription processing error: {e}")
                await self._emit_error(
                    severity="error",
                    source="stt",
                    code="stt.processing_failed",
                    message=str(e),
                    retriable=True,
                )
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
