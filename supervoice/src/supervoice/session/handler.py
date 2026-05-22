"""Per-call session handler.

Builds the supervoice pipeline for a single WebRTC call and drives it via a
``PipelineRunner``. Keeps a ``SessionState`` for the lifetime of the call so
later tasks (idle monitor, metrics, transcript log) can hang off it.

The ``runner_factory`` parameter is injected for testability — tests pass a
``MagicMock``; production callers accept the default ``PipelineRunner`` class.
"""

from __future__ import annotations

import asyncio
import contextlib
from typing import Any, Callable

from loguru import logger
from pipecat.pipeline.runner import PipelineRunner
from pipecat.pipeline.task import PipelineParams, PipelineTask
from pydantic import SecretStr

from supervoice.bridge.client import AgentBridgeClient
from supervoice.pipeline.builder import PipelineConfig, build_pipeline
from supervoice.session.idle_monitor import IdleMonitor
from supervoice.session.state import SessionState
from supervoice.speech.failover import (
    resolve_stt_with_fallback,
    resolve_tts_with_fallback,
)
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig
from supervoice.voice_profile.catalog import VoiceProfileCatalog


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
    config = PipelineConfig(stt=stt, tts=tts, transport=transport, echo_mode=True)
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

        config = PipelineConfig(stt=stt, tts=tts, transport=transport, echo_mode=False)
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


async def run_call_with_profile(
    session_id: str,
    transport: Any,
    profile_id: str,
    api_keys: dict[str, SecretStr],
    agent_bridge_url: str,
    runner_factory: Callable[..., PipelineRunner] = PipelineRunner,
    idle_warning_at_s: float = 30.0,
    idle_disconnect_at_s: float = 60.0,
) -> None:
    """Production call mode with voice-profile-driven STT/TTS + idle monitor.

    Resolves STT and TTS providers from the voice profile catalog with
    fallback, then assembles the pipeline manually so the pre-resolved
    services can be injected. Also launches a background ``IdleMonitor``
    task that fires warnings and a disconnect callback after inactivity.
    """
    catalog = VoiceProfileCatalog.load_default()
    profile = catalog.get(profile_id)  # raises KeyError if unknown
    state = SessionState(session_id=session_id, voice_profile_id=profile_id)

    stt_service = resolve_stt_with_fallback(profile, api_keys)
    tts_service = resolve_tts_with_fallback(profile, api_keys)

    client = AgentBridgeClient(url=agent_bridge_url)
    bridge: Any = None
    monitor_task: asyncio.Task[None] | None = None

    try:
        await client.connect()

        # Custom pipeline assembly (bypass build_pipeline so we can inject
        # pre-resolved STT/TTS services from the profile).
        from pipecat.pipeline.pipeline import Pipeline

        from supervoice.bridge.processor import AgentBridgeProcessor
        from supervoice.pipeline.builder import TTSSanitizeFilter

        bridge = AgentBridgeProcessor(echo_mode=False)
        bridge.attach_client(client)

        processors = [
            transport.input(),
            stt_service,
            bridge,
            TTSSanitizeFilter(),
            tts_service,
            transport.output(),
        ]
        pipeline = Pipeline(processors)

        await bridge.start()

        # Idle monitor: shutdown the runner if user goes silent too long.
        state.mark_idle()
        monitor_task = asyncio.create_task(
            IdleMonitor(
                state=state,
                warning_at_s=idle_warning_at_s,
                disconnect_at_s=idle_disconnect_at_s,
                on_warning=lambda lvl: logger.warning(
                    f"idle warning level {lvl} session_id={session_id}"
                ),
                on_disconnect=lambda: logger.info(
                    f"idle disconnect session_id={session_id}"
                ),
                poll_interval_s=1.0,
            ).run()
        )

        task = PipelineTask(pipeline)
        runner = runner_factory()
        logger.info(
            f"starting profile call session_id={session_id} profile={profile_id}"
        )
        await runner.run(task)
    finally:
        if monitor_task is not None:
            monitor_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await monitor_task
        if bridge is not None:
            with contextlib.suppress(Exception):
                await bridge.stop()
        with contextlib.suppress(Exception):
            await client.close()
        state.end()
        logger.info(f"profile call ended session_id={session_id}")
