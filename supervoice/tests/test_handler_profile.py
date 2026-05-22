"""Tests for the voice-profile-driven call handler."""

import asyncio
from unittest.mock import AsyncMock, MagicMock

import pytest
from pydantic import SecretStr

from supervoice.session.handler import run_call_with_profile


@pytest.mark.asyncio
async def test_run_call_with_profile_resolves_providers(mock_bridge):
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    fake_runner = MagicMock()
    fake_runner.run = AsyncMock()

    await run_call_with_profile(
        session_id="abc",
        transport=fake_transport,
        profile_id="en-female",
        api_keys={
            "deepgram": SecretStr("dg"),
            "cartesia": SecretStr("ct"),
        },
        agent_bridge_url=mock_bridge,
        runner_factory=MagicMock(return_value=fake_runner),
    )
    fake_runner.run.assert_awaited()


@pytest.mark.asyncio
async def test_run_call_with_profile_unknown_profile_raises(mock_bridge):
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    with pytest.raises(KeyError):
        await run_call_with_profile(
            session_id="abc",
            transport=fake_transport,
            profile_id="nonexistent",
            api_keys={
                "deepgram": SecretStr("dg"),
                "cartesia": SecretStr("ct"),
            },
            agent_bridge_url=mock_bridge,
            runner_factory=MagicMock(),
        )


@pytest.mark.asyncio
async def test_run_call_with_profile_terminates_on_idle_disconnect(mock_bridge):
    """Idle disconnect must actually cancel the runner."""
    fake_transport = MagicMock()
    fake_transport.input = MagicMock(return_value=MagicMock())
    fake_transport.output = MagicMock(return_value=MagicMock())

    # Runner that hangs forever — only cancellation will return.
    fake_runner = MagicMock()

    async def hang(_task):
        await asyncio.sleep(60)

    fake_runner.run = AsyncMock(side_effect=hang)

    await asyncio.wait_for(
        run_call_with_profile(
            session_id="abc",
            transport=fake_transport,
            profile_id="en-female",
            api_keys={
                "deepgram": SecretStr("dg"),
                "cartesia": SecretStr("ct"),
            },
            agent_bridge_url=mock_bridge,
            runner_factory=MagicMock(return_value=fake_runner),
            idle_warning_at_s=0.1,
            idle_disconnect_at_s=0.2,
        ),
        timeout=2.0,
    )
    # If the runner.run() task wasn't cancelled by the idle monitor, this
    # test would hit the 2.0s wait_for timeout and fail.
