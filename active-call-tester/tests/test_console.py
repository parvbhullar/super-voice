from __future__ import annotations

from rich.progress import Progress

from active_call_tester.clients.rest import ApiResponse
from active_call_tester.models import Tier, TierResult
from active_call_tester.reports.console import (
    create_progress,
    print_api_check_results,
    print_summary_table,
    print_tier_result,
)


def _make_tier_result() -> TierResult:
    """Create a sample TierResult for testing."""
    return TierResult(
        tier=Tier.SMOKE,
        scenario="test-scenario",
        concurrency=5,
        total_calls=10,
        successful_calls=8,
        failed_calls=2,
        p50_setup_ms=100.0,
        p95_setup_ms=200.0,
        p99_setup_ms=300.0,
        p50_tts_ms=50.0,
        p95_tts_ms=100.0,
        p99_tts_ms=150.0,
        p50_asr_ms=200.0,
        p95_asr_ms=400.0,
        p99_asr_ms=600.0,
    )


def _make_threshold_checks() -> dict[str, tuple[bool, float, float]]:
    """Create sample threshold checks."""
    return {
        "call_setup_p95": (True, 200.0, 500.0),
        "tts_first_byte_p95": (True, 100.0, 300.0),
        "asr_first_result_p95": (
            False,
            400.0,
            300.0,
        ),
    }


class TestPrintTierResult:
    def test_no_crash_with_checks(self) -> None:
        """print_tier_result does not crash."""
        tier_result = _make_tier_result()
        checks = _make_threshold_checks()
        print_tier_result(tier_result, checks)

    def test_no_crash_without_checks(self) -> None:
        """print_tier_result works without checks."""
        tier_result = _make_tier_result()
        print_tier_result(tier_result, None)

    def test_no_crash_empty_checks(self) -> None:
        """print_tier_result works with empty checks."""
        tier_result = _make_tier_result()
        print_tier_result(tier_result, {})

    def test_zero_values(self) -> None:
        """print_tier_result handles zero latencies."""
        tier_result = TierResult(
            tier=Tier.LOAD,
            scenario="zero-test",
            concurrency=1,
            total_calls=0,
            successful_calls=0,
            failed_calls=0,
        )
        print_tier_result(tier_result)


class TestPrintSummaryTable:
    def test_no_crash(self) -> None:
        """print_summary_table does not crash."""
        tier_result = _make_tier_result()
        checks = _make_threshold_checks()
        print_summary_table([(tier_result, checks)])

    def test_empty_results(self) -> None:
        """print_summary_table handles empty list."""
        print_summary_table([])

    def test_multiple_results(self) -> None:
        """print_summary_table with multiple entries."""
        tr1 = _make_tier_result()
        tr2 = TierResult(
            tier=Tier.LOAD,
            scenario="another",
            concurrency=10,
            total_calls=50,
            successful_calls=50,
            failed_calls=0,
            p95_setup_ms=150.0,
            p95_tts_ms=80.0,
            p95_asr_ms=350.0,
        )
        checks_pass: dict[str, tuple[bool, float, float]] = {
            "call_setup_p95": (True, 150.0, 500.0),
            "tts_first_byte_p95": (True, 80.0, 300.0),
            "asr_first_result_p95": (
                True,
                350.0,
                800.0,
            ),
        }
        print_summary_table(
            [
                (tr1, _make_threshold_checks()),
                (tr2, checks_pass),
            ]
        )

    def test_all_passing(self) -> None:
        """All-passing results show PASS status."""
        tr = _make_tier_result()
        checks: dict[str, tuple[bool, float, float]] = {
            "call_setup_p95": (True, 200.0, 500.0),
        }
        print_summary_table([(tr, checks)])


class TestPrintApiCheckResults:
    def test_no_crash(self) -> None:
        """print_api_check_results does not crash."""
        results = [
            ApiResponse(
                method="GET",
                path="/api/v1/health",
                status=200,
                latency_ms=15.3,
            ),
            ApiResponse(
                method="POST",
                path="/api/v1/endpoints",
                status=500,
                latency_ms=120.5,
                error="Internal Server Error",
            ),
        ]
        print_api_check_results(results)

    def test_empty_results(self) -> None:
        """print_api_check_results handles empty."""
        print_api_check_results([])

    def test_all_success(self) -> None:
        """print_api_check_results all success."""
        results = [
            ApiResponse(
                method="GET",
                path="/api/v1/health",
                status=200,
                latency_ms=10.0,
            ),
            ApiResponse(
                method="GET",
                path="/api/v1/info",
                status=201,
                latency_ms=20.0,
            ),
        ]
        print_api_check_results(results)


class TestCreateProgress:
    def test_returns_progress(self) -> None:
        """create_progress returns Progress instance."""
        progress = create_progress()
        assert isinstance(progress, Progress)
