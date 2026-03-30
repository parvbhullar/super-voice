from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import MagicMock, patch

from active_call_tester.metrics.exporters import (
    JsonExporter,
    OtelExporter,
    PrometheusExporter,
)
from active_call_tester.models import (
    CallResult,
    Protocol,
    Tier,
    TierResult,
)


def _make_result(
    scenario: str = "ws-opus-deepgram-elevenlabs",
    success: bool = True,
    setup_ms: float = 100.0,
    tts_ms: float = 200.0,
    asr_ms: float = 300.0,
    error: str | None = None,
) -> CallResult:
    """Helper to create a CallResult with defaults."""
    return CallResult(
        scenario=scenario,
        protocol=Protocol.WEBSOCKET,
        codec="opus",
        asr_provider="deepgram",
        tts_provider="elevenlabs",
        setup_latency_ms=setup_ms,
        first_tts_byte_ms=tts_ms,
        first_asr_result_ms=asr_ms,
        teardown_ms=10.0,
        total_duration_ms=setup_ms + tts_ms + asr_ms + 10.0,
        success=success,
        error=error,
    )


def _make_tier_result(
    results: list[CallResult] | None = None,
) -> TierResult:
    """Helper to create a TierResult with defaults."""
    if results is None:
        results = [_make_result(), _make_result(success=False, error="timeout")]
    return TierResult(
        tier=Tier.SMOKE,
        scenario="ws-opus-deepgram-elevenlabs",
        concurrency=2,
        total_calls=len(results),
        successful_calls=sum(1 for r in results if r.success),
        failed_calls=sum(1 for r in results if not r.success),
        results=results,
        p50_setup_ms=100.0,
        p95_setup_ms=150.0,
        p99_setup_ms=180.0,
        p50_tts_ms=200.0,
        p95_tts_ms=250.0,
        p99_tts_ms=280.0,
        p50_asr_ms=300.0,
        p95_asr_ms=350.0,
        p99_asr_ms=380.0,
    )


class TestPrometheusExporter:
    """Tests for PrometheusExporter."""

    def test_lazy_initialization(self) -> None:
        exporter = PrometheusExporter()
        assert not exporter._initialized
        assert exporter.call_setup_histogram is None

    def test_ensure_initialized(self) -> None:
        exporter = PrometheusExporter()
        exporter._ensure_initialized()
        assert exporter._initialized
        assert exporter.call_setup_histogram is not None
        assert exporter._registry is not None

    def test_double_init_is_noop(self) -> None:
        exporter = PrometheusExporter()
        exporter._ensure_initialized()
        registry = exporter._registry
        exporter._ensure_initialized()
        assert exporter._registry is registry

    def test_record_call_result_success(self) -> None:
        exporter = PrometheusExporter()
        result = _make_result()
        exporter.record_call_result(result, tier="smoke")
        assert exporter._initialized

    def test_record_call_result_failure(self) -> None:
        exporter = PrometheusExporter()
        result = _make_result(success=False, error="timeout")
        exporter.record_call_result(result, tier="smoke")
        assert exporter._initialized

    def test_record_call_result_zero_latencies(self) -> None:
        exporter = PrometheusExporter()
        result = _make_result(setup_ms=0.0, tts_ms=0.0, asr_ms=0.0)
        exporter.record_call_result(result, tier="smoke")
        assert exporter._initialized

    def test_record_api_response(self) -> None:
        exporter = PrometheusExporter()
        exporter.record_api_response("GET", "/health", 50.0)
        assert exporter._initialized

    def test_custom_config(self) -> None:
        exporter = PrometheusExporter(
            pushgateway_url="http://custom:9091",
            job_name="custom_job",
        )
        assert exporter._pushgateway_url == "http://custom:9091"
        assert exporter._job_name == "custom_job"


class TestOtelExporter:
    """Tests for OtelExporter."""

    def test_lazy_initialization(self) -> None:
        exporter = OtelExporter()
        assert exporter._tracer is None
        assert exporter._provider is None

    @patch("active_call_tester.metrics.exporters.OtelExporter._ensure_initialized")
    def test_record_call_result_initializes(self, mock_init: MagicMock) -> None:
        exporter = OtelExporter()
        mock_tracer = MagicMock()
        mock_span = MagicMock()
        mock_tracer.start_as_current_span.return_value = mock_span
        mock_span.__enter__ = MagicMock(return_value=mock_span)
        mock_span.__exit__ = MagicMock(return_value=False)
        exporter._tracer = mock_tracer
        result = _make_result()
        exporter.record_call_result(result)
        mock_init.assert_called_once()

    def test_custom_config(self) -> None:
        exporter = OtelExporter(
            endpoint="http://custom:4317",
            service_name="custom-svc",
        )
        assert exporter._endpoint == "http://custom:4317"
        assert exporter._service_name == "custom-svc"

    async def test_shutdown_no_provider(self) -> None:
        exporter = OtelExporter()
        await exporter.shutdown()

    async def test_shutdown_with_provider(self) -> None:
        exporter = OtelExporter()
        mock_provider = MagicMock()
        exporter._provider = mock_provider
        await exporter.shutdown()
        mock_provider.shutdown.assert_called_once()


class TestJsonExporter:
    """Tests for JsonExporter."""

    def test_creates_directory(self, tmp_path: Path) -> None:
        output_dir = tmp_path / "nested" / "results"
        exporter = JsonExporter(output_dir=str(output_dir))
        tr = _make_tier_result()
        filepath = exporter.export_tier_result(tr)
        assert output_dir.exists()
        assert filepath.exists()

    def test_file_naming_convention(self, tmp_path: Path) -> None:
        exporter = JsonExporter(output_dir=str(tmp_path))
        tr = _make_tier_result()
        filepath = exporter.export_tier_result(tr)
        name = filepath.name
        assert name.endswith(".json")
        assert "smoke" in name
        assert "ws-opus-deepgram-elevenlabs" in name
        # Format: YYYYMMDD_HHMMSS-tier-scenario.json
        parts = name.replace(".json", "").split("-", 1)
        assert len(parts[0].split("_")) == 2

    def test_valid_json_output(self, tmp_path: Path) -> None:
        exporter = JsonExporter(output_dir=str(tmp_path))
        tr = _make_tier_result()
        filepath = exporter.export_tier_result(tr)
        data = json.loads(filepath.read_text())
        assert isinstance(data, dict)

    def test_export_round_trip(self, tmp_path: Path) -> None:
        results = [
            _make_result(setup_ms=100.0),
            _make_result(
                success=False,
                error="timeout",
                setup_ms=200.0,
            ),
        ]
        tr = _make_tier_result(results=results)
        exporter = JsonExporter(output_dir=str(tmp_path))
        filepath = exporter.export_tier_result(tr)
        data = json.loads(filepath.read_text())

        assert data["tier"] == "smoke"
        assert data["scenario"] == "ws-opus-deepgram-elevenlabs"
        assert data["concurrency"] == 2
        assert data["total_calls"] == 2
        assert data["successful_calls"] == 1
        assert data["failed_calls"] == 1

        percs = data["percentiles"]
        assert percs["setup"]["p50"] == 100.0
        assert percs["setup"]["p95"] == 150.0
        assert percs["tts"]["p50"] == 200.0
        assert percs["asr"]["p99"] == 380.0

        assert len(data["results"]) == 2
        r0 = data["results"][0]
        assert r0["protocol"] == "websocket"
        assert r0["codec"] == "opus"
        assert r0["success"] is True
        r1 = data["results"][1]
        assert r1["success"] is False
        assert r1["error"] == "timeout"

    def test_export_empty_results(self, tmp_path: Path) -> None:
        tr = _make_tier_result(results=[])
        exporter = JsonExporter(output_dir=str(tmp_path))
        filepath = exporter.export_tier_result(tr)
        data = json.loads(filepath.read_text())
        assert data["results"] == []
        assert data["total_calls"] == 0

    def test_multiple_exports(self, tmp_path: Path) -> None:
        exporter = JsonExporter(output_dir=str(tmp_path))
        tr1 = _make_tier_result()
        tr2 = TierResult(
            tier=Tier.LOAD,
            scenario="sip-pcmu-whisper-aws",
            concurrency=10,
            total_calls=5,
            successful_calls=5,
            failed_calls=0,
        )
        exporter.export_tier_result(tr1)
        exporter.export_tier_result(tr2)
        json_files = list(tmp_path.glob("*.json"))
        assert len(json_files) == 2
