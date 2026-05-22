"""Tests for the orchestrator auth middleware (Task 14)."""

from __future__ import annotations

import pytest
from fastapi import Depends, FastAPI
from fastapi.testclient import TestClient

from supervoice.orchestrator.api.auth import (
    AuthConfig,
    AuthContext,
    TenantSecret,
    get_auth_context,
    require_admin,
)


def test_authconfig_from_env_parses_entries() -> None:
    cfg = AuthConfig.from_env(
        "tenant-a:secret-a,tenant-b:secret-b:admin,tenant-a:secret-a2"
    )
    a = cfg.lookup_api_secret("secret-a")
    b = cfg.lookup_api_secret("secret-b")
    a2 = cfg.lookup_api_secret("secret-a2")
    assert a == TenantSecret(tenant_id="tenant-a", secret="secret-a", admin=False)
    assert b == TenantSecret(tenant_id="tenant-b", secret="secret-b", admin=True)
    assert a2 == TenantSecret(
        tenant_id="tenant-a", secret="secret-a2", admin=False
    )


def test_authconfig_from_env_handles_empty() -> None:
    assert AuthConfig.from_env(None).lookup_api_secret("x") is None
    assert AuthConfig.from_env("").lookup_api_secret("x") is None
    # Malformed entries are skipped silently.
    cfg = AuthConfig.from_env("nopair, ,only-tenant:,:only-secret,ok:val")
    assert cfg.lookup_api_secret("val") == TenantSecret(
        tenant_id="ok", secret="val", admin=False
    )


def test_authconfig_lookup_returns_tenant() -> None:
    cfg = AuthConfig.from_env("tenant-a:secret-a")
    ts = cfg.lookup_api_secret("secret-a")
    assert ts is not None
    assert ts.tenant_id == "tenant-a"
    assert ts.admin is False


def test_authconfig_lookup_unknown_returns_none() -> None:
    cfg = AuthConfig.from_env("tenant-a:secret-a")
    assert cfg.lookup_api_secret("nope") is None


@pytest.fixture
def app_with_auth() -> FastAPI:
    app = FastAPI()
    app.state.auth_config = AuthConfig.from_env(
        "tenant-a:secret-a,tenant-b:secret-b:admin"
    )

    @app.get("/whoami")
    async def whoami(
        auth: AuthContext = Depends(get_auth_context),
    ) -> dict[str, object]:
        return {"tenant_id": auth.tenant_id, "admin": auth.admin}

    @app.get("/admin-only")
    async def admin_only(
        auth: AuthContext = Depends(require_admin),
    ) -> dict[str, object]:
        return {"ok": True, "tenant_id": auth.tenant_id}

    return app


def test_get_auth_context_api_secret_match(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/whoami", headers={"Authorization": "Bearer secret-a"}
        )
        assert r.status_code == 200
        assert r.json() == {"tenant_id": "tenant-a", "admin": False}


def test_get_auth_context_admin_flag(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/whoami", headers={"Authorization": "Bearer secret-b"}
        )
        assert r.status_code == 200
        assert r.json() == {"tenant_id": "tenant-b", "admin": True}


def test_get_auth_context_missing_header_401(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get("/whoami")
        assert r.status_code == 401
        assert r.json()["detail"] == "missing authorization"


def test_get_auth_context_unknown_token_401(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/whoami", headers={"Authorization": "Bearer not-a-real-secret"}
        )
        assert r.status_code == 401
        assert r.json()["detail"] == "invalid token"


def test_get_auth_context_jwt_stub_header(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/whoami",
            headers={
                "Authorization": "Bearer some-jwt-looking-blob",
                "X-Stub-JWT-Tenant": "tenant-xyz",
            },
        )
        assert r.status_code == 200
        assert r.json() == {"tenant_id": "tenant-xyz", "admin": False}


def test_query_param_fallback(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get("/whoami?api_key=secret-a")
        assert r.status_code == 200
        assert r.json() == {"tenant_id": "tenant-a", "admin": False}


def test_require_admin_blocks_non_admin_403(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/admin-only", headers={"Authorization": "Bearer secret-a"}
        )
        assert r.status_code == 403
        assert r.json()["detail"] == "admin scope required"


def test_require_admin_allows_admin_200(app_with_auth: FastAPI) -> None:
    with TestClient(app_with_auth) as client:
        r = client.get(
            "/admin-only", headers={"Authorization": "Bearer secret-b"}
        )
        assert r.status_code == 200
        assert r.json() == {"ok": True, "tenant_id": "tenant-b"}
