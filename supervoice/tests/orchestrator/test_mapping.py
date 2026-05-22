"""Tests for the number-mapping cache and webhook handler."""

from __future__ import annotations

import asyncio

import pytest

from supervoice.orchestrator.mapping import (
    AgentConfig,
    NumberMappingCache,
    handle_webhook,
    initial_sync,
)


def _make_config(profile: str = "vp-1") -> AgentConfig:
    return AgentConfig(
        voice_profile_id=profile,
        runner_url="http://runner.local",
        agent_secret="shh",
        metadata={"k": "v"},
    )


async def test_get_returns_none_for_missing():
    cache = NumberMappingCache()
    assert (
        await cache.get(tenant_id="t1", to_number="+1")
    ) is None


async def test_upsert_then_get_roundtrip():
    cache = NumberMappingCache()
    cfg = _make_config()
    await cache.upsert(tenant_id="t1", to_number="+1", config=cfg)
    fetched = await cache.get(tenant_id="t1", to_number="+1")
    assert fetched == cfg


async def test_remove_clears_entry():
    cache = NumberMappingCache()
    await cache.upsert(
        tenant_id="t1", to_number="+1", config=_make_config()
    )
    await cache.remove(tenant_id="t1", to_number="+1")
    assert (
        await cache.get(tenant_id="t1", to_number="+1")
    ) is None


async def test_ttl_expiry_returns_none():
    cache = NumberMappingCache(ttl_s=0.05)
    await cache.upsert(
        tenant_id="t1", to_number="+1", config=_make_config()
    )
    await asyncio.sleep(0.1)
    assert (
        await cache.get(tenant_id="t1", to_number="+1")
    ) is None


async def test_size_tracks_entries():
    cache = NumberMappingCache()
    assert await cache.size() == 0
    await cache.upsert(
        tenant_id="t1", to_number="+1", config=_make_config()
    )
    await cache.upsert(
        tenant_id="t1", to_number="+2", config=_make_config()
    )
    assert await cache.size() == 2
    await cache.remove(tenant_id="t1", to_number="+1")
    assert await cache.size() == 1


async def test_handle_webhook_upsert_action():
    cache = NumberMappingCache()
    await handle_webhook(
        cache,
        {
            "action": "upsert",
            "tenant_id": "t1",
            "to_number": "+1",
            "config": {
                "voice_profile_id": "vp-1",
                "runner_url": "http://r",
                "agent_secret": "s",
                "metadata": {"x": 1},
            },
        },
    )
    cfg = await cache.get(tenant_id="t1", to_number="+1")
    assert cfg is not None
    assert cfg.voice_profile_id == "vp-1"
    assert cfg.metadata == {"x": 1}


async def test_handle_webhook_delete_action():
    cache = NumberMappingCache()
    await cache.upsert(
        tenant_id="t1", to_number="+1", config=_make_config()
    )
    await handle_webhook(
        cache,
        {
            "action": "delete",
            "tenant_id": "t1",
            "to_number": "+1",
        },
    )
    assert (
        await cache.get(tenant_id="t1", to_number="+1")
    ) is None


async def test_handle_webhook_unknown_action_raises():
    cache = NumberMappingCache()
    with pytest.raises(ValueError, match="unknown action"):
        await handle_webhook(
            cache,
            {
                "action": "nope",
                "tenant_id": "t1",
                "to_number": "+1",
            },
        )


async def test_handle_webhook_missing_fields_raises():
    cache = NumberMappingCache()
    with pytest.raises(ValueError, match="missing tenant_id"):
        await handle_webhook(
            cache,
            {"action": "upsert", "to_number": "+1"},
        )


async def test_initial_sync_raises_on_unreachable_url():
    """Sync is no longer a no-op (un-stubbed in Task 35); it now makes a real
    HTTP call. Verify it raises on a dead URL rather than silently succeeding."""
    import httpx

    cache = NumberMappingCache()
    with pytest.raises(httpx.ConnectError):
        await initial_sync(
            cache, unpod_url="http://unreachable-host-that-does-not-exist", shared_secret="x"
        )
    assert await cache.size() == 0
