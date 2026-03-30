from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import TYPE_CHECKING

from active_call_tester.models import CallResult, TierResult

if TYPE_CHECKING:
    from opentelemetry.sdk.trace import TracerProvider
    from opentelemetry.trace import Tracer
    from prometheus_client import (
        CollectorRegistry,
        Counter,
        Gauge,
        Histogram,
    )


class PrometheusExporter:
    """Export metrics to Prometheus Pushgateway."""

    def __init__(
        self,
        pushgateway_url: str = "http://localhost:9091",
        job_name: str = "active_call_tester",
    ) -> None:
        self._pushgateway_url = pushgateway_url
        self._job_name = job_name
        self._initialized = False
        self._registry: CollectorRegistry | None = None
        self.call_setup_histogram: Histogram | None = None
        self.tts_first_byte_histogram: Histogram | None = None
        self.asr_first_result_histogram: Histogram | None = None
        self.call_errors_counter: Counter | None = None
        self.call_active_gauge: Gauge | None = None
        self.api_response_histogram: Histogram | None = None

    def _ensure_initialized(self) -> None:
        """Lazy init prometheus_client metrics."""
        if self._initialized:
            return
        from prometheus_client import (
            CollectorRegistry,
            Counter,
            Gauge,
            Histogram,
        )

        self._registry = CollectorRegistry()

        self.call_setup_histogram = Histogram(
            "active_call_setup_seconds",
            "Call setup latency",
            labelnames=[
                "protocol",
                "codec",
                "asr",
                "tts",
                "tier",
            ],
            buckets=[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
            registry=self._registry,
        )
        self.tts_first_byte_histogram = Histogram(
            "active_call_tts_first_byte_seconds",
            "TTS first byte latency",
            labelnames=["tts", "protocol"],
            buckets=[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
            registry=self._registry,
        )
        self.asr_first_result_histogram = Histogram(
            "active_call_asr_first_result_seconds",
            "ASR first result latency",
            labelnames=["asr", "protocol"],
            buckets=[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
            registry=self._registry,
        )
        self.call_errors_counter = Counter(
            "active_call_errors_total",
            "Total call errors",
            labelnames=["protocol", "error_type"],
            registry=self._registry,
        )
        self.call_active_gauge = Gauge(
            "active_call_concurrent",
            "Currently active test calls",
            labelnames=["protocol"],
            registry=self._registry,
        )
        self.api_response_histogram = Histogram(
            "active_call_api_response_seconds",
            "API response latency",
            labelnames=["method", "endpoint"],
            buckets=[
                0.01,
                0.05,
                0.1,
                0.25,
                0.5,
                1.0,
                2.5,
            ],
            registry=self._registry,
        )
        self._initialized = True

    def record_call_result(self, result: CallResult, tier: str) -> None:
        """Record a call result into Prometheus metrics."""
        self._ensure_initialized()
        assert self.call_setup_histogram is not None
        assert self.tts_first_byte_histogram is not None
        assert self.asr_first_result_histogram is not None
        assert self.call_errors_counter is not None

        labels = {
            "protocol": result.protocol.value,
            "codec": result.codec,
            "asr": result.asr_provider,
            "tts": result.tts_provider,
            "tier": tier,
        }

        if result.setup_latency_ms > 0:
            self.call_setup_histogram.labels(**labels).observe(
                result.setup_latency_ms / 1000
            )
        if result.first_tts_byte_ms > 0:
            self.tts_first_byte_histogram.labels(
                tts=result.tts_provider,
                protocol=result.protocol.value,
            ).observe(result.first_tts_byte_ms / 1000)
        if result.first_asr_result_ms > 0:
            self.asr_first_result_histogram.labels(
                asr=result.asr_provider,
                protocol=result.protocol.value,
            ).observe(result.first_asr_result_ms / 1000)

        if not result.success:
            error_type = result.error or "unknown"
            self.call_errors_counter.labels(
                protocol=result.protocol.value,
                error_type=error_type,
            ).inc()

    def record_api_response(
        self, method: str, endpoint: str, latency_ms: float
    ) -> None:
        """Record API response latency."""
        self._ensure_initialized()
        assert self.api_response_histogram is not None
        self.api_response_histogram.labels(method=method, endpoint=endpoint).observe(
            latency_ms / 1000
        )

    async def push(self) -> None:
        """Push metrics to Pushgateway."""
        self._ensure_initialized()
        from prometheus_client import push_to_gateway

        assert self._registry is not None
        push_to_gateway(
            self._pushgateway_url,
            job=self._job_name,
            registry=self._registry,
        )


class OtelExporter:
    """Export traces to OpenTelemetry collector."""

    def __init__(
        self,
        endpoint: str = "http://localhost:4317",
        service_name: str = "active-call-tester",
    ) -> None:
        self._endpoint = endpoint
        self._service_name = service_name
        self._tracer: Tracer | None = None
        self._provider: TracerProvider | None = None

    def _ensure_initialized(self) -> None:
        """Lazy init OpenTelemetry tracer."""
        if self._tracer is not None:
            return
        from opentelemetry import trace
        from opentelemetry.exporter.otlp.proto.grpc.trace_exporter import (
            OTLPSpanExporter,
        )
        from opentelemetry.sdk.resources import Resource
        from opentelemetry.sdk.trace import TracerProvider
        from opentelemetry.sdk.trace.export import (
            BatchSpanProcessor,
        )

        resource = Resource.create({"service.name": self._service_name})
        provider = TracerProvider(resource=resource)
        exporter = OTLPSpanExporter(endpoint=self._endpoint)
        provider.add_span_processor(BatchSpanProcessor(exporter))
        trace.set_tracer_provider(provider)
        self._tracer = trace.get_tracer(self._service_name)
        self._provider = provider

    def record_call_result(self, result: CallResult) -> None:
        """Create trace span for a call result."""
        self._ensure_initialized()
        assert self._tracer is not None
        with self._tracer.start_as_current_span(f"call.{result.scenario}") as span:
            span.set_attribute("protocol", result.protocol.value)
            span.set_attribute("codec", result.codec)
            span.set_attribute("asr", result.asr_provider)
            span.set_attribute("tts", result.tts_provider)
            span.set_attribute("success", result.success)
            span.set_attribute("total_duration_ms", result.total_duration_ms)

            if result.setup_latency_ms > 0:
                with self._tracer.start_span("call.setup") as child:
                    child.set_attribute(
                        "latency_ms",
                        result.setup_latency_ms,
                    )
            if result.first_tts_byte_ms > 0:
                with self._tracer.start_span("call.tts") as child:
                    child.set_attribute(
                        "latency_ms",
                        result.first_tts_byte_ms,
                    )
            if result.first_asr_result_ms > 0:
                with self._tracer.start_span("call.asr") as child:
                    child.set_attribute(
                        "latency_ms",
                        result.first_asr_result_ms,
                    )
            if result.teardown_ms > 0:
                with self._tracer.start_span("call.teardown") as child:
                    child.set_attribute(
                        "latency_ms",
                        result.teardown_ms,
                    )
            if result.error:
                span.set_attribute("error", result.error)

    async def shutdown(self) -> None:
        """Flush and shutdown the tracer provider."""
        if self._provider is not None:
            self._provider.shutdown()


class JsonExporter:
    """Export results to JSON files."""

    def __init__(self, output_dir: str = "results") -> None:
        self._output_dir = Path(output_dir)

    def export_tier_result(self, tier_result: TierResult) -> Path:
        """Export tier result to a JSON file."""
        self._output_dir.mkdir(parents=True, exist_ok=True)

        timestamp = datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
        filename = f"{timestamp}-{tier_result.tier.value}-{tier_result.scenario}.json"
        filepath = self._output_dir / filename

        data = {
            "tier": tier_result.tier.value,
            "scenario": tier_result.scenario,
            "concurrency": tier_result.concurrency,
            "total_calls": tier_result.total_calls,
            "successful_calls": tier_result.successful_calls,
            "failed_calls": tier_result.failed_calls,
            "percentiles": {
                "setup": {
                    "p50": tier_result.p50_setup_ms,
                    "p95": tier_result.p95_setup_ms,
                    "p99": tier_result.p99_setup_ms,
                },
                "tts": {
                    "p50": tier_result.p50_tts_ms,
                    "p95": tier_result.p95_tts_ms,
                    "p99": tier_result.p99_tts_ms,
                },
                "asr": {
                    "p50": tier_result.p50_asr_ms,
                    "p95": tier_result.p95_asr_ms,
                    "p99": tier_result.p99_asr_ms,
                },
            },
            "passed_thresholds": tier_result.passed_thresholds,
            "results": [
                {
                    "scenario": r.scenario,
                    "protocol": r.protocol.value,
                    "codec": r.codec,
                    "asr_provider": r.asr_provider,
                    "tts_provider": r.tts_provider,
                    "setup_latency_ms": r.setup_latency_ms,
                    "first_tts_byte_ms": r.first_tts_byte_ms,
                    "first_asr_result_ms": (r.first_asr_result_ms),
                    "teardown_ms": r.teardown_ms,
                    "total_duration_ms": r.total_duration_ms,
                    "success": r.success,
                    "error": r.error,
                }
                for r in tier_result.results
            ],
        }

        filepath.write_text(json.dumps(data, indent=2))
        return filepath
