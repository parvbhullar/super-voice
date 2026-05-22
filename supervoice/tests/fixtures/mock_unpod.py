"""Mock unpod control plane for number-mapping sync tests."""

from __future__ import annotations

from typing import Any

from fastapi import FastAPI


def create_mock_unpod(configs: list[dict[str, Any]]) -> FastAPI:
    """Build a tiny FastAPI app that serves GET /v1/agents/sync.

    Parameters
    ----------
    configs:
        List of dicts with keys: ``tenant_id``, ``to_number``,
        ``voice_profile_id``, ``runner_url``, ``agent_secret``,
        and optionally ``metadata``.
    """
    app = FastAPI()

    @app.get("/v1/agents/sync")
    async def sync() -> dict[str, Any]:
        return {"agents": configs}

    return app
