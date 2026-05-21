"""Per-call session handler.

Builds the supervoice pipeline for a single WebRTC call and drives it via a
``PipelineRunner``. Keeps a ``SessionState`` for the lifetime of the call so
later tasks (idle monitor, metrics, transcript log) can hang off it.

The ``runner_factory`` parameter is injected for testability — tests pass a
``MagicMock``; production callers accept the default ``PipelineRunner`` class.
"""

from __future__ import annotations

import contextlib
from typing import Any, Callable

from loguru import logger
from pipecat.pipeline.runner import PipelineRunner
from pipecat.pipeline.task import PipelineParams, PipelineTask

from supervoice.bridge.client import AgentBridgeClient
from supervoice.pipeline.builder import PipelineConfig, build_pipeline
from supervoice.session.state import SessionState
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


async def run_echo_call(
    session_id: str,
    transport: Any,
    stt: STTProviderConfig,
    tts: TTSProviderConfig,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
) -> None:
    """Build and run an echo-mode pipeline for one call.

    Args:
        session_id: Identifier used for log correlation.
        transport: Pipecat transport (typically ``SmallWebRTCTransport``).
        stt: STT provider configuration.
        tts: TTS provider configuration.
        runner_factory: Callable returning a ``PipelineRunner`` instance.
            Injected so tests can substitute a mock.
    """
    state = SessionState(session_id=session_id)
    config = PipelineConfig(
        stt=stt, tts=tts, transport=transport, echo_mode=True
    )
    pipeline, _bridge = build_pipeline(config)

    task = PipelineTask(pipeline, params=PipelineParams())
    runner = runner_factory()
    logger.info(f"starting echo call session_id={session_id}")
    try:
        await runner.run(task)
    finally:
        state.end()
        logger.info(f"echo call ended session_id={session_id}")


async def run_bridge_call(
    session_id: str,
    transport: Any,
    stt: STTProviderConfig,
    tts: TTSProviderConfig,
    agent_bridge_url: str,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
) -> None:
    """Production call mode: AgentBridgeProcessor talks to remote bridge over WSS."""
    state = SessionState(session_id=session_id)
    client = AgentBridgeClient(url=agent_bridge_url)
    bridge: Any = None

    try:
        await client.connect()

        config = PipelineConfig(
            stt=stt, tts=tts, transport=transport, echo_mode=False
        )
        pipeline, bridge = build_pipeline(config)
        # The pipeline builder creates a fresh bridge processor in echo or
        # WSS mode. For WSS mode (echo_mode=False), the bridge has no client
        # yet — inject ours before start() runs the consumer task.
        bridge.attach_client(client)
        await bridge.start()

        task = PipelineTask(pipeline)
        runner = runner_factory()
        logger.info(f"starting bridge call session_id={session_id}")
        await runner.run(task)
    finally:
        # Each cleanup is independent — one failure must not skip the others.
        if bridge is not None:
            with contextlib.suppress(Exception):
                await bridge.stop()
        with contextlib.suppress(Exception):
            await client.close()
        state.end()
        logger.info(f"bridge call ended session_id={session_id}")
