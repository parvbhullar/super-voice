import pytest

from supervoice.shared.config import Settings


def test_settings_loads_from_env(monkeypatch):
    monkeypatch.setenv("AGENT_BRIDGE_URL", "ws://localhost:7000/bridge")
    monkeypatch.setenv("DEEPGRAM_API_KEY", "dg_test")
    monkeypatch.setenv("CARTESIA_API_KEY", "ct_test")
    s = Settings()
    assert s.agent_bridge_url == "ws://localhost:7000/bridge"
    assert s.deepgram_api_key.get_secret_value() == "dg_test"
    assert s.cartesia_api_key.get_secret_value() == "ct_test"
    assert s.host == "0.0.0.0"
    assert s.port == 8080


def test_settings_missing_required_raises(monkeypatch):
    monkeypatch.delenv("AGENT_BRIDGE_URL", raising=False)
    monkeypatch.delenv("DEEPGRAM_API_KEY", raising=False)
    monkeypatch.delenv("CARTESIA_API_KEY", raising=False)
    with pytest.raises(Exception):
        Settings()
