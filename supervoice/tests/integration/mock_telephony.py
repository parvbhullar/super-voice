"""Mock telephony that drives POST /v1/dispatch like a real media gateway."""

from __future__ import annotations

from typing import Any

from fastapi.testclient import TestClient


def simulate_inbound_call(
    client: TestClient,
    *,
    from_number: str = "+91-caller",
    to_number: str = "+91-test",
    sdp_offer: str = "v=0\r\nfake-sdp-offer",
    api_secret: str = "test-secret",
    external_call_id: str | None = None,
) -> dict[str, Any]:
    """POST /v1/dispatch as a mock telephony gateway.

    Returns the response JSON (session_id, state, sdp_answer, ...).
    """
    body: dict[str, Any] = {
        "direction": "inbound",
        "from_number": from_number,
        "to_number": to_number,
        "sdp_offer": sdp_offer,
    }
    if external_call_id:
        body["external_call_id"] = external_call_id

    r = client.post(
        "/v1/dispatch",
        json=body,
        headers={"Authorization": f"Bearer {api_secret}"},
    )
    return {"status_code": r.status_code, **r.json()}
