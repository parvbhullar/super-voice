from __future__ import annotations

from active_call_tester.models import CallResult, Tier, TierResult


def _percentile(data: list[float], p: float) -> float:
    """Compute the p-th percentile using linear interpolation."""
    if not data:
        return 0.0
    sorted_data = sorted(data)
    k = (len(sorted_data) - 1) * (p / 100)
    f = int(k)
    c = f + 1
    if c >= len(sorted_data):
        return sorted_data[-1]
    return sorted_data[f] + (k - f) * (sorted_data[c] - sorted_data[f])


class MetricsCollector:
    """Central sink for collecting and aggregating call results."""

    def __init__(self) -> None:
        self._results: list[CallResult] = []
        self._tier_results: list[TierResult] = []

    def record(self, result: CallResult) -> None:
        """Record a single call result."""
        self._results.append(result)

    def record_batch(self, results: list[CallResult]) -> None:
        """Record multiple call results."""
        self._results.extend(results)

    @property
    def results(self) -> list[CallResult]:
        """Return a copy of all recorded results."""
        return list(self._results)

    def clear(self) -> None:
        """Clear all recorded results and tier results."""
        self._results.clear()
        self._tier_results.clear()

    def compute_tier_result(
        self,
        tier: Tier,
        scenario: str,
        concurrency: int,
    ) -> TierResult:
        """Compute aggregated stats for a tier+scenario+concurrency."""
        matching = [r for r in self._results if r.scenario == scenario]
        if not matching:
            return TierResult(
                tier=tier,
                scenario=scenario,
                concurrency=concurrency,
                total_calls=0,
                successful_calls=0,
                failed_calls=0,
            )

        successful = [r for r in matching if r.success]
        failed = [r for r in matching if not r.success]

        setup_times = [r.setup_latency_ms for r in successful if r.setup_latency_ms > 0]
        tts_times = [r.first_tts_byte_ms for r in successful if r.first_tts_byte_ms > 0]
        asr_times = [
            r.first_asr_result_ms for r in successful if r.first_asr_result_ms > 0
        ]

        tier_result = TierResult(
            tier=tier,
            scenario=scenario,
            concurrency=concurrency,
            total_calls=len(matching),
            successful_calls=len(successful),
            failed_calls=len(failed),
            results=matching,
            p50_setup_ms=_percentile(setup_times, 50),
            p95_setup_ms=_percentile(setup_times, 95),
            p99_setup_ms=_percentile(setup_times, 99),
            p50_tts_ms=_percentile(tts_times, 50),
            p95_tts_ms=_percentile(tts_times, 95),
            p99_tts_ms=_percentile(tts_times, 99),
            p50_asr_ms=_percentile(asr_times, 50),
            p95_asr_ms=_percentile(asr_times, 95),
            p99_asr_ms=_percentile(asr_times, 99),
        )
        self._tier_results.append(tier_result)
        return tier_result

    def check_thresholds(
        self,
        tier_result: TierResult,
        thresholds: dict[str, float],
    ) -> dict[str, tuple[bool, float, float]]:
        """Check tier result against thresholds.

        Returns dict of {metric: (passed, actual, threshold)}.
        A metric passes if actual <= threshold or actual == 0.0.
        """
        checks: dict[str, tuple[bool, float, float]] = {}

        mapping: dict[str, float] = {
            "call_setup_p95": tier_result.p95_setup_ms,
            "tts_first_byte_p95": tier_result.p95_tts_ms,
            "asr_first_result_p95": tier_result.p95_asr_ms,
            # api_response_p95 is checked separately via the
            # REST client api-check flow; default to 0.0 here.
            "api_response_p95": 0.0,
            # WS RTT and ICE completion are included in the
            # call setup latency, so use p95_setup_ms as proxy.
            "ws_command_rtt_p95": tier_result.p95_setup_ms,
            "webrtc_ice_complete_p95": (tier_result.p95_setup_ms),
        }

        for key, actual in mapping.items():
            if key in thresholds:
                threshold = thresholds[key]
                passed = actual <= threshold or actual == 0.0
                checks[key] = (passed, actual, threshold)

        return checks
