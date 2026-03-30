from __future__ import annotations

import asyncio
import signal
import uuid
from typing import Any

from active_call_tester.clients.rest import RestClient
from active_call_tester.clients.sip import (
    SipCallConfig,
    SipClient,
)
from active_call_tester.clients.webrtc import (
    AiortcWebRtcClient,
    PlaywrightWebRtcClient,
    WebRtcCallConfig,
)
from active_call_tester.clients.ws import (
    WsCallConfig,
    WsClient,
)
from active_call_tester.config import (
    ScenarioConfig,
    TestMatrixConfig,
    TierConfig,
)
from active_call_tester.metrics.collector import (
    MetricsCollector,
)
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
from active_call_tester.reports.console import (
    console,
    print_api_check_results,
    print_summary_table,
    print_tier_header,
    print_tier_result,
)


class CallScheduler:
    """Manages concurrent call workers for a scenario."""

    def __init__(
        self,
        config: TestMatrixConfig,
        scenario: ScenarioConfig,
        tier_config: TierConfig,
        tier_name: str,
        collector: MetricsCollector,
    ) -> None:
        self._config = config
        self._scenario = scenario
        self._tier_config = tier_config
        self._tier_name = tier_name
        self._collector = collector
        self._active_calls: set[asyncio.Task[Any]] = set()
        self._shutdown = False

    async def _create_call_worker(self, concurrency_idx: int) -> CallResult:
        """Execute a single call based on protocol."""
        protocol = self._scenario.protocol
        session_id = f"test-{uuid.uuid4().hex[:8]}"

        callee = self._scenario.callee or "sip:test@localhost:5060"
        caller = self._scenario.caller or ""

        if protocol == "websocket":
            client = WsClient(self._config.target.ws_url)
            call_config = WsCallConfig(
                scenario=self._scenario.name,
                codec=self._scenario.codec,
                asr_provider=self._scenario.asr,
                tts_provider=self._scenario.tts,
                callee=callee,
                call_hold_secs=self._tier_config.call_hold_secs,
                session_id=session_id,
            )
            await client.connect(call_config)
            try:
                return await client.execute_call(call_config)
            finally:
                await client.disconnect()

        elif protocol == "sip":
            sip_client = SipClient(self._config.target.url)
            sip_config = SipCallConfig(
                scenario=self._scenario.name,
                codec=self._scenario.codec,
                asr_provider=self._scenario.asr,
                tts_provider=self._scenario.tts,
                callee=callee,
                caller=caller or "sip:tester@localhost",
                call_hold_secs=self._tier_config.call_hold_secs,
                session_id=session_id,
            )
            await sip_client.connect(sip_config)
            try:
                return await sip_client.execute_call(sip_config)
            finally:
                await sip_client.disconnect()

        elif protocol == "webrtc_aiortc":
            rtc_client = AiortcWebRtcClient(self._config.target.url)
            rtc_config = WebRtcCallConfig(
                scenario=self._scenario.name,
                codec=self._scenario.codec,
                asr_provider=self._scenario.asr,
                tts_provider=self._scenario.tts,
                call_hold_secs=self._tier_config.call_hold_secs,
                session_id=session_id,
                mode="aiortc",
            )
            await rtc_client.connect(rtc_config)
            try:
                return await rtc_client.execute_call(rtc_config)
            finally:
                await rtc_client.disconnect()

        elif protocol == "webrtc_browser":
            pw_client = PlaywrightWebRtcClient(self._config.target.url)
            pw_config = WebRtcCallConfig(
                scenario=self._scenario.name,
                codec=self._scenario.codec,
                asr_provider=self._scenario.asr,
                tts_provider=self._scenario.tts,
                call_hold_secs=self._tier_config.call_hold_secs,
                session_id=session_id,
                mode="browser",
            )
            await pw_client.connect(pw_config)
            try:
                return await pw_client.execute_call(pw_config)
            finally:
                await pw_client.disconnect()

        else:
            return CallResult(
                scenario=self._scenario.name,
                protocol=Protocol.WEBSOCKET,
                codec=self._scenario.codec,
                asr_provider=self._scenario.asr,
                tts_provider=self._scenario.tts,
                setup_latency_ms=0,
                first_tts_byte_ms=0,
                first_asr_result_ms=0,
                teardown_ms=0,
                total_duration_ms=0,
                success=False,
                error=f"unknown_protocol_{protocol}",
            )

    async def run_at_concurrency(self, concurrency: int) -> list[CallResult]:
        """Run calls at a given concurrency level."""
        semaphore = asyncio.Semaphore(concurrency)
        results: list[CallResult] = []
        duration = self._tier_config.duration_secs
        ramp_up = max(1, int(duration * 0.1))

        start_time = asyncio.get_event_loop().time()
        call_index = 0

        async def worker(idx: int) -> None:
            async with semaphore:
                if self._shutdown:
                    return
                try:
                    result = await self._create_call_worker(idx)
                    results.append(result)
                    self._collector.record(result)
                except Exception as e:
                    result = CallResult(
                        scenario=self._scenario.name,
                        protocol=Protocol.WEBSOCKET,
                        codec=self._scenario.codec,
                        asr_provider=self._scenario.asr,
                        tts_provider=self._scenario.tts,
                        setup_latency_ms=0,
                        first_tts_byte_ms=0,
                        first_asr_result_ms=0,
                        teardown_ms=0,
                        total_duration_ms=0,
                        success=False,
                        error=str(e),
                    )
                    results.append(result)
                    self._collector.record(result)

        # Spawn workers over the duration
        tasks: list[asyncio.Task[None]] = []
        elapsed = 0.0
        while elapsed < duration and not self._shutdown:
            # Calculate target concurrency with ramp-up
            if elapsed < ramp_up:
                target = max(
                    1,
                    int(concurrency * (elapsed / ramp_up)),
                )
            else:
                target = concurrency

            # Spawn new workers to replace completed ones
            active = [t for t in tasks if not t.done()]
            needed = target - len(active)
            for _ in range(max(0, needed)):
                task = asyncio.create_task(worker(call_index))
                tasks.append(task)
                call_index += 1

            await asyncio.sleep(0.5)
            elapsed = asyncio.get_event_loop().time() - start_time

        # Wait for all in-flight calls to finish
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)

        return results

    def request_shutdown(self) -> None:
        """Signal the scheduler to stop spawning calls."""
        self._shutdown = True


class TestRunner:
    """Main test runner that orchestrates all scenarios."""

    def __init__(self, config: TestMatrixConfig) -> None:
        self._config = config
        self._collector = MetricsCollector()
        self._prometheus = PrometheusExporter(
            pushgateway_url=(config.metrics.prometheus.pushgateway_url),
            job_name=config.metrics.prometheus.job_name,
        )
        self._otel = OtelExporter(
            endpoint=(config.metrics.opentelemetry.endpoint),
            service_name=(config.metrics.opentelemetry.service_name),
        )
        self._json_exporter = JsonExporter()
        self._all_results: list[
            tuple[
                TierResult,
                dict[str, tuple[bool, float, float]],
            ]
        ] = []
        self._any_failed = False

    async def run_scenario(
        self,
        scenario: ScenarioConfig,
        tier_name: str,
    ) -> list[TierResult]:
        """Run a scenario at all concurrency levels."""
        tier_config = self._config.tiers[tier_name]
        tier_enum = Tier(tier_name)
        tier_results: list[TierResult] = []

        for concurrency in tier_config.concurrency:
            print_tier_header(tier_name, scenario.name, concurrency)

            self._collector.clear()
            scheduler = CallScheduler(
                config=self._config,
                scenario=scenario,
                tier_config=tier_config,
                tier_name=tier_name,
                collector=self._collector,
            )

            # Set up SIGINT handler
            original_handler = signal.getsignal(signal.SIGINT)

            def handle_sigint(sig: int, frame: Any) -> None:
                console.print("[yellow]Shutting down gracefully...[/yellow]")
                scheduler.request_shutdown()

            signal.signal(signal.SIGINT, handle_sigint)

            try:
                await scheduler.run_at_concurrency(concurrency)
            finally:
                signal.signal(signal.SIGINT, original_handler)

            # Compute tier result
            tier_result = self._collector.compute_tier_result(
                tier=tier_enum,
                scenario=scenario.name,
                concurrency=concurrency,
            )

            # Check thresholds
            thresholds: dict[str, float] = {
                "call_setup_p95": (self._config.thresholds.call_setup_p95),
                "tts_first_byte_p95": (self._config.thresholds.tts_first_byte_p95),
                "asr_first_result_p95": (self._config.thresholds.asr_first_result_p95),
            }
            checks = self._collector.check_thresholds(tier_result, thresholds)

            if any(not passed for passed, _, _ in checks.values()):
                tier_result.passed_thresholds = False
                self._any_failed = True

            print_tier_result(tier_result, checks)

            # Export
            for result in self._collector.results:
                self._prometheus.record_call_result(result, tier_name)
                self._otel.record_call_result(result)
            self._json_exporter.export_tier_result(tier_result)

            self._all_results.append((tier_result, checks))
            tier_results.append(tier_result)

            # Cooldown
            console.print("[dim]Cooldown 5s...[/dim]")
            await asyncio.sleep(5)

        return tier_results

    async def run_tier(self, tier_name: str) -> None:
        """Run all available scenarios for a tier."""
        scenarios = self._config.get_available_scenarios(tier_name)
        if not scenarios:
            console.print(
                f"[yellow]No available scenarios for tier '{tier_name}'[/yellow]"
            )
            return

        console.print(
            f"\n[bold]Running {len(scenarios)}"
            f" scenarios for tier"
            f" '{tier_name}'[/bold]\n"
        )
        for scenario in scenarios:
            await self.run_scenario(scenario, tier_name)

    async def run_scenario_by_name(self, scenario_name: str, tier_name: str) -> None:
        """Run a specific scenario by name."""
        scenarios = [s for s in self._config.matrix if s.name == scenario_name]
        if not scenarios:
            console.print(f"[red]Scenario '{scenario_name}' not found[/red]")
            return
        await self.run_scenario(scenarios[0], tier_name)

    async def run_all(self) -> None:
        """Run all tiers for all scenarios."""
        for tier_name in self._config.tiers:
            await self.run_tier(tier_name)

    async def run_api_check(self) -> bool:
        """Run REST API endpoint validation."""
        console.print("\n[bold]Running API Endpoint Check[/bold]\n")
        client = RestClient(
            base_url=self._config.target.url,
            api_key=self._config.target.api_key,
        )
        await client.connect()
        try:
            await client.run_full_api_check()
            results = client.results
            print_api_check_results(results)

            for r in results:
                self._prometheus.record_api_response(r.method, r.path, r.latency_ms)

            failed = [r for r in results if not r.success]
            if failed:
                console.print(f"[red]{len(failed)} API checks failed[/red]")
                return False
            console.print(f"[green]All {len(results)} API checks passed[/green]")
            return True
        finally:
            await client.disconnect()

    def print_summary(self) -> None:
        """Print final summary."""
        if self._all_results:
            console.print()
            print_summary_table(self._all_results)

    async def push_metrics(self) -> None:
        """Push metrics to external systems."""
        try:
            await self._prometheus.push()
            console.print("[green]Pushed metrics to Prometheus Pushgateway[/green]")
        except Exception as e:
            console.print(f"[yellow]Prometheus push failed: {e}[/yellow]")

        try:
            await self._otel.shutdown()
            console.print("[green]Flushed OpenTelemetry traces[/green]")
        except Exception as e:
            console.print(f"[yellow]OTel flush failed: {e}[/yellow]")

    @property
    def has_failures(self) -> bool:
        """Whether any threshold checks failed."""
        return self._any_failed
