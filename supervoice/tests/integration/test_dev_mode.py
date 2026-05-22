"""Integration tests for dev mode: --dev-mode + --single-process.

Covers:
- POST /v1/dev/inject-audio with a synthetic WAV (200 + participant)
- POST /v1/dev/inject-audio on a nonexistent session (404)
- Dev endpoint not available when dev_mode=False (404)
- create_single_process_app() wires worker into the app
"""

from __future__ import annotations

import io
import struct

import pytest
from httpx import ASGITransport, AsyncClient

from supervoice.orchestrator.api.auth import AuthConfig, TenantSecret
from supervoice.orchestrator.api.dependencies import AgentConfig
from supervoice.orchestrator.main import create_app, create_single_process_app
from supervoice.orchestrator.room.engine import RoomOpts
from supervoice.orchestrator.room.in_process_engine import InProcessRoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session
from supervoice.orchestrator.worker_registry import (
    WorkerDispatcher,
    WorkerRegistry,
)


def _dev_auth_config() -> AuthConfig:
    """Auth config with a dev-mode tenant secret."""
    return AuthConfig(
        secrets=[
            TenantSecret(
                tenant_id="dev-mode",
                secret="dev-secret",
                admin=True,
            )
        ]
    )


class _StubMappingCache:
    """Minimal in-memory mapping cache for tests."""

    def __init__(self) -> None:
        self._data: dict[tuple[str, str], AgentConfig] = {}

    async def get(
        self, *, tenant_id: str, to_number: str
    ) -> AgentConfig | None:
        return self._data.get((tenant_id, to_number))

    async def upsert(
        self, *, tenant_id: str, to_number: str, config: AgentConfig
    ) -> None:
        self._data[(tenant_id, to_number)] = config


def make_tiny_wav(
    duration_s: float = 0.1, sample_rate: int = 16000
) -> bytes:
    """Generate a minimal WAV file with silence."""
    num_samples = int(sample_rate * duration_s)
    audio_data = b"\x00\x00" * num_samples  # 16-bit silence
    buf = io.BytesIO()
    data_size = len(audio_data)
    buf.write(b"RIFF")
    buf.write(struct.pack("<I", 36 + data_size))
    buf.write(b"WAVE")
    buf.write(b"fmt ")
    buf.write(struct.pack("<I", 16))  # chunk size
    buf.write(struct.pack("<H", 1))  # PCM
    buf.write(struct.pack("<H", 1))  # mono
    buf.write(struct.pack("<I", sample_rate))
    buf.write(struct.pack("<I", sample_rate * 2))  # byte rate
    buf.write(struct.pack("<H", 2))  # block align
    buf.write(struct.pack("<H", 16))  # bits per sample
    buf.write(b"data")
    buf.write(struct.pack("<I", data_size))
    buf.write(audio_data)
    return buf.getvalue()


def _build_dev_app(*, dev_mode: bool = True) -> tuple:
    """Build an app with dev mode and an InProcessRoomEngine.

    Returns (app, engine, session_registry).
    """
    engine = InProcessRoomEngine()
    session_registry = SessionRegistry()
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=2.0)

    app = create_app(
        auth_config=_dev_auth_config(),
        room_engine=engine,
        mapping_cache=_StubMappingCache(),  # type: ignore[arg-type]
        worker_dispatcher=dispatcher,
        session_registry=session_registry,
        dev_mode=dev_mode,
    )
    return app, engine, session_registry


@pytest.mark.asyncio
async def test_dev_mode_inject_audio_creates_participant() -> None:
    """POST /v1/dev/inject-audio creates a synthetic participant."""
    app, engine, session_registry = _build_dev_app(dev_mode=True)

    # Create a session with a room.
    session = Session(
        session_id="s-test-inject",
        tenant_id="dev-mode",
        metadata={"test": True},
    )
    await session_registry.register(session)

    room_handle = await engine.create_room(
        RoomOpts(
            session_id="s-test-inject",
            metadata={},
            max_participants=4,
        )
    )
    session.room_handle = room_handle

    wav_data = make_tiny_wav()

    transport = ASGITransport(app=app)
    async with AsyncClient(
        transport=transport, base_url="http://test"
    ) as client:
        resp = await client.post(
            "/v1/dev/inject-audio",
            data={
                "session_id": "s-test-inject",
                "play_as": "user_speaking",
                "loop": "false",
            },
            files={"file": ("test.wav", wav_data, "audio/wav")},
        )

    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "injected"
    assert body["session_id"] == "s-test-inject"
    assert body["participant_id"]
    assert body["audio_size_bytes"] == len(wav_data)
    assert body["play_as"] == "user_speaking"
    assert body["loop"] is False


@pytest.mark.asyncio
async def test_dev_mode_inject_audio_session_not_found() -> None:
    """POST /v1/dev/inject-audio with nonexistent session returns 404."""
    app, _engine, _registry = _build_dev_app(dev_mode=True)

    wav_data = make_tiny_wav()

    transport = ASGITransport(app=app)
    async with AsyncClient(
        transport=transport, base_url="http://test"
    ) as client:
        resp = await client.post(
            "/v1/dev/inject-audio",
            data={
                "session_id": "s-nonexistent",
                "play_as": "user_speaking",
                "loop": "false",
            },
            files={"file": ("test.wav", wav_data, "audio/wav")},
        )

    assert resp.status_code == 404
    assert resp.json()["detail"] == "session not found"


@pytest.mark.asyncio
async def test_dev_mode_endpoint_not_available_without_flag() -> None:
    """Dev endpoints return 404 when dev_mode is False."""
    app, _engine, _registry = _build_dev_app(dev_mode=False)

    wav_data = make_tiny_wav()

    transport = ASGITransport(app=app)
    async with AsyncClient(
        transport=transport, base_url="http://test"
    ) as client:
        resp = await client.post(
            "/v1/dev/inject-audio",
            data={
                "session_id": "s-test",
                "play_as": "user_speaking",
                "loop": "false",
            },
            files={"file": ("test.wav", wav_data, "audio/wav")},
        )

    # Router not mounted => 404 (Method Not Allowed would also be
    # acceptable, but FastAPI returns 404 for unmatched routes).
    assert resp.status_code in {404, 405}


@pytest.mark.asyncio
async def test_single_process_mode_creates_worker() -> None:
    """create_single_process_app() wires a worker into the app."""
    app = create_single_process_app()

    # Verify app.state has the expected single-process attributes.
    assert getattr(app.state, "single_process", False) is True
    assert app.state.worker_registration is not None
    assert app.state.job_runner is not None
    assert app.state.worker_registry is not None
    assert app.state.worker_dispatch_server is not None

    # Dev mode should be enabled by default in single-process.
    assert getattr(app.state, "dev_mode", False) is True

    # Health endpoint should still work.
    transport = ASGITransport(app=app)
    async with AsyncClient(
        transport=transport, base_url="http://test"
    ) as client:
        resp = await client.get("/health")

    assert resp.status_code == 200
    assert resp.json() == {"status": "ok"}


@pytest.mark.asyncio
async def test_dev_mode_inject_audio_no_room() -> None:
    """POST /v1/dev/inject-audio on a session without a room returns 409."""
    app, _engine, session_registry = _build_dev_app(dev_mode=True)

    # Create a session WITHOUT a room.
    session = Session(
        session_id="s-no-room",
        tenant_id="dev-mode",
        metadata={},
    )
    await session_registry.register(session)

    wav_data = make_tiny_wav()

    transport = ASGITransport(app=app)
    async with AsyncClient(
        transport=transport, base_url="http://test"
    ) as client:
        resp = await client.post(
            "/v1/dev/inject-audio",
            data={
                "session_id": "s-no-room",
                "play_as": "user_speaking",
                "loop": "false",
            },
            files={"file": ("test.wav", wav_data, "audio/wav")},
        )

    assert resp.status_code == 409
    assert resp.json()["detail"] == "session has no room"
