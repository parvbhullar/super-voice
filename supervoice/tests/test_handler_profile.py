"""Tests for the voice-profile-driven call handler."""

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
