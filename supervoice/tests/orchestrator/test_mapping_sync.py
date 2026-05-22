"""Tests for number-mapping initial_sync against mock unpod."""

from __future__ import annotations

from typing import Any
from unittest.mock import patch

import httpx
import pytest

from supervoice.orchestrator.mapping.cache import NumberMappingCache
from supervoice.orchestrator.mapping.sync import initial_sync


def _make_config(
    tenant: str = "t1",
    number: str = "+1555000",
    profile: str = "en-female",
) -> dict[str, Any]:
    return {
        "tenant_id": tenant,
        "to_number": number,
        "voice_profile_id": profile,
        "runner_url": "https://runner.example.com",
        "agent_secret": "secret-abc",
        "metadata": {"label": "test"},
    }


@pytest.mark.asyncio
async def test_initial_sync_populates_cache() -> None:
    """Two configs from unpod should populate the cache."""
    configs = [
        _make_config("t1", "+1555001"),
        _make_config("t2", "+1555002", "en-male"),
    ]

    async def _mock_get(
        self: Any, url: str, **kwargs: Any
    ) -> httpx.Response:
        return httpx.Response(
            200,
            json={"agents": configs},
            request=httpx.Request("GET", url),
        )

    cache = NumberMappingCache(ttl_s=300.0)

    with patch.object(httpx.AsyncClient, "get", _mock_get):
        count = await initial_sync(
            cache,
            unpod_url="http://mock-unpod",
            shared_secret="test-secret",
        )

    assert count == 2
    cfg1 = await cache.get(tenant_id="t1", to_number="+1555001")
    assert cfg1 is not None
    assert cfg1.voice_profile_id == "en-female"
    assert cfg1.runner_url == "https://runner.example.com"
    assert cfg1.metadata == {"label": "test"}

    cfg2 = await cache.get(tenant_id="t2", to_number="+1555002")
    assert cfg2 is not None
    assert cfg2.voice_profile_id == "en-male"


@pytest.mark.asyncio
async def test_initial_sync_empty_response() -> None:
    """Empty agents list should return 0 and leave cache empty."""

    async def _mock_get(
        self: Any, url: str, **kwargs: Any
    ) -> httpx.Response:
        return httpx.Response(
            200,
            json={"agents": []},
            request=httpx.Request("GET", url),
        )

    cache = NumberMappingCache(ttl_s=300.0)

    with patch.object(httpx.AsyncClient, "get", _mock_get):
        count = await initial_sync(
            cache,
            unpod_url="http://mock-unpod",
            shared_secret="test-secret",
        )

    assert count == 0
    assert await cache.size() == 0


@pytest.mark.asyncio
async def test_initial_sync_connection_error_raises() -> None:
    """Pointing at a dead URL should raise an httpx error."""
    cache = NumberMappingCache(ttl_s=300.0)

    with pytest.raises((httpx.ConnectError, httpx.HTTPError)):
        await initial_sync(
            cache,
            unpod_url="http://localhost:1",
            shared_secret="test-secret",
        )
