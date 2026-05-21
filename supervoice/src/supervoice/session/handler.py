"""Per-call session handler.

Builds the supervoice pipeline for a single WebRTC call and drives it via a
``PipelineRunner``. Keeps a ``SessionState`` for the lifetime of the call so
later tasks (idle monitor, metrics, transcript log) can hang off it.

The ``runner_factory`` parameter is injected for testability — tests pass a
``MagicMock``; production callers accept the default ``PipelineRunner`` class.
"""

from __future__ import annotations

from typing import Any, Callable

from loguru import logger
from pipecat.pipeline.runner import PipelineRunner
from pipecat.pipeline.task import PipelineParams, PipelineTask

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
