from __future__ import annotations

import os
from pathlib import Path
from unittest.mock import patch

import pytest
from pydantic import ValidationError

from active_call_tester.config import (
    MetricsConfig,
    OtelConfig,
    PrometheusConfig,
    ProviderConfig,
    ProvidersConfig,
    ScenarioConfig,
    TargetConfig,
    TestMatrixConfig,
    ThresholdsConfig,
    TierConfig,
    _interpolate_env_vars,
    _interpolate_recursive,
    load_config,
)

YAML_PATH = Path(__file__).resolve().parent.parent / "config" / "test-matrix.yaml"


# --------------- env var interpolation ---------------


class TestInterpolateEnvVars:
    def test_simple_substitution(self) -> None:
        with patch.dict(os.environ, {"MY_VAR": "hello"}):
            assert _interpolate_env_vars("${MY_VAR}") == "hello"

    def test_missing_var_becomes_empty(self) -> None:
        env = {k: v for k, v in os.environ.items()}
        env.pop("NONEXISTENT_KEY_XYZ", None)
        with patch.dict(os.environ, env, clear=True):
            assert _interpolate_env_vars("${NONEXISTENT_KEY_XYZ}") == ""

    def test_multiple_vars(self) -> None:
        with patch.dict(os.environ, {"A": "1", "B": "2"}):
            result = _interpolate_env_vars("${A}-${B}")
            assert result == "1-2"

    def test_no_vars_unchanged(self) -> None:
        assert _interpolate_env_vars("plain text") == "plain text"

    def test_partial_string(self) -> None:
        with patch.dict(os.environ, {"HOST": "example.com"}):
            result = _interpolate_env_vars("https://${HOST}/api")
            assert result == "https://example.com/api"


class TestInterpolateRecursive:
    def test_dict(self) -> None:
        with patch.dict(os.environ, {"X": "val"}):
            result = _interpolate_recursive({"key": "${X}"})
            assert result == {"key": "val"}

    def test_list(self) -> None:
        with patch.dict(os.environ, {"X": "val"}):
            result = _interpolate_recursive(["${X}", "static"])
            assert result == ["val", "static"]

    def test_nested(self) -> None:
        with patch.dict(os.environ, {"X": "val"}):
            result = _interpolate_recursive({"a": [{"b": "${X}"}]})
            assert result == {"a": [{"b": "val"}]}

    def test_non_string_passthrough(self) -> None:
        assert _interpolate_recursive(42) == 42
        assert _interpolate_recursive(True) is True
        assert _interpolate_recursive(None) is None


# --------------- provider availability ---------------


class TestProviderAvailability:
    def test_offline_always_available(self) -> None:
        p = ProviderConfig(name="local", type="offline")
        assert p.is_available is True

    def test_cloud_available_when_env_set(self) -> None:
        p = ProviderConfig(name="azure", type="cloud", env_key="TEST_AZURE_KEY")
        with patch.dict(os.environ, {"TEST_AZURE_KEY": "secret"}):
            assert p.is_available is True

    def test_cloud_unavailable_when_env_missing(self) -> None:
        p = ProviderConfig(name="azure", type="cloud", env_key="MISSING_KEY_XYZ")
        env = {k: v for k, v in os.environ.items()}
        env.pop("MISSING_KEY_XYZ", None)
        with patch.dict(os.environ, env, clear=True):
            assert p.is_available is False

    def test_cloud_no_env_key_unavailable(self) -> None:
        p = ProviderConfig(name="anon", type="cloud")
        assert p.is_available is False


class TestProvidersConfig:
    def _make_providers(self) -> ProvidersConfig:
        return ProvidersConfig(
            asr=[
                ProviderConfig(name="local_asr", type="offline"),
                ProviderConfig(
                    name="cloud_asr",
                    type="cloud",
                    env_key="CLOUD_ASR_KEY",
                ),
            ],
            tts=[
                ProviderConfig(name="local_tts", type="offline"),
                ProviderConfig(
                    name="cloud_tts",
                    type="cloud",
                    env_key="CLOUD_TTS_KEY",
                ),
            ],
        )

    def test_available_asr_offline_only(self) -> None:
        providers = self._make_providers()
        env = {k: v for k, v in os.environ.items()}
        env.pop("CLOUD_ASR_KEY", None)
        with patch.dict(os.environ, env, clear=True):
            available = providers.available_asr()
            assert len(available) == 1
            assert available[0].name == "local_asr"

    def test_available_asr_with_cloud(self) -> None:
        providers = self._make_providers()
        with patch.dict(os.environ, {"CLOUD_ASR_KEY": "k"}):
            available = providers.available_asr()
            assert len(available) == 2

    def test_available_tts_offline_only(self) -> None:
        providers = self._make_providers()
        env = {k: v for k, v in os.environ.items()}
        env.pop("CLOUD_TTS_KEY", None)
        with patch.dict(os.environ, env, clear=True):
            available = providers.available_tts()
            assert len(available) == 1
            assert available[0].name == "local_tts"

    def test_asr_names(self) -> None:
        providers = self._make_providers()
        assert providers.asr_names() == {
            "local_asr",
            "cloud_asr",
        }

    def test_tts_names(self) -> None:
        providers = self._make_providers()
        assert providers.tts_names() == {
            "local_tts",
            "cloud_tts",
        }


# --------------- scenario filtering ---------------


class TestScenarioFiltering:
    def _make_config(self) -> TestMatrixConfig:
        return TestMatrixConfig(
            target=TargetConfig(url="http://localhost:8080"),
            tiers={
                "smoke": TierConfig(
                    concurrency=[1],
                    duration_secs=30,
                    call_hold_secs=5,
                ),
                "load": TierConfig(
                    concurrency=[10],
                    duration_secs=120,
                    call_hold_secs=10,
                ),
            },
            protocols=["websocket", "sip"],
            codecs=["pcmu", "opus"],
            providers=ProvidersConfig(
                asr=[
                    ProviderConfig(name="offline_asr", type="offline"),
                    ProviderConfig(
                        name="cloud_asr",
                        type="cloud",
                        env_key="CLOUD_ASR_KEY",
                    ),
                ],
                tts=[
                    ProviderConfig(name="offline_tts", type="offline"),
                    ProviderConfig(
                        name="cloud_tts",
                        type="cloud",
                        env_key="CLOUD_TTS_KEY",
                    ),
                ],
            ),
            matrix=[
                ScenarioConfig(
                    name="s1",
                    protocol="websocket",
                    codec="pcmu",
                    asr="offline_asr",
                    tts="offline_tts",
                    tiers=["smoke", "load"],
                ),
                ScenarioConfig(
                    name="s2",
                    protocol="sip",
                    codec="opus",
                    asr="cloud_asr",
                    tts="cloud_tts",
                    tiers=["smoke"],
                ),
            ],
        )

    def test_get_scenarios_smoke(self) -> None:
        cfg = self._make_config()
        scenarios = cfg.get_scenarios("smoke")
        assert len(scenarios) == 2

    def test_get_scenarios_load(self) -> None:
        cfg = self._make_config()
        scenarios = cfg.get_scenarios("load")
        assert len(scenarios) == 1
        assert scenarios[0].name == "s1"

    def test_get_scenarios_nonexistent_tier(self) -> None:
        cfg = self._make_config()
        assert cfg.get_scenarios("stress") == []

    def test_get_available_scenarios_offline(self) -> None:
        """Cloud env vars missing: only offline scenarios."""
        cfg = self._make_config()
        env = {k: v for k, v in os.environ.items()}
        env.pop("CLOUD_ASR_KEY", None)
        env.pop("CLOUD_TTS_KEY", None)
        with patch.dict(os.environ, env, clear=True):
            available = cfg.get_available_scenarios("smoke")
            assert len(available) == 1
            assert available[0].name == "s1"

    def test_get_available_scenarios_all(self) -> None:
        """Cloud env vars set: all scenarios available."""
        cfg = self._make_config()
        with patch.dict(
            os.environ,
            {"CLOUD_ASR_KEY": "k", "CLOUD_TTS_KEY": "k"},
        ):
            available = cfg.get_available_scenarios("smoke")
            assert len(available) == 2


# --------------- validation errors ---------------


class TestValidationErrors:
    def _base_kwargs(self) -> dict[str, object]:
        return {
            "target": TargetConfig(url="http://localhost"),
            "tiers": {
                "smoke": TierConfig(
                    concurrency=[1],
                    duration_secs=30,
                    call_hold_secs=5,
                ),
            },
            "protocols": ["websocket"],
            "codecs": ["pcmu"],
            "providers": ProvidersConfig(
                asr=[
                    ProviderConfig(name="local_asr", type="offline"),
                ],
                tts=[
                    ProviderConfig(name="local_tts", type="offline"),
                ],
            ),
            "matrix": [
                ScenarioConfig(
                    name="ok",
                    protocol="websocket",
                    codec="pcmu",
                    asr="local_asr",
                    tts="local_tts",
                    tiers=["smoke"],
                ),
            ],
        }

    def test_invalid_protocol(self) -> None:
        kwargs = self._base_kwargs()
        kwargs["matrix"] = [
            ScenarioConfig(
                name="bad",
                protocol="grpc",
                codec="pcmu",
                asr="local_asr",
                tts="local_tts",
                tiers=["smoke"],
            ),
        ]
        with pytest.raises(ValidationError, match="protocol"):
            TestMatrixConfig(**kwargs)  # type: ignore[arg-type]

    def test_invalid_codec(self) -> None:
        kwargs = self._base_kwargs()
        kwargs["matrix"] = [
            ScenarioConfig(
                name="bad",
                protocol="websocket",
                codec="mp3",
                asr="local_asr",
                tts="local_tts",
                tiers=["smoke"],
            ),
        ]
        with pytest.raises(ValidationError, match="codec"):
            TestMatrixConfig(**kwargs)  # type: ignore[arg-type]

    def test_invalid_asr(self) -> None:
        kwargs = self._base_kwargs()
        kwargs["matrix"] = [
            ScenarioConfig(
                name="bad",
                protocol="websocket",
                codec="pcmu",
                asr="nonexistent",
                tts="local_tts",
                tiers=["smoke"],
            ),
        ]
        with pytest.raises(ValidationError, match="ASR"):
            TestMatrixConfig(**kwargs)  # type: ignore[arg-type]

    def test_invalid_tts(self) -> None:
        kwargs = self._base_kwargs()
        kwargs["matrix"] = [
            ScenarioConfig(
                name="bad",
                protocol="websocket",
                codec="pcmu",
                asr="local_asr",
                tts="nonexistent",
                tiers=["smoke"],
            ),
        ]
        with pytest.raises(ValidationError, match="TTS"):
            TestMatrixConfig(**kwargs)  # type: ignore[arg-type]

    def test_invalid_tier(self) -> None:
        kwargs = self._base_kwargs()
        kwargs["matrix"] = [
            ScenarioConfig(
                name="bad",
                protocol="websocket",
                codec="pcmu",
                asr="local_asr",
                tts="local_tts",
                tiers=["nonexistent"],
            ),
        ]
        with pytest.raises(ValidationError, match="tier"):
            TestMatrixConfig(**kwargs)  # type: ignore[arg-type]


# --------------- YAML loading ---------------


class TestLoadConfig:
    def test_load_actual_yaml(self) -> None:
        """Load the real test-matrix.yaml and validate."""
        with patch.dict(os.environ, {"API_KEY": "test-key"}):
            cfg = load_config(YAML_PATH)
        assert cfg.target.url == "http://localhost:8080"
        assert cfg.target.api_key == "test-key"
        assert "smoke" in cfg.tiers
        assert "load" in cfg.tiers
        assert "stress" in cfg.tiers
        assert len(cfg.protocols) == 4
        assert len(cfg.codecs) == 4
        assert len(cfg.matrix) == 4

    def test_env_var_interpolation_in_yaml(self) -> None:
        with patch.dict(os.environ, {"API_KEY": "secret-123"}):
            cfg = load_config(YAML_PATH)
        assert cfg.target.api_key == "secret-123"

    def test_missing_env_var_becomes_empty(self) -> None:
        env = {k: v for k, v in os.environ.items()}
        env.pop("API_KEY", None)
        with patch.dict(os.environ, env, clear=True):
            cfg = load_config(YAML_PATH)
        assert cfg.target.api_key == ""

    def test_thresholds_loaded(self) -> None:
        with patch.dict(os.environ, {"API_KEY": "k"}):
            cfg = load_config(YAML_PATH)
        assert cfg.thresholds.api_response_p95 == 200
        assert cfg.thresholds.call_setup_p95 == 500

    def test_metrics_loaded(self) -> None:
        with patch.dict(os.environ, {"API_KEY": "k"}):
            cfg = load_config(YAML_PATH)
        assert cfg.metrics.prometheus.pushgateway_url == "http://localhost:9091"
        assert cfg.metrics.opentelemetry.service_name == "active-call-tester"

    def test_scenarios_in_yaml(self) -> None:
        with patch.dict(os.environ, {"API_KEY": "k"}):
            cfg = load_config(YAML_PATH)
        names = {s.name for s in cfg.matrix}
        assert "offline-pcmu-ws" in names
        assert "browser-smoke" in names

    def test_providers_in_yaml(self) -> None:
        with patch.dict(os.environ, {"API_KEY": "k"}):
            cfg = load_config(YAML_PATH)
        asr_names = cfg.providers.asr_names()
        assert "sensevoice" in asr_names
        assert "aliyun" in asr_names
        tts_names = cfg.providers.tts_names()
        assert "supertonic" in tts_names

    def test_file_not_found(self) -> None:
        with pytest.raises(FileNotFoundError):
            load_config("/nonexistent/path.yaml")


# --------------- individual model tests ---------------


class TestIndividualModels:
    def test_target_defaults(self) -> None:
        t = TargetConfig(url="http://localhost")
        assert t.api_key == ""
        assert t.ws_url == ""

    def test_tier_config(self) -> None:
        tc = TierConfig(
            concurrency=[1, 5],
            duration_secs=30,
            call_hold_secs=5,
        )
        assert tc.concurrency == [1, 5]

    def test_thresholds_defaults(self) -> None:
        th = ThresholdsConfig()
        assert th.api_response_p95 == 200
        assert th.webrtc_ice_complete_p95 == 2000

    def test_prometheus_defaults(self) -> None:
        p = PrometheusConfig()
        assert p.job_name == "active_call_tester"

    def test_otel_defaults(self) -> None:
        o = OtelConfig()
        assert o.endpoint == "http://localhost:4317"

    def test_metrics_defaults(self) -> None:
        m = MetricsConfig()
        assert m.prometheus.job_name == "active_call_tester"
        assert m.opentelemetry.service_name == "active-call-tester"
