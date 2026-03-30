from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any

import aiohttp


@dataclass
class ApiResponse:
    """Result of a single API call with timing."""

    method: str
    path: str
    status: int
    latency_ms: float
    body: Any = None
    error: str | None = None

    @property
    def success(self) -> bool:
        """True when HTTP status is 2xx."""
        return 200 <= self.status < 300


# -- Sample payloads for CRUD checks ----------------------------------

_SAMPLE_DATA: dict[str, dict[str, Any]] = {
    "endpoints": {
        "name": "test-ep",
        "context": "default",
        "codecs": ["PCMU", "PCMA"],
    },
    "gateways": {
        "name": "test-gw",
        "host": "10.0.0.1",
        "port": 5060,
        "transport": "udp",
    },
    "trunks": {
        "name": "test-trunk",
        "inbound_uri": "sip:test@example.com",
        "send_register": False,
    },
    "dids": {
        "number": "+15550001234",
        "city": "Test City",
        "country": "US",
    },
    "routing_tables": {
        "name": "test-rt",
        "description": "Test routing table",
    },
    "translations": {
        "name": "test-xlat",
        "match": "^\\+1(\\d{10})$",
        "replacement": "\\1",
    },
    "manipulations": {
        "name": "test-manip",
        "type": "header",
        "action": "add",
        "header": "X-Test",
        "value": "true",
    },
    "webhooks": {
        "name": "test-webhook",
        "url": "https://example.com/hook",
        "events": ["call.started"],
    },
}

# Maps group name -> API base path
_GROUP_PATHS: dict[str, str] = {
    "endpoints": "/api/v1/endpoints",
    "gateways": "/api/v1/gateways",
    "trunks": "/api/v1/trunks",
    "dids": "/api/v1/dids",
    "routing_tables": "/api/v1/routing/tables",
    "translations": "/api/v1/translations",
    "manipulations": "/api/v1/manipulations",
    "webhooks": "/api/v1/webhooks",
}


class RestClient:
    """Async REST client for Active Call carrier API."""

    def __init__(
        self,
        base_url: str,
        api_key: str = "",
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._session: aiohttp.ClientSession | None = None
        self._results: list[ApiResponse] = []

    async def connect(self) -> None:
        """Open an aiohttp session with auth headers."""
        headers: dict[str, str] = {
            "Content-Type": "application/json",
        }
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        self._session = aiohttp.ClientSession(
            base_url=self._base_url,
            headers=headers,
        )

    async def disconnect(self) -> None:
        """Close the underlying HTTP session."""
        if self._session:
            await self._session.close()
            self._session = None

    # -- Core request helper -------------------------------------------

    async def _request(
        self,
        method: str,
        path: str,
        json: dict[str, Any] | None = None,
        params: dict[str, str] | None = None,
    ) -> ApiResponse:
        """Make an HTTP request, track latency, return result."""
        if self._session is None:
            return ApiResponse(
                method=method,
                path=path,
                status=0,
                latency_ms=0.0,
                error="Session not connected",
            )
        start = time.monotonic()
        try:
            async with self._session.request(
                method,
                path,
                json=json,
                params=params,
            ) as resp:
                latency = (time.monotonic() - start) * 1000
                try:
                    body = await resp.json()
                except Exception:
                    body = await resp.text()
                error = None if 200 <= resp.status < 300 else str(body)
                result = ApiResponse(
                    method=method,
                    path=path,
                    status=resp.status,
                    latency_ms=latency,
                    body=body,
                    error=error,
                )
        except Exception as exc:
            latency = (time.monotonic() - start) * 1000
            result = ApiResponse(
                method=method,
                path=path,
                status=0,
                latency_ms=latency,
                error=str(exc),
            )
        self._results.append(result)
        return result

    # -- Results -------------------------------------------------------

    @property
    def results(self) -> list[ApiResponse]:
        """Return a copy of all collected results."""
        return list(self._results)

    def clear_results(self) -> None:
        """Reset collected results."""
        self._results.clear()

    # ==================================================================
    # CRUD Groups
    # ==================================================================

    # -- Generic CRUD helpers ------------------------------------------

    async def _create(self, base: str, data: dict[str, Any]) -> ApiResponse:
        return await self._request("POST", base, json=data)

    async def _list(self, base: str) -> ApiResponse:
        return await self._request("GET", base)

    async def _get(self, base: str, id: str) -> ApiResponse:
        return await self._request("GET", f"{base}/{id}")

    async def _update(self, base: str, id: str, data: dict[str, Any]) -> ApiResponse:
        return await self._request("PUT", f"{base}/{id}", json=data)

    async def _delete(self, base: str, id: str) -> ApiResponse:
        return await self._request("DELETE", f"{base}/{id}")

    # -- Endpoints -----------------------------------------------------

    async def create_endpoint(self, data: dict[str, Any]) -> ApiResponse:
        """Create a SIP endpoint."""
        return await self._create("/api/v1/endpoints", data)

    async def list_endpoints(self) -> ApiResponse:
        """List all SIP endpoints."""
        return await self._list("/api/v1/endpoints")

    async def get_endpoint(self, id: str) -> ApiResponse:
        """Get a SIP endpoint by ID."""
        return await self._get("/api/v1/endpoints", id)

    async def update_endpoint(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a SIP endpoint."""
        return await self._update("/api/v1/endpoints", id, data)

    async def delete_endpoint(self, id: str) -> ApiResponse:
        """Delete a SIP endpoint."""
        return await self._delete("/api/v1/endpoints", id)

    # -- Gateways ------------------------------------------------------

    async def create_gateway(self, data: dict[str, Any]) -> ApiResponse:
        """Create a gateway."""
        return await self._create("/api/v1/gateways", data)

    async def list_gateways(self) -> ApiResponse:
        """List all gateways."""
        return await self._list("/api/v1/gateways")

    async def get_gateway(self, id: str) -> ApiResponse:
        """Get a gateway by ID."""
        return await self._get("/api/v1/gateways", id)

    async def update_gateway(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a gateway."""
        return await self._update("/api/v1/gateways", id, data)

    async def delete_gateway(self, id: str) -> ApiResponse:
        """Delete a gateway."""
        return await self._delete("/api/v1/gateways", id)

    # -- Trunks --------------------------------------------------------

    async def create_trunk(self, data: dict[str, Any]) -> ApiResponse:
        """Create a trunk."""
        return await self._create("/api/v1/trunks", data)

    async def list_trunks(self) -> ApiResponse:
        """List all trunks."""
        return await self._list("/api/v1/trunks")

    async def get_trunk(self, id: str) -> ApiResponse:
        """Get a trunk by ID."""
        return await self._get("/api/v1/trunks", id)

    async def update_trunk(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a trunk."""
        return await self._update("/api/v1/trunks", id, data)

    async def delete_trunk(self, id: str) -> ApiResponse:
        """Delete a trunk."""
        return await self._delete("/api/v1/trunks", id)

    # Trunk sub-resources

    async def set_trunk_credentials(
        self, trunk_id: str, data: dict[str, Any]
    ) -> ApiResponse:
        """Set credentials for a trunk."""
        return await self._request(
            "PUT",
            f"/api/v1/trunks/{trunk_id}/credentials",
            json=data,
        )

    async def get_trunk_credentials(self, trunk_id: str) -> ApiResponse:
        """Get credentials for a trunk."""
        return await self._request(
            "GET",
            f"/api/v1/trunks/{trunk_id}/credentials",
        )

    async def set_trunk_acl(self, trunk_id: str, data: dict[str, Any]) -> ApiResponse:
        """Set ACL for a trunk."""
        return await self._request(
            "PUT",
            f"/api/v1/trunks/{trunk_id}/acl",
            json=data,
        )

    async def get_trunk_acl(self, trunk_id: str) -> ApiResponse:
        """Get ACL for a trunk."""
        return await self._request(
            "GET",
            f"/api/v1/trunks/{trunk_id}/acl",
        )

    async def set_trunk_origination_uris(
        self, trunk_id: str, data: dict[str, Any]
    ) -> ApiResponse:
        """Set origination URIs for a trunk."""
        return await self._request(
            "PUT",
            f"/api/v1/trunks/{trunk_id}/origination-uris",
            json=data,
        )

    async def get_trunk_origination_uris(self, trunk_id: str) -> ApiResponse:
        """Get origination URIs for a trunk."""
        return await self._request(
            "GET",
            f"/api/v1/trunks/{trunk_id}/origination-uris",
        )

    async def set_trunk_media(self, trunk_id: str, data: dict[str, Any]) -> ApiResponse:
        """Set media settings for a trunk."""
        return await self._request(
            "PUT",
            f"/api/v1/trunks/{trunk_id}/media",
            json=data,
        )

    async def get_trunk_media(self, trunk_id: str) -> ApiResponse:
        """Get media settings for a trunk."""
        return await self._request(
            "GET",
            f"/api/v1/trunks/{trunk_id}/media",
        )

    async def set_trunk_capacity(
        self, trunk_id: str, data: dict[str, Any]
    ) -> ApiResponse:
        """Set capacity for a trunk."""
        return await self._request(
            "PUT",
            f"/api/v1/trunks/{trunk_id}/capacity",
            json=data,
        )

    async def get_trunk_capacity(self, trunk_id: str) -> ApiResponse:
        """Get capacity for a trunk."""
        return await self._request(
            "GET",
            f"/api/v1/trunks/{trunk_id}/capacity",
        )

    # -- DIDs ----------------------------------------------------------

    async def create_did(self, data: dict[str, Any]) -> ApiResponse:
        """Create a DID."""
        return await self._create("/api/v1/dids", data)

    async def list_dids(self) -> ApiResponse:
        """List all DIDs."""
        return await self._list("/api/v1/dids")

    async def get_did(self, id: str) -> ApiResponse:
        """Get a DID by ID."""
        return await self._get("/api/v1/dids", id)

    async def update_did(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a DID."""
        return await self._update("/api/v1/dids", id, data)

    async def delete_did(self, id: str) -> ApiResponse:
        """Delete a DID."""
        return await self._delete("/api/v1/dids", id)

    # -- Routing Tables ------------------------------------------------

    async def create_routing_table(self, data: dict[str, Any]) -> ApiResponse:
        """Create a routing table."""
        return await self._create("/api/v1/routing/tables", data)

    async def list_routing_tables(self) -> ApiResponse:
        """List all routing tables."""
        return await self._list("/api/v1/routing/tables")

    async def get_routing_table(self, id: str) -> ApiResponse:
        """Get a routing table by ID."""
        return await self._get("/api/v1/routing/tables", id)

    async def update_routing_table(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a routing table."""
        return await self._update("/api/v1/routing/tables", id, data)

    async def delete_routing_table(self, id: str) -> ApiResponse:
        """Delete a routing table."""
        return await self._delete("/api/v1/routing/tables", id)

    # Routing sub-resources

    async def create_routing_record(
        self, table_id: str, data: dict[str, Any]
    ) -> ApiResponse:
        """Add a record to a routing table."""
        return await self._request(
            "POST",
            f"/api/v1/routing/tables/{table_id}/records",
            json=data,
        )

    async def list_routing_records(self, table_id: str) -> ApiResponse:
        """List records in a routing table."""
        return await self._request(
            "GET",
            f"/api/v1/routing/tables/{table_id}/records",
        )

    async def resolve_route(self, data: dict[str, Any]) -> ApiResponse:
        """Evaluate route resolution."""
        return await self._request(
            "POST",
            "/api/v1/routing/tables/resolve",
            json=data,
        )

    # -- Translations --------------------------------------------------

    async def create_translation(self, data: dict[str, Any]) -> ApiResponse:
        """Create a translation rule."""
        return await self._create("/api/v1/translations", data)

    async def list_translations(self) -> ApiResponse:
        """List all translation rules."""
        return await self._list("/api/v1/translations")

    async def get_translation(self, id: str) -> ApiResponse:
        """Get a translation rule by ID."""
        return await self._get("/api/v1/translations", id)

    async def update_translation(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a translation rule."""
        return await self._update("/api/v1/translations", id, data)

    async def delete_translation(self, id: str) -> ApiResponse:
        """Delete a translation rule."""
        return await self._delete("/api/v1/translations", id)

    # -- Manipulations -------------------------------------------------

    async def create_manipulation(self, data: dict[str, Any]) -> ApiResponse:
        """Create a manipulation rule."""
        return await self._create("/api/v1/manipulations", data)

    async def list_manipulations(self) -> ApiResponse:
        """List all manipulation rules."""
        return await self._list("/api/v1/manipulations")

    async def get_manipulation(self, id: str) -> ApiResponse:
        """Get a manipulation rule by ID."""
        return await self._get("/api/v1/manipulations", id)

    async def update_manipulation(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a manipulation rule."""
        return await self._update("/api/v1/manipulations", id, data)

    async def delete_manipulation(self, id: str) -> ApiResponse:
        """Delete a manipulation rule."""
        return await self._delete("/api/v1/manipulations", id)

    # -- Webhooks ------------------------------------------------------

    async def create_webhook(self, data: dict[str, Any]) -> ApiResponse:
        """Create a webhook."""
        return await self._create("/api/v1/webhooks", data)

    async def list_webhooks(self) -> ApiResponse:
        """List all webhooks."""
        return await self._list("/api/v1/webhooks")

    async def get_webhook(self, id: str) -> ApiResponse:
        """Get a webhook by ID."""
        return await self._get("/api/v1/webhooks", id)

    async def update_webhook(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Update a webhook."""
        return await self._update("/api/v1/webhooks", id, data)

    async def delete_webhook(self, id: str) -> ApiResponse:
        """Delete a webhook."""
        return await self._delete("/api/v1/webhooks", id)

    # ==================================================================
    # Active Call Management
    # ==================================================================

    async def list_calls(self) -> ApiResponse:
        """List active calls."""
        return await self._request("GET", "/api/v1/calls")

    async def get_call(self, id: str) -> ApiResponse:
        """Get details of an active call."""
        return await self._request("GET", f"/api/v1/calls/{id}")

    async def hangup_call(self, id: str) -> ApiResponse:
        """Hang up an active call."""
        return await self._request("POST", f"/api/v1/calls/{id}/hangup")

    async def transfer_call(self, id: str, data: dict[str, Any]) -> ApiResponse:
        """Transfer an active call."""
        return await self._request(
            "POST",
            f"/api/v1/calls/{id}/transfer",
            json=data,
        )

    async def mute_call(self, id: str) -> ApiResponse:
        """Mute an active call."""
        return await self._request("POST", f"/api/v1/calls/{id}/mute")

    async def unmute_call(self, id: str) -> ApiResponse:
        """Unmute an active call."""
        return await self._request("POST", f"/api/v1/calls/{id}/unmute")

    # ==================================================================
    # Security
    # ==================================================================

    async def list_firewall_rules(self) -> ApiResponse:
        """List firewall rules."""
        return await self._request("GET", "/api/v1/security/firewall")

    async def add_firewall_rule(self, data: dict[str, Any]) -> ApiResponse:
        """Add a firewall rule."""
        return await self._request("POST", "/api/v1/security/firewall", json=data)

    async def delete_firewall_rule(self, id: str) -> ApiResponse:
        """Delete a firewall rule."""
        return await self._request("DELETE", f"/api/v1/security/firewall/{id}")

    async def list_blocks(self) -> ApiResponse:
        """List blocked IPs."""
        return await self._request("GET", "/api/v1/security/blocks")

    async def unblock_ip(self, ip: str) -> ApiResponse:
        """Unblock an IP address."""
        return await self._request("DELETE", f"/api/v1/security/blocks/{ip}")

    async def get_flood_tracker(self) -> ApiResponse:
        """Get flood tracker state."""
        return await self._request("GET", "/api/v1/security/flood-tracker")

    async def get_auth_failures(self) -> ApiResponse:
        """Get authentication failures."""
        return await self._request("GET", "/api/v1/security/auth-failures")

    # ==================================================================
    # Diagnostics
    # ==================================================================

    async def get_diagnostic_summary(self) -> ApiResponse:
        """Get diagnostic summary."""
        return await self._request("GET", "/api/v1/diagnostics/summary")

    async def test_trunk_connectivity(self, data: dict[str, Any]) -> ApiResponse:
        """Test trunk connectivity."""
        return await self._request(
            "POST",
            "/api/v1/diagnostics/trunk-test",
            json=data,
        )

    async def evaluate_routing(self, data: dict[str, Any]) -> ApiResponse:
        """Evaluate routing for a number."""
        return await self._request(
            "POST",
            "/api/v1/diagnostics/route-eval",
            json=data,
        )

    async def get_registrations(self) -> ApiResponse:
        """Get SIP registrations."""
        return await self._request("GET", "/api/v1/diagnostics/registrations")

    # ==================================================================
    # System
    # ==================================================================

    async def get_health(self) -> ApiResponse:
        """Health check."""
        return await self._request("GET", "/api/v1/system/health")

    async def get_system_info(self) -> ApiResponse:
        """Get system info."""
        return await self._request("GET", "/api/v1/system/info")

    async def get_stats(self) -> ApiResponse:
        """Get system statistics."""
        return await self._request("GET", "/api/v1/system/stats")

    async def get_cluster_status(self) -> ApiResponse:
        """Get cluster status."""
        return await self._request("GET", "/api/v1/system/cluster")

    async def reload_config(self) -> ApiResponse:
        """Reload system configuration."""
        return await self._request("POST", "/api/v1/system/reload")

    async def get_system_config(self) -> ApiResponse:
        """Get system configuration."""
        return await self._request("GET", "/api/v1/system/config")

    # ==================================================================
    # CDRs
    # ==================================================================

    async def list_cdrs(self) -> ApiResponse:
        """List call detail records."""
        return await self._request("GET", "/api/v1/cdrs")

    async def get_cdr(self, id: str) -> ApiResponse:
        """Get a CDR by ID."""
        return await self._request("GET", f"/api/v1/cdrs/{id}")

    async def delete_cdr(self, id: str) -> ApiResponse:
        """Delete a CDR."""
        return await self._request("DELETE", f"/api/v1/cdrs/{id}")

    async def get_cdr_recording(self, id: str) -> ApiResponse:
        """Get recording for a CDR."""
        return await self._request("GET", f"/api/v1/cdrs/{id}/recording")

    async def get_cdr_sip_flow(self, id: str) -> ApiResponse:
        """Get SIP flow for a CDR."""
        return await self._request("GET", f"/api/v1/cdrs/{id}/sip-flow")

    # ==================================================================
    # Playbook API
    # ==================================================================

    async def list_playbooks(self) -> ApiResponse:
        """List playbooks."""
        return await self._request("GET", "/api/playbooks")

    async def get_playbook(self, name: str) -> ApiResponse:
        """Get a playbook by name."""
        return await self._request("GET", f"/api/playbooks/{name}")

    async def save_playbook(self, name: str, data: dict[str, Any]) -> ApiResponse:
        """Save a playbook."""
        return await self._request("POST", f"/api/playbooks/{name}", json=data)

    async def run_playbook(self, data: dict[str, Any]) -> ApiResponse:
        """Run a playbook."""
        return await self._request("POST", "/api/playbook/run", json=data)

    async def list_records(self) -> ApiResponse:
        """List playbook records."""
        return await self._request("GET", "/api/records")

    # ==================================================================
    # CRUD Check Runner
    # ==================================================================

    async def run_crud_check(self, group: str) -> list[ApiResponse]:
        """Run a full CRUD cycle for a resource group.

        Creates a resource, lists, gets, updates, deletes it,
        and returns all responses collected during the cycle.
        """
        base = _GROUP_PATHS.get(group)
        if base is None:
            err = ApiResponse(
                method="",
                path="",
                status=0,
                latency_ms=0.0,
                error=f"Unknown group: {group}",
            )
            self._results.append(err)
            return [err]

        sample = dict(_SAMPLE_DATA.get(group, {"name": "test"}))
        results: list[ApiResponse] = []

        # Create
        create_resp = await self._create(base, sample)
        results.append(create_resp)

        # Extract ID from create response
        resource_id: str | None = None
        if create_resp.success and isinstance(create_resp.body, dict):
            resource_id = str(
                create_resp.body.get("id") or create_resp.body.get("ref") or ""
            )

        # List
        list_resp = await self._list(base)
        results.append(list_resp)

        # Get, Update, Delete (only if we got an ID)
        if resource_id:
            get_resp = await self._get(base, resource_id)
            results.append(get_resp)

            sample["name"] = sample.get("name", "test") + "-upd"
            update_resp = await self._update(base, resource_id, sample)
            results.append(update_resp)

            delete_resp = await self._delete(base, resource_id)
            results.append(delete_resp)
        else:
            # Still attempt operations with a placeholder ID
            placeholder = "test-placeholder-id"
            results.append(await self._get(base, placeholder))
            results.append(await self._update(base, placeholder, sample))
            results.append(await self._delete(base, placeholder))

        return results

    async def run_full_api_check(self) -> list[ApiResponse]:
        """Run CRUD checks on all groups plus operational endpoints.

        Returns every ApiResponse collected during the run.
        """
        self.clear_results()

        # CRUD groups
        for group in _GROUP_PATHS:
            await self.run_crud_check(group)

        # Active calls (read-only check)
        await self.list_calls()

        # Security
        await self.list_firewall_rules()
        await self.list_blocks()
        await self.get_flood_tracker()
        await self.get_auth_failures()

        # Diagnostics
        await self.get_diagnostic_summary()
        await self.get_registrations()
        await self.test_trunk_connectivity({"trunk_name": "test-trunk"})
        await self.evaluate_routing({"destination": "+14155551234"})

        # System
        await self.get_health()
        await self.get_system_info()
        await self.get_stats()
        await self.get_cluster_status()
        await self.get_system_config()
        await self.reload_config()

        # CDRs
        await self.list_cdrs()

        # Playbooks
        await self.list_playbooks()
        await self.list_records()

        return list(self._results)
