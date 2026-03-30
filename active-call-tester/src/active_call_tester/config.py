from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Any

import yaml
from pydantic import BaseModel, model_validator


_ENV_VAR_PATTERN = re.compile(r"\$\{([^}]+)\}")


def _interpolate_env_vars(value: str) -> str:
    """Replace ${VAR} patterns with os.environ values."""

    def _replace(match: re.Match[str]) -> str:
        var_name = match.group(1)
        return os.environ.get(var_name, "")

    return _ENV_VAR_PATTERN.sub(_replace, value)


def _interpolate_recursive(obj: Any) -> Any:
    """Recursively interpolate env vars in a data structure."""
    if isinstance(obj, str):
        return _interpolate_env_vars(obj)
    if isinstance(obj, dict):
        return {k: _interpolate_recursive(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_interpolate_recursive(item) for item in obj]
    return obj


class TargetConfig(BaseModel):
    """Target server configuration."""

    url: str
    api_key: str = ""
    ws_url: str = ""


class TierConfig(BaseModel):
    """Configuration for a single test tier."""

    concurrency: list[int]
    duration_secs: int
    call_hold_secs: int


class ProviderConfig(BaseModel):
    """Configuration for an ASR or TTS provider."""

    name: str
    type: str  # "offline" or "cloud"
    env_key: str | None = None

    @property
    def is_available(self) -> bool:
        """Check if provider is available in current env."""
        if self.type == "offline":
            return True
        if self.env_key:
            return bool(os.environ.get(self.env_key))
        return False


class ProvidersConfig(BaseModel):
    """ASR and TTS provider lists."""

    asr: list[ProviderConfig]
    tts: list[ProviderConfig]

    def available_asr(self) -> list[ProviderConfig]:
        """Return only ASR providers available in env."""
        return [p for p in self.asr if p.is_available]

    def available_tts(self) -> list[ProviderConfig]:
        """Return only TTS providers available in env."""
        return [p for p in self.tts if p.is_available]

    def asr_names(self) -> set[str]:
        """All ASR provider names."""
        return {p.name for p in self.asr}

    def tts_names(self) -> set[str]:
        """All TTS provider names."""
        return {p.name for p in self.tts}

    def available_asr_names(self) -> set[str]:
        """Available ASR provider names."""
        return {p.name for p in self.available_asr()}

    def available_tts_names(self) -> set[str]:
        """Available TTS provider names."""
        return {p.name for p in self.available_tts()}


class ScenarioConfig(BaseModel):
    """A single test scenario in the matrix."""

    name: str
    protocol: str
    codec: str
    asr: str
    tts: str
    tiers: list[str]
    callee: str = ""
    caller: str = ""


class ThresholdsConfig(BaseModel):
    """Latency thresholds in milliseconds."""

    api_response_p95: float = 200
    ws_command_rtt_p95: float = 100
    call_setup_p95: float = 500
    asr_first_result_p95: float = 800
    tts_first_byte_p95: float = 300
    webrtc_ice_complete_p95: float = 2000


class PrometheusConfig(BaseModel):
    """Prometheus push-gateway configuration."""

    pushgateway_url: str = "http://localhost:9091"
    job_name: str = "active_call_tester"


class OtelConfig(BaseModel):
    """OpenTelemetry exporter configuration."""

    endpoint: str = "http://localhost:4317"
    service_name: str = "active-call-tester"


class MetricsConfig(BaseModel):
    """Metrics export configuration."""

    prometheus: PrometheusConfig = PrometheusConfig()
    opentelemetry: OtelConfig = OtelConfig()


class TestMatrixConfig(BaseModel):
    """Root configuration for the active-call-tester matrix."""

    target: TargetConfig
    tiers: dict[str, TierConfig]
    protocols: list[str]
    codecs: list[str]
    providers: ProvidersConfig
    matrix: list[ScenarioConfig]
    thresholds: ThresholdsConfig = ThresholdsConfig()
    metrics: MetricsConfig = MetricsConfig()

    @model_validator(mode="after")
    def validate_matrix_references(self) -> TestMatrixConfig:
        """Ensure matrix entries reference valid values."""
        valid_protocols = set(self.protocols)
        valid_codecs = set(self.codecs)
        valid_tiers = set(self.tiers.keys())
        valid_asr = self.providers.asr_names()
        valid_tts = self.providers.tts_names()

        for scenario in self.matrix:
            if scenario.protocol not in valid_protocols:
                raise ValueError(
                    f"Scenario '{scenario.name}': protocol "
                    f"'{scenario.protocol}' not in {valid_protocols}"
                )
            if scenario.codec not in valid_codecs:
                raise ValueError(
                    f"Scenario '{scenario.name}': codec "
                    f"'{scenario.codec}' not in {valid_codecs}"
                )
            if scenario.asr not in valid_asr:
                raise ValueError(
                    f"Scenario '{scenario.name}': ASR provider "
                    f"'{scenario.asr}' not in {valid_asr}"
                )
            if scenario.tts not in valid_tts:
                raise ValueError(
                    f"Scenario '{scenario.name}': TTS provider "
                    f"'{scenario.tts}' not in {valid_tts}"
                )
            for tier in scenario.tiers:
                if tier not in valid_tiers:
                    raise ValueError(
                        f"Scenario '{scenario.name}': tier "
                        f"'{tier}' not in {valid_tiers}"
                    )
        return self

    def get_scenarios(self, tier: str) -> list[ScenarioConfig]:
        """Return scenarios that include the given tier."""
        return [s for s in self.matrix if tier in s.tiers]

    def get_available_scenarios(self, tier: str) -> list[ScenarioConfig]:
        """Return tier scenarios with available providers."""
        available_asr = self.providers.available_asr_names()
        available_tts = self.providers.available_tts_names()
        return [
            s
            for s in self.matrix
            if tier in s.tiers and s.asr in available_asr and s.tts in available_tts
        ]


def load_config(path: str | Path) -> TestMatrixConfig:
    """Load and validate test matrix from YAML file.

    Reads the YAML, interpolates ${VAR} env-var patterns,
    and returns a validated TestMatrixConfig.
    """
    path = Path(path)
    with path.open() as fh:
        raw = yaml.safe_load(fh)

    interpolated = _interpolate_recursive(raw)
    return TestMatrixConfig.model_validate(interpolated)
