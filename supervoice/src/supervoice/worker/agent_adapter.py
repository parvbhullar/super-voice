"""Agent adapter — wraps the V1 PipeCat pipeline + bridge for one dispatched job.

This is the V2 successor to V1's ``run_call_with_profile``. It owns the
lifecycle for a single agent attached to a single room.

For Phase 2 the LiveKit transport is not yet wired in (Task 20). Tests
inject a stub transport factory and mock the pipeline factories.
"""

from __future__ import annotations

import asyncio
import contextlib
from dataclasses import dataclass, field
from typing import Any, Callable

from loguru import logger
from pipecat.pipeline.runner import PipelineRunner
from pipecat.pipeline.task import PipelineParams, PipelineTask
from pydantic import SecretStr

from supervoice.shared.speech.failover import (
    resolve_stt_with_fallback,
    resolve_tts_with_fallback,
)
from supervoice.shared.speech.stt_factory import STTProviderConfig
from supervoice.shared.speech.tts_factory import TTSProviderConfig
from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog
from supervoice.worker.bridge.client import AgentBridgeClient
from supervoice.worker.bridge.processor import AgentBridgeProcessor
from supervoice.worker.pipeline.builder import PipelineConfig, build_pipeline


@dataclass(frozen=True)
class JobContext:
    """Per-job parameters delivered by the orchestrator via a Dispatch frame."""

    job_id: str
    session_id: str
    room: dict[str, Any]
    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict[str, Any] = field(default_factory=dict)


# Factory protocol types — exposed so tests can inject stubs.
BridgeClientFactory = Callable[[str], AgentBridgeClient]
PipelineBuilder = Callable[
    [PipelineConfig], tuple[Any, AgentBridgeProcessor]
]


def _default_bridge_client_factory(url: str) -> AgentBridgeClient:
    return AgentBridgeClient(url=url)


class AgentAdapter:
    """Owns one PipeCat pipeline + one bridge WSS for one dispatched job.

    Lifecycle:
        attach()      -> resolve providers, open bridge, build pipeline,
                         spawn pipeline runner as a background task.
        wait_for_end() -> awaits pipeline termination.
        detach()      -> best-effort cleanup; idempotent; swallows errors.

    The HMAC handshake on the bridge lands in Phase 4 (Task 23); the
    ``agent_secret`` field on the context is stored but not yet
    enforced. Real LiveKit transport lands in Phase 5; for Phase 2 a
    ``transport_factory`` of ``None`` is acceptable.
    """

    def __init__(
        self,
        ctx: JobContext,
        *,
        api_keys: dict[str, SecretStr],
        catalog: VoiceProfileCatalog,
        transport_factory: Callable[[], Any] | None = None,
        bridge_client_factory: BridgeClientFactory = _default_bridge_client_factory,
        pipeline_builder: PipelineBuilder = build_pipeline,
        runner_factory: Callable[[], PipelineRunner] = PipelineRunner,
    ) -> None:
        self.ctx = ctx
        self._api_keys = api_keys
        self._catalog = catalog
        self._transport_factory = transport_factory
        self._bridge_client_factory = bridge_client_factory
        self._pipeline_builder = pipeline_builder
        self._runner_factory = runner_factory
        self._bridge_client: AgentBridgeClient | None = None
        self._bridge_processor: AgentBridgeProcessor | None = None
        self._pipeline_task: asyncio.Task[None] | None = None
        self._ended: asyncio.Event = asyncio.Event()
        self._detached: bool = False

    async def attach(self) -> None:
        """Open bridge, build pipeline, and spawn the pipeline runner."""
        profile = self._catalog.get(self.ctx.voice_profile_id)

        # Resolve providers via failover (validates api_keys + provider).
        resolve_stt_with_fallback(profile, self._api_keys)
        resolve_tts_with_fallback(profile, self._api_keys)

        # Pick the first usable provider/key pair to feed PipelineConfig.
        stt_spec = profile.stt_preference[0]
        tts_spec = profile.tts_preference[0]
        stt_key = self._api_keys.get(stt_spec.provider)
        tts_key = self._api_keys.get(tts_spec.provider)
        if stt_key is None or tts_key is None:
            raise RuntimeError(
                f"missing api key for primary providers in profile {profile.id}"
            )

        # Open bridge WSS to the runner.
        self._bridge_client = self._bridge_client_factory(self.ctx.runner_url)
        await self._bridge_client.connect()

        transport = self._transport_factory() if self._transport_factory else None
        config = PipelineConfig(
            stt=STTProviderConfig(
                provider=stt_spec.provider,
                api_key=stt_key,
                language=stt_spec.language,
            ),
            tts=TTSProviderConfig(
                provider=tts_spec.provider,
                api_key=tts_key,
                voice_id=tts_spec.voice_id,
            ),
            transport=transport,
            echo_mode=False,
        )
        pipeline, bridge = self._pipeline_builder(config)
        bridge.attach_client(self._bridge_client)
        await bridge.start()
        self._bridge_processor = bridge

        task = PipelineTask(pipeline, params=PipelineParams())
        runner = self._runner_factory()

        async def _run_pipeline() -> None:
            try:
                await runner.run(task)
            except asyncio.CancelledError:
                raise
            except Exception as e:  # pragma: no cover - defensive
                logger.exception(f"pipeline raised: {e}")
            finally:
                self._ended.set()

        self._pipeline_task = asyncio.create_task(_run_pipeline())
        logger.info(
            "agent attached job={} session={}",
            self.ctx.job_id,
            self.ctx.session_id,
        )

    async def detach(self, reason: str = "ended") -> None:
        """Tear down bridge + pipeline. Idempotent and exception-safe."""
        if self._detached:
            return
        self._detached = True
        logger.info("detaching agent job={} reason={}", self.ctx.job_id, reason)

        if self._bridge_processor is not None:
            with contextlib.suppress(Exception):
                await self._bridge_processor.stop()
        if self._bridge_client is not None:
            with contextlib.suppress(Exception):
                await self._bridge_client.close()
        if self._pipeline_task is not None:
            self._pipeline_task.cancel()
            with contextlib.suppress(asyncio.CancelledError, Exception):
                await self._pipeline_task
        self._ended.set()

    async def wait_for_end(self) -> None:
        """Block until the pipeline task signals completion."""
        await self._ended.wait()


__all__ = ["AgentAdapter", "JobContext"]
