from __future__ import annotations

import pytest

from active_call_tester.metrics.collector import (
    MetricsCollector,
    _percentile,
)
from active_call_tester.models import (
    CallResult,
    Protocol,
    Tier,
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


class TestMetricsCollectorRecord:
    """Tests for record and record_batch."""

    def test_record_single(self) -> None:
        collector = MetricsCollector()
        result = _make_result()
        collector.record(result)
        assert len(collector.results) == 1
        assert collector.results[0] is result

    def test_record_batch(self) -> None:
        collector = MetricsCollector()
        results = [_make_result() for _ in range(5)]
        collector.record_batch(results)
        assert len(collector.results) == 5

    def test_record_mixed(self) -> None:
        collector = MetricsCollector()
        collector.record(_make_result())
        collector.record_batch([_make_result(), _make_result()])
        assert len(collector.results) == 3

    def test_results_returns_copy(self) -> None:
        collector = MetricsCollector()
        collector.record(_make_result())
        copy = collector.results
        copy.clear()
        assert len(collector.results) == 1


class TestMetricsCollectorClear:
    """Tests for clear method."""

    def test_clear_results(self) -> None:
        collector = MetricsCollector()
        collector.record(_make_result())
        collector.compute_tier_result(Tier.SMOKE, "ws-opus-deepgram-elevenlabs", 1)
        collector.clear()
        assert len(collector.results) == 0
        assert len(collector._tier_results) == 0

    def test_clear_empty(self) -> None:
        collector = MetricsCollector()
        collector.clear()
        assert len(collector.results) == 0


class TestComputeTierResult:
    """Tests for compute_tier_result."""

    def test_no_matching_results(self) -> None:
        collector = MetricsCollector()
        collector.record(_make_result(scenario="other"))
        tr = collector.compute_tier_result(Tier.SMOKE, "nonexistent", 1)
        assert tr.total_calls == 0
        assert tr.successful_calls == 0
        assert tr.failed_calls == 0

    def test_matching_results(self) -> None:
        collector = MetricsCollector()
        scenario = "test-scenario"
        collector.record_batch([_make_result(scenario=scenario) for _ in range(3)])
        collector.record(
            _make_result(
                scenario=scenario,
                success=False,
                error="timeout",
            )
        )
        tr = collector.compute_tier_result(Tier.LOAD, scenario, 4)
        assert tr.total_calls == 4
        assert tr.successful_calls == 3
        assert tr.failed_calls == 1
        assert tr.tier == Tier.LOAD
        assert tr.scenario == scenario
        assert tr.concurrency == 4

    def test_percentiles_with_known_data(self) -> None:
        collector = MetricsCollector()
        scenario = "perc-test"
        # Create results with known setup latencies 10..100
        for i in range(1, 11):
            collector.record(
                _make_result(
                    scenario=scenario,
                    setup_ms=float(i * 10),
                )
            )
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 10)
        assert tr.p50_setup_ms == pytest.approx(55.0)
        assert tr.p95_setup_ms == pytest.approx(95.5)
        assert tr.p99_setup_ms == pytest.approx(99.1)

    def test_zero_latency_excluded(self) -> None:
        """Results with 0 latency are excluded from percentiles."""
        collector = MetricsCollector()
        scenario = "zero-test"
        collector.record(_make_result(scenario=scenario, setup_ms=0.0, tts_ms=0.0))
        collector.record(_make_result(scenario=scenario, setup_ms=100.0))
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 2)
        assert tr.p50_setup_ms == 100.0
        assert tr.total_calls == 2

    def test_tier_results_accumulated(self) -> None:
        collector = MetricsCollector()
        collector.record(_make_result(scenario="s1"))
        collector.compute_tier_result(Tier.SMOKE, "s1", 1)
        collector.compute_tier_result(Tier.LOAD, "s1", 1)
        assert len(collector._tier_results) == 2


class TestPercentile:
    """Tests for the _percentile helper."""

    def test_empty_list(self) -> None:
        assert _percentile([], 50) == 0.0

    def test_single_element(self) -> None:
        assert _percentile([42.0], 50) == 42.0
        assert _percentile([42.0], 99) == 42.0

    def test_two_elements(self) -> None:
        assert _percentile([10.0, 20.0], 50) == 15.0

    def test_sorted_input(self) -> None:
        data = [1.0, 2.0, 3.0, 4.0, 5.0]
        assert _percentile(data, 0) == 1.0
        assert _percentile(data, 100) == 5.0

    def test_unsorted_input(self) -> None:
        data = [5.0, 1.0, 3.0, 2.0, 4.0]
        assert _percentile(data, 50) == 3.0


class TestCheckThresholds:
    """Tests for check_thresholds."""

    def test_all_pass(self) -> None:
        collector = MetricsCollector()
        scenario = "pass-test"
        collector.record(
            _make_result(
                scenario=scenario,
                setup_ms=100.0,
                tts_ms=50.0,
                asr_ms=200.0,
            )
        )
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 1)
        thresholds = {
            "call_setup_p95": 500.0,
            "tts_first_byte_p95": 300.0,
            "asr_first_result_p95": 800.0,
        }
        checks = collector.check_thresholds(tr, thresholds)
        for metric, (passed, actual, thresh) in checks.items():
            assert passed, f"{metric} failed: {actual} > {thresh}"

    def test_some_fail(self) -> None:
        collector = MetricsCollector()
        scenario = "fail-test"
        collector.record(
            _make_result(
                scenario=scenario,
                setup_ms=600.0,
                tts_ms=50.0,
                asr_ms=200.0,
            )
        )
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 1)
        thresholds = {
            "call_setup_p95": 500.0,
            "tts_first_byte_p95": 300.0,
        }
        checks = collector.check_thresholds(tr, thresholds)
        passed, actual, thresh = checks["call_setup_p95"]
        assert not passed
        assert actual == 600.0
        assert thresh == 500.0

    def test_zero_actual_passes(self) -> None:
        collector = MetricsCollector()
        scenario = "zero-actual"
        collector.record(
            _make_result(
                scenario=scenario,
                setup_ms=0.0,
                tts_ms=0.0,
                asr_ms=0.0,
            )
        )
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 1)
        thresholds = {"call_setup_p95": 500.0}
        checks = collector.check_thresholds(tr, thresholds)
        passed, _, _ = checks["call_setup_p95"]
        assert passed

    def test_missing_threshold_key_ignored(self) -> None:
        collector = MetricsCollector()
        scenario = "partial"
        collector.record(_make_result(scenario=scenario))
        tr = collector.compute_tier_result(Tier.SMOKE, scenario, 1)
        checks = collector.check_thresholds(tr, {"nonexistent_key": 100.0})
        assert len(checks) == 0
