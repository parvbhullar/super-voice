"""Auth middleware for the orchestrator REST API.

Provides FastAPI dependencies that resolve an ``AuthContext`` from the
incoming request. Two auth mechanisms are supported:

1. API secrets — exact match against a configured list of per-tenant
   secrets, loaded from ``SUPERVOICE_API_SECRETS``.
2. JWT (V1 stub) — accepts a ``X-Stub-JWT-Tenant`` header for testing
   convenience. Production wires unpod JWKS validation later.
"""

from __future__ import annotations

from dataclasses import dataclass

from fastapi import Depends, HTTPException, Request, status
from loguru import logger


@dataclass(frozen=True)
class AuthContext:
    """Resolved auth context attached to a request."""

    tenant_id: str
    admin: bool = False


@dataclass(frozen=True)
class TenantSecret:
    """A configured API secret bound to a tenant.

    Loaded from environment / config; for V1 keep it simple — a list of
    these. unpod-issued JWTs come later.
    """

    tenant_id: str
    secret: str
    admin: bool = False


class AuthConfig:
    """Container for the loaded auth config.

    V1 source: environment variable ``SUPERVOICE_API_SECRETS`` formatted
    as ``tenant_id:secret[:admin],tenant_id:secret[:admin],...``.

    Example: ``SUPERVOICE_API_SECRETS=t1:abc,t2:xyz:admin,t1:def``.
    """

    def __init__(self, secrets: list[TenantSecret]) -> None:
        self._by_secret: dict[str, TenantSecret] = {
            ts.secret: ts for ts in secrets
        }

    @classmethod
    def from_env(cls, env_value: str | None) -> "AuthConfig":
        """Parse the env-var formatted string into a config object."""
        secrets: list[TenantSecret] = []
        if env_value:
            for raw in env_value.split(","):
                entry = raw.strip()
                if not entry:
                    continue
                parts = entry.split(":")
                if len(parts) < 2:
                    continue
                tenant_id, secret = parts[0], parts[1]
                if not tenant_id or not secret:
                    continue
                admin = len(parts) > 2 and parts[2] == "admin"
                secrets.append(
                    TenantSecret(
                        tenant_id=tenant_id, secret=secret, admin=admin
                    )
                )
        return cls(secrets)

    def lookup_api_secret(self, presented: str) -> TenantSecret | None:
        """Return the matching ``TenantSecret`` or ``None``."""
        return self._by_secret.get(presented)


def _extract_token(request: Request) -> str | None:
    """Pull bearer token from ``Authorization`` header or ``?api_key=``."""
    header = request.headers.get("authorization", "")
    if header.lower().startswith("bearer "):
        token = header[7:].strip()
        if token:
            return token
    qp = request.query_params.get("api_key")
    return qp or None


async def get_auth_context(request: Request) -> AuthContext:
    """FastAPI dependency. Resolves the auth context from request.

    Auth precedence:
      1. API secret (exact match against configured tenant secrets)
      2. JWT (stub: V1 accepts a header ``X-Stub-JWT-Tenant`` for
         testing convenience until unpod JWKS integration lands).
    """
    config: AuthConfig = request.app.state.auth_config
    token = _extract_token(request)
    if token is None:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="missing authorization",
        )

    ts = config.lookup_api_secret(token)
    if ts is not None:
        return AuthContext(tenant_id=ts.tenant_id, admin=ts.admin)

    # JWT stub — V1. Production: validate against unpod's JWKS and
    # extract the ``tenant_id`` claim.
    stub_tenant = request.headers.get("x-stub-jwt-tenant")
    if stub_tenant:
        return AuthContext(tenant_id=stub_tenant, admin=False)

    logger.warning(
        "auth rejected: token not in API-secret list and no JWT stub"
    )
    raise HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="invalid token",
    )


def require_admin(
    auth: AuthContext = Depends(get_auth_context),
) -> AuthContext:
    """FastAPI dependency for admin-only endpoints."""
    if not auth.admin:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="admin scope required",
        )
    return auth
