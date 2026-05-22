"""Worker entrypoint — V2 dispatch-driven runtime.

Usage::

    uv run python -m supervoice.worker.main \\
        --orchestrator-url ws://localhost:8090/v1/internal/workers \\
        --shared-secret <secret> \\
        --pool default \\
        --voice-profiles hi-female,en-female \\
        --max-concurrent 50

This replaces V1's FastAPI-app entrypoint. The V1 ``/call`` WebSocket
endpoint is reintroduced as a thin shim under the orchestrator in
Phase 3 / Task 21.

Phase 2 wiring note: ``JobRunner.upstream_send`` frames are routed
through an in-process queue and logged; actual delivery through the
registration's WSS happens once the orchestrator-side endpoint can
accept ``StateChanged`` / ``JobCompleted`` mid-stream (Task 21).
"""

from __future__ import annotations

import argparse
import asyncio
import os
import uuid
from typing import Any

from loguru import logger
from pydantic import SecretStr

from supervoice.shared.dispatch_protocol import WorkerCapabilities
from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog

from .job_runner import JobRunner
from .registration import WorkerRegistration


_API_KEY_ENV: dict[str, str] = {
    "deepgram": "DEEPGRAM_API_KEY",
    "cartesia": "CARTESIA_API_KEY",
    "elevenlabs": "ELEVENLABS_API_KEY",
}


def build_api_keys_from_env(
    env: dict[str, str] | None = None,
) -> dict[str, SecretStr]:
    """Read provider API keys from process environment."""
    env = env if env is not None else dict(os.environ)
    keys: dict[str, SecretStr] = {}
    for provider, env_name in _API_KEY_ENV.items():
        v = env.get(env_name)
        if v:
            keys[provider] = SecretStr(v)
    return keys


class Worker:
    """Top-level worker runtime — wires registration + job runner."""

    def __init__(
        self,
        *,
        orchestrator_url: str,
        shared_secret: str,
        worker_id: str,
        pool: str,
        voice_profiles: list[str],
        max_concurrent: int,
        catalog: VoiceProfileCatalog,
        api_keys: dict[str, SecretStr],
    ) -> None:
        self._upstream_outbound: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

        async def upstream_send(frame: dict[str, Any]) -> None:
            await self._upstream_outbound.put(frame)

        self.job_runner = JobRunner(
            max_concurrent=max_concurrent,
            api_keys=api_keys,
            catalog=catalog,
            upstream_send=upstream_send,
        )

        capabilities = WorkerCapabilities(
            voice_profiles=voice_profiles,
            max_concurrent=max_concurrent,
        )
        self.registration = WorkerRegistration(
            orchestrator_url=orchestrator_url,
            shared_secret=shared_secret,
            worker_id=worker_id,
            pool=pool,
            capabilities=capabilities,
            dispatch_handler=self.job_runner.accept,
            active_jobs_counter=self.job_runner.active_count,
        )

    async def run(self) -> None:
        drain_task = asyncio.create_task(self._drain_outbound())
        try:
            await self.registration.run()
        finally:
            drain_task.cancel()

    async def _drain_outbound(self) -> None:
        # Phase 2 stub — Phase 3 / Task 21 wires this through the WSS.
        while True:
            frame = await self._upstream_outbound.get()
            logger.info(f"upstream frame (phase-2 stub): {frame}")

    async def shutdown(self) -> None:
        await self.registration.close()
        await self.job_runner.shutdown()


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="supervoice.worker")
    parser.add_argument("--orchestrator-url", required=True)
    parser.add_argument("--shared-secret", required=True)
    parser.add_argument("--pool", default="default")
    parser.add_argument(
        "--voice-profiles",
        required=True,
        help="Comma-separated voice_profile_ids this worker can serve.",
    )
    parser.add_argument("--max-concurrent", type=int, default=50)
    parser.add_argument(
        "--worker-id",
        default=None,
        help="Stable id; defaults to a fresh UUID-based id.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = _parse_args(argv)
    worker_id = args.worker_id or f"w-{uuid.uuid4().hex[:12]}"
    voice_profiles = [p.strip() for p in args.voice_profiles.split(",") if p.strip()]
    catalog = VoiceProfileCatalog.load_default()
    api_keys = build_api_keys_from_env()

    worker = Worker(
        orchestrator_url=args.orchestrator_url,
        shared_secret=args.shared_secret,
        worker_id=worker_id,
        pool=args.pool,
        voice_profiles=voice_profiles,
        max_concurrent=args.max_concurrent,
        catalog=catalog,
        api_keys=api_keys,
    )
    asyncio.run(worker.run())


__all__ = ["Worker", "build_api_keys_from_env", "main"]


if __name__ == "__main__":  # pragma: no cover
    main()
