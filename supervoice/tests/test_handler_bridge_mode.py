"""Tests for the bridge-mode call handler."""

from unittest.mock import AsyncMock, MagicMock

import pytest
from pydantic import SecretStr

from supervoice.bridge.client import AgentBridgeClient
from supervoice.session.handler import run_bridge_call
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


@pytest.mark.asyncio
async def test_run_bridge_call_connects_and_runs(mock_bridge):
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()
    runner_factory = MagicMock(return_value=fake_runner)

    await run_bridge_call(
        session_id="abc",
        transport=fake_transport,
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        agent_bridge_url=mock_bridge,
        runner_factory=runner_factory,
    )
    fake_runner.run.assert_awaited()


@pytest.mark.asyncio
async def test_run_bridge_call_closes_client_on_runner_exception(
    mock_bridge, monkeypatch
):
    """If runner.run raises, the bridge client must still be closed."""
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock(side_effect=RuntimeError("boom"))
    runner_factory = MagicMock(return_value=fake_runner)

    close_called: list[bool] = []
    original_close = AgentBridgeClient.close

    async def tracking_close(self):
        close_called.append(True)
        await original_close(self)

    monkeypatch.setattr(AgentBridgeClient, "close", tracking_close)

    with pytest.raises(RuntimeError, match="boom"):
        await run_bridge_call(
            session_id="abc",
            transport=fake_transport,
            stt=STTProviderConfig(
                provider="deepgram", api_key=SecretStr("x"), language="en"
            ),
            tts=TTSProviderConfig(
                provider="cartesia", api_key=SecretStr("x"), voice_id="v"
            ),
            agent_bridge_url=mock_bridge,
            runner_factory=runner_factory,
        )

    assert close_called == [True], (
        "client.close() must be called even on runner exception"
    )
