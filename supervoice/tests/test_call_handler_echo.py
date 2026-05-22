"""Tests for the per-call echo handler in supervoice.session.handler."""

from __future__ import annotations

from unittest.mock import AsyncMock, MagicMock

import pytest
from pydantic import SecretStr

from supervoice.session.handler import run_echo_call
from supervoice.shared.speech.stt_factory import STTProviderConfig
from supervoice.shared.speech.tts_factory import TTSProviderConfig


@pytest.mark.asyncio
async def test_run_echo_call_constructs_pipeline_and_runs() -> None:
    """Handler should build pipeline and await runner.run()."""
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()
    runner_factory = MagicMock(return_value=fake_runner)

    await run_echo_call(
        session_id="abc",
        transport=fake_transport,
        stt=STTProviderConfig(
            provider="deepgram",
            api_key=SecretStr("x"),
            language="en",
        ),
        tts=TTSProviderConfig(
            provider="cartesia",
            api_key=SecretStr("x"),
            voice_id="v",
        ),
        runner_factory=runner_factory,
    )

    fake_runner.run.assert_awaited()


@pytest.mark.asyncio
async def test_run_echo_call_ends_state_on_runner_exception() -> None:
    """SessionState must be ended even if the runner raises."""
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock(side_effect=RuntimeError("boom"))
    runner_factory = MagicMock(return_value=fake_runner)

    with pytest.raises(RuntimeError):
        await run_echo_call(
            session_id="abc",
            transport=fake_transport,
            stt=STTProviderConfig(
                provider="deepgram",
                api_key=SecretStr("x"),
                language="en",
            ),
            tts=TTSProviderConfig(
                provider="cartesia",
                api_key=SecretStr("x"),
                voice_id="v",
            ),
            runner_factory=runner_factory,
        )

    fake_runner.run.assert_awaited()
