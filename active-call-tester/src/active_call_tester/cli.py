from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

import click
from rich.table import Table

from active_call_tester import __version__
from active_call_tester.config import load_config
from active_call_tester.reports.console import console
from active_call_tester.runner import TestRunner


@click.group()
@click.version_option(version=__version__)
def main() -> None:
    """Active Call Tester -- Bulk test Active Call."""
    pass


@main.command()
@click.option(
    "--tier",
    type=click.Choice(["smoke", "load", "stress"]),
    help="Tier to run",
)
@click.option(
    "--scenario",
    help="Specific scenario name to run",
)
@click.option(
    "--config",
    "config_path",
    default="config/test-matrix.yaml",
    help="Path to test matrix YAML",
)
@click.option(
    "--all",
    "run_all",
    is_flag=True,
    help="Run all tiers",
)
def run(
    tier: str | None,
    scenario: str | None,
    config_path: str,
    run_all: bool,
) -> None:
    """Run test scenarios."""
    config = load_config(config_path)
    runner = TestRunner(config)

    async def _run() -> None:
        if run_all:
            await runner.run_all()
        elif scenario and tier:
            await runner.run_scenario_by_name(scenario, tier)
        elif tier:
            await runner.run_tier(tier)
        else:
            console.print("[red]Specify --tier, --scenario + --tier, or --all[/red]")
            return

        runner.print_summary()
        await runner.push_metrics()

    asyncio.run(_run())

    if runner.has_failures:
        sys.exit(1)


@main.command("api-check")
@click.option(
    "--config",
    "config_path",
    default="config/test-matrix.yaml",
    help="Path to test matrix YAML",
)
def api_check(config_path: str) -> None:
    """Validate all REST API endpoints."""
    config = load_config(config_path)
    runner = TestRunner(config)

    async def _run() -> bool:
        result = await runner.run_api_check()
        await runner.push_metrics()
        return result

    success = asyncio.run(_run())
    if not success:
        sys.exit(1)


@main.command("list")
@click.option(
    "--config",
    "config_path",
    default="config/test-matrix.yaml",
    help="Path to test matrix YAML",
)
def list_scenarios(config_path: str) -> None:
    """List available test scenarios."""
    config = load_config(config_path)

    table = Table(title="Test Scenarios")
    table.add_column("Name", style="bold")
    table.add_column("Protocol")
    table.add_column("Codec")
    table.add_column("ASR")
    table.add_column("TTS")
    table.add_column("Tiers")
    table.add_column("Available", justify="center")

    for scenario in config.matrix:
        available_scenarios = (
            config.get_available_scenarios(scenario.tiers[0]) if scenario.tiers else []
        )
        is_available = any(s.name == scenario.name for s in available_scenarios)

        table.add_row(
            scenario.name,
            scenario.protocol,
            scenario.codec,
            scenario.asr,
            scenario.tts,
            ", ".join(scenario.tiers),
            "[green]Yes[/green]" if is_available else "[red]No[/red]",
        )

    console.print(table)


@main.command("grafana-dashboard")
@click.option(
    "--output",
    default="dashboards/",
    help="Output directory",
)
def grafana_dashboard(output: str) -> None:
    """Export Grafana dashboard JSON."""
    dashboard = {
        "dashboard": {
            "title": "Active Call Tester",
            "panels": [
                {
                    "title": "Call Setup Latency",
                    "type": "timeseries",
                    "targets": [
                        {
                            "expr": (
                                "histogram_quantile("
                                "0.95, rate("
                                "active_call_setup"
                                "_seconds_bucket"
                                "[5m]))"
                            )
                        }
                    ],
                },
                {
                    "title": "TTS First Byte Latency",
                    "type": "timeseries",
                    "targets": [
                        {
                            "expr": (
                                "histogram_quantile("
                                "0.95, rate("
                                "active_call_tts"
                                "_first_byte"
                                "_seconds_bucket"
                                "[5m]))"
                            )
                        }
                    ],
                },
                {
                    "title": "ASR First Result Latency",
                    "type": "timeseries",
                    "targets": [
                        {
                            "expr": (
                                "histogram_quantile("
                                "0.95, rate("
                                "active_call_asr"
                                "_first_result"
                                "_seconds_bucket"
                                "[5m]))"
                            )
                        }
                    ],
                },
                {
                    "title": "Error Rate",
                    "type": "stat",
                    "targets": [{"expr": ("rate(active_call_errors_total[5m])")}],
                },
                {
                    "title": "API Response Latency",
                    "type": "timeseries",
                    "targets": [
                        {
                            "expr": (
                                "histogram_quantile("
                                "0.95, rate("
                                "active_call_api"
                                "_response"
                                "_seconds_bucket"
                                "[5m]))"
                            )
                        }
                    ],
                },
                {
                    "title": "Concurrent Calls",
                    "type": "gauge",
                    "targets": [{"expr": ("active_call_concurrent")}],
                },
            ],
        },
    }

    out_path = Path(output)
    out_path.mkdir(parents=True, exist_ok=True)
    filepath = out_path / "active-call-tester.json"
    filepath.write_text(json.dumps(dashboard, indent=2))
    console.print(f"[green]Dashboard exported to {filepath}[/green]")
