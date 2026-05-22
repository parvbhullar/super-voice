"""Unpod control-plane sync (initial pull + webhook handler)."""

from __future__ import annotations

from typing import Any

import httpx
from loguru import logger

from .cache import AgentConfig, NumberMappingCache


async def initial_sync(
    cache: NumberMappingCache,
    *,
    unpod_url: str,
    shared_secret: str,
) -> int:
    """Pull all agent configs from unpod on startup.

    Makes a GET to ``{unpod_url}/v1/agents/sync`` with Bearer auth,
    parses the response, and upserts each entry into the cache.

    Returns the number of entries synced.
    """
    async with httpx.AsyncClient() as client:
        r = await client.get(
            f"{unpod_url}/v1/agents/sync",
            headers={"Authorization": f"Bearer {shared_secret}"},
        )
        r.raise_for_status()
        data = r.json()

    count = 0
    for entry in data.get("agents", []):
        config = AgentConfig(
            voice_profile_id=entry["voice_profile_id"],
            runner_url=entry["runner_url"],
            agent_secret=entry["agent_secret"],
            metadata=entry.get("metadata", {}),
        )
        await cache.upsert(
            tenant_id=entry["tenant_id"],
            to_number=entry["to_number"],
            config=config,
        )
        count += 1

    logger.info("initial_sync: loaded {} agent configs", count)
    return count


async def handle_webhook(
    cache: NumberMappingCache,
    payload: dict[str, Any],
) -> None:
    """Process an upsert/delete webhook from unpod.

    Expected payload shape::

        {"action": "upsert" | "delete",
         "tenant_id": str, "to_number": str,
         "config"?: {"voice_profile_id", "runner_url",
                     "agent_secret", "metadata"?}}

    V1: parses + applies. No HMAC verification (deferred to Phase 6).
    """
    action = payload.get("action")
    tenant_id = payload.get("tenant_id")
    to_number = payload.get("to_number")
    if not isinstance(tenant_id, str) or not isinstance(to_number, str):
        raise ValueError("missing tenant_id or to_number")
    if action == "upsert":
        cfg = payload.get("config") or {}
        config = AgentConfig(
            voice_profile_id=cfg["voice_profile_id"],
            runner_url=cfg["runner_url"],
            agent_secret=cfg["agent_secret"],
            metadata=cfg.get("metadata", {}),
        )
        await cache.upsert(
            tenant_id=tenant_id, to_number=to_number, config=config
        )
    elif action == "delete":
        await cache.remove(tenant_id=tenant_id, to_number=to_number)
    else:
        raise ValueError(f"unknown action: {action!r}")
