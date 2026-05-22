"""Stubs for unpod control-plane sync (initial pull + webhook handler)."""

from __future__ import annotations

from typing import Any

from loguru import logger

from .cache import AgentConfig, NumberMappingCache


async def initial_sync(
    cache: NumberMappingCache,
    *,
    unpod_url: str,
    shared_secret: str,
) -> int:
    """Pull all agent configs from unpod on startup.

    V1: NO-OP -- unpod control-plane endpoint is not built yet.
    Returns 0 (entries synced). Production wires real HTTP fetch + iteration.
    """
    logger.info(
        "initial_sync stub -- unpod control plane integration deferred"
    )
    return 0


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
