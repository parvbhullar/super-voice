from __future__ import annotations

import os
from unittest.mock import patch

from click.testing import CliRunner

from active_call_tester.cli import main
from active_call_tester.config import (
    ProvidersConfig,
    ProviderConfig,
    ScenarioConfig,
    TargetConfig,
    TestMatrixConfig,
    TierConfig,
)
from active_call_tester.metrics.collector import (
    MetricsCollector,
)
from active_call_tester.runner import (
    CallScheduler,
    TestRunner,
)


def _make_config() -> TestMatrixConfig:
    """Create a minimal TestMatrixConfig for testing."""
    return TestMatrixConfig(
        target=TargetConfig(
            url="http://localhost:8080",
            ws_url="ws://localhost:8080/call",
        ),
        tiers={
            "smoke": TierConfig(
                concurrency=[1],
                duration_secs=10,
                call_hold_secs=2,
            ),
        },
        protocols=["websocket"],
        codecs=["pcmu"],
        providers=ProvidersConfig(
            asr=[
                ProviderConfig(name="sensevoice", type="offline"),
            ],
            tts=[
                ProviderConfig(name="supertonic", type="offline"),
            ],
        ),
        matrix=[
            ScenarioConfig(
                name="test-ws",
                protocol="websocket",
                codec="pcmu",
                asr="sensevoice",
                tts="supertonic",
                tiers=["smoke"],
            ),
        ],
    )


class TestCallScheduler:
    def test_init(self) -> None:
        """CallScheduler initializes correctly."""
        config = _make_config()
        scenario = config.matrix[0]
        tier_config = config.tiers["smoke"]
        collector = MetricsCollector()

        scheduler = CallScheduler(
            config=config,
            scenario=scenario,
            tier_config=tier_config,
            tier_name="smoke",
            collector=collector,
        )

        assert scheduler._config is config
        assert scheduler._scenario is scenario
        assert scheduler._tier_config is tier_config
        assert scheduler._tier_name == "smoke"
        assert scheduler._shutdown is False

    def test_request_shutdown(self) -> None:
        """request_shutdown sets the flag."""
        config = _make_config()
        scenario = config.matrix[0]
        tier_config = config.tiers["smoke"]
        collector = MetricsCollector()

        scheduler = CallScheduler(
            config=config,
            scenario=scenario,
            tier_config=tier_config,
            tier_name="smoke",
            collector=collector,
        )

        assert scheduler._shutdown is False
        scheduler.request_shutdown()
        assert scheduler._shutdown is True


class TestTestRunner:
    def test_init(self) -> None:
        """TestRunner initializes with config."""
        config = _make_config()
        runner = TestRunner(config)

        assert runner._config is config
        assert runner._all_results == []
        assert runner._any_failed is False

    def test_has_failures_default(self) -> None:
        """has_failures starts False."""
        config = _make_config()
        runner = TestRunner(config)
        assert runner.has_failures is False


class TestCLI:
    def test_main_help(self) -> None:
        """CLI --help returns 0."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["--help"])
        assert result.exit_code == 0
        assert "Active Call Tester" in result.output

    def test_run_help(self) -> None:
        """run --help returns 0."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["run", "--help"])
        assert result.exit_code == 0
        assert "--tier" in result.output

    def test_api_check_help(self) -> None:
        """api-check --help returns 0."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["api-check", "--help"])
        assert result.exit_code == 0

    def test_list_help(self) -> None:
        """list --help returns 0."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["list", "--help"])
        assert result.exit_code == 0

    def test_grafana_dashboard_help(self) -> None:
        """grafana-dashboard --help returns 0."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["grafana-dashboard", "--help"])
        assert result.exit_code == 0

    def test_list_command(self) -> None:
        """list command with real config works."""
        cli_runner = CliRunner()
        with patch.dict(os.environ, {"API_KEY": "test"}):
            result = cli_runner.invoke(
                main,
                [
                    "list",
                    "--config",
                    "config/test-matrix.yaml",
                ],
            )
        assert result.exit_code == 0

    def test_grafana_dashboard_command(self) -> None:
        """grafana-dashboard exports JSON."""
        cli_runner = CliRunner()
        with cli_runner.isolated_filesystem():
            result = cli_runner.invoke(
                main,
                [
                    "grafana-dashboard",
                    "--output",
                    "test-out/",
                ],
            )
            assert result.exit_code == 0
            import json
            from pathlib import Path

            out = Path("test-out/active-call-tester.json")
            assert out.exists()
            data = json.loads(out.read_text())
            assert "dashboard" in data
            assert len(data["dashboard"]["panels"]) == 6

    def test_version(self) -> None:
        """--version returns version."""
        cli_runner = CliRunner()
        result = cli_runner.invoke(main, ["--version"])
        assert result.exit_code == 0
        assert "0.1.0" in result.output
