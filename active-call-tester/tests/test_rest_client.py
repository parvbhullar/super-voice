from __future__ import annotations

from typing import Any
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from active_call_tester.clients.rest import (
    ApiResponse,
    RestClient,
    _GROUP_PATHS,
    _SAMPLE_DATA,
)


# ==================================================================
# ApiResponse dataclass tests
# ==================================================================


class TestApiResponse:
    """Tests for ApiResponse dataclass."""

    def test_success_true_for_200(self) -> None:
        resp = ApiResponse(method="GET", path="/x", status=200, latency_ms=1.0)
        assert resp.success is True

    def test_success_true_for_201(self) -> None:
        resp = ApiResponse(method="POST", path="/x", status=201, latency_ms=2.0)
        assert resp.success is True

    def test_success_true_for_204(self) -> None:
        resp = ApiResponse(
            method="DELETE",
            path="/x",
            status=204,
            latency_ms=0.5,
        )
        assert resp.success is True

    def test_success_false_for_400(self) -> None:
        resp = ApiResponse(
            method="PUT",
            path="/x",
            status=400,
            latency_ms=3.0,
            error="bad request",
        )
        assert resp.success is False

    def test_success_false_for_500(self) -> None:
        resp = ApiResponse(
            method="GET",
            path="/x",
            status=500,
            latency_ms=10.0,
        )
        assert resp.success is False

    def test_success_false_for_0(self) -> None:
        resp = ApiResponse(
            method="GET",
            path="/x",
            status=0,
            latency_ms=0.0,
            error="timeout",
        )
        assert resp.success is False

    def test_body_defaults_to_none(self) -> None:
        resp = ApiResponse(method="GET", path="/x", status=200, latency_ms=1.0)
        assert resp.body is None

    def test_error_defaults_to_none(self) -> None:
        resp = ApiResponse(method="GET", path="/x", status=200, latency_ms=1.0)
        assert resp.error is None

    def test_body_round_trips(self) -> None:
        body = {"id": "abc", "name": "test"}
        resp = ApiResponse(
            method="GET",
            path="/x",
            status=200,
            latency_ms=1.0,
            body=body,
        )
        assert resp.body == body


# ==================================================================
# RestClient initialisation
# ==================================================================


class TestRestClientInit:
    """Tests for RestClient constructor."""

    def test_base_url_trailing_slash_stripped(self) -> None:
        client = RestClient("http://localhost:8080/")
        assert client._base_url == "http://localhost:8080"

    def test_base_url_no_trailing_slash(self) -> None:
        client = RestClient("http://localhost:8080")
        assert client._base_url == "http://localhost:8080"

    def test_api_key_stored(self) -> None:
        client = RestClient("http://x", api_key="secret")
        assert client._api_key == "secret"

    def test_api_key_defaults_empty(self) -> None:
        client = RestClient("http://x")
        assert client._api_key == ""

    def test_session_starts_none(self) -> None:
        client = RestClient("http://x")
        assert client._session is None

    def test_results_starts_empty(self) -> None:
        client = RestClient("http://x")
        assert client.results == []


# ==================================================================
# Connection / Disconnection
# ==================================================================


class TestRestClientConnect:
    """Tests for connect and disconnect."""

    @pytest.mark.asyncio
    async def test_connect_creates_session(self) -> None:
        client = RestClient("http://localhost:8080", api_key="tok")
        with patch("active_call_tester.clients.rest.aiohttp.ClientSession") as mock_cls:
            mock_session = MagicMock()
            mock_cls.return_value = mock_session
            await client.connect()
            assert client._session is mock_session
            mock_cls.assert_called_once()
            call_kwargs = mock_cls.call_args[1]
            assert call_kwargs["headers"]["Authorization"] == "Bearer tok"
            assert call_kwargs["headers"]["Content-Type"] == "application/json"

    @pytest.mark.asyncio
    async def test_connect_no_api_key_no_auth_header(
        self,
    ) -> None:
        client = RestClient("http://localhost:8080")
        with patch("active_call_tester.clients.rest.aiohttp.ClientSession") as mock_cls:
            mock_cls.return_value = MagicMock()
            await client.connect()
            call_kwargs = mock_cls.call_args[1]
            assert "Authorization" not in call_kwargs["headers"]

    @pytest.mark.asyncio
    async def test_disconnect_closes_session(self) -> None:
        client = RestClient("http://x")
        mock_session = AsyncMock()
        client._session = mock_session
        await client.disconnect()
        mock_session.close.assert_awaited_once()
        assert client._session is None

    @pytest.mark.asyncio
    async def test_disconnect_when_no_session(self) -> None:
        client = RestClient("http://x")
        await client.disconnect()  # should not raise
        assert client._session is None


# ==================================================================
# _request
# ==================================================================


def _make_mock_response(
    status: int = 200,
    json_body: Any = None,
) -> MagicMock:
    """Build a mock aiohttp response context manager."""
    mock_resp = MagicMock()
    mock_resp.status = status
    mock_resp.json = AsyncMock(return_value=json_body or {})
    mock_resp.text = AsyncMock(return_value="")
    mock_cm = MagicMock()
    mock_cm.__aenter__ = AsyncMock(return_value=mock_resp)
    mock_cm.__aexit__ = AsyncMock(return_value=False)
    return mock_cm


class TestRequestMethod:
    """Tests for _request timing and result tracking."""

    @pytest.mark.asyncio
    async def test_request_no_session_returns_error(
        self,
    ) -> None:
        client = RestClient("http://x")
        resp = await client._request("GET", "/test")
        assert resp.status == 0
        assert resp.error == "Session not connected"
        assert len(client.results) == 0  # not appended

    @pytest.mark.asyncio
    async def test_request_success_captures_latency(
        self,
    ) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(
            return_value=_make_mock_response(200, {"ok": True})
        )
        client._session = mock_session

        resp = await client._request("GET", "/api/v1/system/health")
        assert resp.status == 200
        assert resp.success is True
        assert resp.latency_ms >= 0
        assert resp.body == {"ok": True}
        assert resp.error is None
        assert len(client.results) == 1

    @pytest.mark.asyncio
    async def test_request_error_status_captures_error(
        self,
    ) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(
            return_value=_make_mock_response(404, {"detail": "not found"})
        )
        client._session = mock_session

        resp = await client._request("GET", "/missing")
        assert resp.status == 404
        assert resp.success is False
        assert resp.error is not None

    @pytest.mark.asyncio
    async def test_request_network_error(self) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()

        mock_cm = MagicMock()
        mock_cm.__aenter__ = AsyncMock(side_effect=ConnectionError("refused"))
        mock_cm.__aexit__ = AsyncMock(return_value=False)
        mock_session.request = MagicMock(return_value=mock_cm)
        client._session = mock_session

        resp = await client._request("GET", "/fail")
        assert resp.status == 0
        assert "refused" in (resp.error or "")
        assert resp.latency_ms >= 0

    @pytest.mark.asyncio
    async def test_request_json_fallback_to_text(
        self,
    ) -> None:
        client = RestClient("http://x")
        mock_resp = MagicMock()
        mock_resp.status = 200
        mock_resp.json = AsyncMock(side_effect=ValueError("bad json"))
        mock_resp.text = AsyncMock(return_value="plain text")
        mock_cm = MagicMock()
        mock_cm.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_cm.__aexit__ = AsyncMock(return_value=False)

        mock_session = MagicMock()
        mock_session.request = MagicMock(return_value=mock_cm)
        client._session = mock_session

        resp = await client._request("GET", "/text")
        assert resp.body == "plain text"
        assert resp.success is True


# ==================================================================
# Results management
# ==================================================================


class TestResultsManagement:
    """Tests for results property and clear_results."""

    @pytest.mark.asyncio
    async def test_results_returns_copy(self) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(return_value=_make_mock_response(200))
        client._session = mock_session

        await client._request("GET", "/a")
        results = client.results
        results.clear()
        assert len(client.results) == 1  # original intact

    @pytest.mark.asyncio
    async def test_clear_results(self) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(return_value=_make_mock_response(200))
        client._session = mock_session

        await client._request("GET", "/a")
        assert len(client.results) == 1
        client.clear_results()
        assert len(client.results) == 0


# ==================================================================
# CRUD check logic
# ==================================================================


class TestCrudCheck:
    """Tests for run_crud_check with mocked responses."""

    def _setup_client(self, create_body: dict[str, Any] | None = None) -> RestClient:
        """Create a RestClient with a mocked session."""
        client = RestClient("http://x")
        body = create_body or {"id": "new-123"}
        mock_session = MagicMock()
        mock_session.request = MagicMock(return_value=_make_mock_response(200, body))
        client._session = mock_session
        return client

    @pytest.mark.asyncio
    async def test_unknown_group(self) -> None:
        client = self._setup_client()
        results = await client.run_crud_check("nonexistent")
        assert len(results) == 1
        assert results[0].error == "Unknown group: nonexistent"

    @pytest.mark.asyncio
    async def test_crud_cycle_with_id(self) -> None:
        client = self._setup_client({"id": "abc-1"})
        results = await client.run_crud_check("endpoints")
        # create + list + get + update + delete = 5
        assert len(results) == 5
        assert all(r.success for r in results)

    @pytest.mark.asyncio
    async def test_crud_cycle_without_id(self) -> None:
        client = self._setup_client({"status": "ok"})
        results = await client.run_crud_check("gateways")
        # create + list + get + update + delete = 5
        assert len(results) == 5

    @pytest.mark.asyncio
    async def test_all_groups_have_sample_data(self) -> None:
        for group in _GROUP_PATHS:
            assert group in _SAMPLE_DATA, f"Missing sample data for group: {group}"


# ==================================================================
# run_full_api_check
# ==================================================================


class TestFullApiCheck:
    """Tests for run_full_api_check."""

    @pytest.mark.asyncio
    async def test_full_check_clears_results_first(
        self,
    ) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(
            return_value=_make_mock_response(200, {"id": "x"})
        )
        client._session = mock_session

        # Pre-fill a result
        client._results.append(
            ApiResponse(
                method="GET",
                path="/old",
                status=200,
                latency_ms=0,
            )
        )

        results = await client.run_full_api_check()
        # Should NOT contain the pre-filled result
        assert not any(r.path == "/old" for r in results)

    @pytest.mark.asyncio
    async def test_full_check_covers_all_groups(
        self,
    ) -> None:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(
            return_value=_make_mock_response(200, {"id": "x"})
        )
        client._session = mock_session

        results = await client.run_full_api_check()
        # 8 groups * 5 CRUD ops + operational endpoints
        # At minimum, should be well above 40
        assert len(results) > 40

        # Verify system health was called
        paths = {r.path for r in results}
        assert "/api/v1/system/health" in paths
        assert "/api/v1/cdrs" in paths
        assert "/api/playbooks" in paths


# ==================================================================
# Endpoint method routing (spot-checks)
# ==================================================================


class TestEndpointRouting:
    """Spot-check that individual methods hit correct paths."""

    def _make_client(self) -> RestClient:
        client = RestClient("http://x")
        mock_session = MagicMock()
        mock_session.request = MagicMock(return_value=_make_mock_response(200))
        client._session = mock_session
        return client

    @pytest.mark.asyncio
    async def test_list_endpoints_path(self) -> None:
        client = self._make_client()
        await client.list_endpoints()
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == ("GET", "/api/v1/endpoints")

    @pytest.mark.asyncio
    async def test_create_gateway_path(self) -> None:
        client = self._make_client()
        await client.create_gateway({"name": "gw"})
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == ("POST", "/api/v1/gateways")

    @pytest.mark.asyncio
    async def test_delete_trunk_path(self) -> None:
        client = self._make_client()
        await client.delete_trunk("t-1")
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "DELETE",
            "/api/v1/trunks/t-1",
        )

    @pytest.mark.asyncio
    async def test_trunk_credentials_path(self) -> None:
        client = self._make_client()
        await client.get_trunk_credentials("t-1")
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "GET",
            "/api/v1/trunks/t-1/credentials",
        )

    @pytest.mark.asyncio
    async def test_hangup_call_path(self) -> None:
        client = self._make_client()
        await client.hangup_call("c-99")
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "POST",
            "/api/v1/calls/c-99/hangup",
        )

    @pytest.mark.asyncio
    async def test_get_health_path(self) -> None:
        client = self._make_client()
        await client.get_health()
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "GET",
            "/api/v1/system/health",
        )

    @pytest.mark.asyncio
    async def test_list_cdrs_path(self) -> None:
        client = self._make_client()
        await client.list_cdrs()
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == ("GET", "/api/v1/cdrs")

    @pytest.mark.asyncio
    async def test_save_playbook_path(self) -> None:
        client = self._make_client()
        await client.save_playbook("pb1", {"steps": []})
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "POST",
            "/api/playbooks/pb1",
        )

    @pytest.mark.asyncio
    async def test_add_firewall_rule_path(self) -> None:
        client = self._make_client()
        await client.add_firewall_rule({"ip": "1.2.3.4"})
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "POST",
            "/api/v1/security/firewall",
        )

    @pytest.mark.asyncio
    async def test_evaluate_routing_path(self) -> None:
        client = self._make_client()
        await client.evaluate_routing({"number": "+15551234567"})
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "POST",
            "/api/v1/diagnostics/route-eval",
        )

    @pytest.mark.asyncio
    async def test_get_cdr_sip_flow_path(self) -> None:
        client = self._make_client()
        await client.get_cdr_sip_flow("cdr-1")
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == (
            "GET",
            "/api/v1/cdrs/cdr-1/sip-flow",
        )

    @pytest.mark.asyncio
    async def test_run_playbook_path(self) -> None:
        client = self._make_client()
        await client.run_playbook({"name": "test"})
        call_args = client._session.request.call_args  # type: ignore[union-attr]
        assert call_args[0] == ("POST", "/api/playbook/run")
