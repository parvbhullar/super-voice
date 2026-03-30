from __future__ import annotations

from typing import Any

from rich.console import Console
from rich.panel import Panel
from rich.progress import (
    BarColumn,
    Progress,
    SpinnerColumn,
    TaskProgressColumn,
    TextColumn,
    TimeRemainingColumn,
)
from rich.table import Table

from active_call_tester.models import TierResult

console = Console()


def print_tier_header(tier: str, scenario: str, concurrency: int) -> None:
    """Print a header for a tier run."""
    console.print(
        Panel(
            f"[bold]{scenario}[/bold] | Tier: {tier} | Concurrency: {concurrency}",
            style="cyan",
        )
    )


def print_tier_result(
    tier_result: TierResult,
    threshold_checks: (dict[str, tuple[bool, float, float]] | None) = None,
) -> None:
    """Print a single tier result as a rich table."""
    table = Table(title=(f"{tier_result.scenario} - {tier_result.tier.value}"))
    table.add_column("Metric", style="cyan")
    table.add_column("p50", justify="right")
    table.add_column("p95", justify="right")
    table.add_column("p99", justify="right")
    table.add_column("Status", justify="center")

    def fmt(v: float) -> str:
        return f"{v:.1f}ms" if v > 0 else "-"

    def status(metric_key: str) -> str:
        if not threshold_checks or metric_key not in threshold_checks:
            return "[dim]N/A[/dim]"
        passed, _actual, threshold = threshold_checks[metric_key]
        if passed:
            return "[green]PASS[/green]"
        return f"[red]FAIL ({threshold:.0f}ms)[/red]"

    table.add_row(
        "Setup",
        fmt(tier_result.p50_setup_ms),
        fmt(tier_result.p95_setup_ms),
        fmt(tier_result.p99_setup_ms),
        status("call_setup_p95"),
    )
    table.add_row(
        "TTS First Byte",
        fmt(tier_result.p50_tts_ms),
        fmt(tier_result.p95_tts_ms),
        fmt(tier_result.p99_tts_ms),
        status("tts_first_byte_p95"),
    )
    table.add_row(
        "ASR First Result",
        fmt(tier_result.p50_asr_ms),
        fmt(tier_result.p95_asr_ms),
        fmt(tier_result.p99_asr_ms),
        status("asr_first_result_p95"),
    )

    console.print(table)
    console.print(
        f"  Calls: {tier_result.successful_calls}"
        f"/{tier_result.total_calls} succeeded,"
        f" {tier_result.failed_calls} failed"
    )
    # Show errors from failed calls
    failed = [r for r in tier_result.results if not r.success]
    if failed:
        errors: dict[str, int] = {}
        for r in failed:
            err = r.error or "unknown"
            errors[err] = errors.get(err, 0) + 1
        for err, count in errors.items():
            console.print(f"  [red]Error ({count}x): {err}[/red]")
    console.print()


def print_summary_table(
    results: list[tuple[TierResult, dict[str, tuple[bool, float, float]]]],
) -> None:
    """Print final summary table of all tier results."""
    table = Table(title="Test Summary")
    table.add_column("Scenario", style="bold")
    table.add_column("Tier")
    table.add_column("Calls", justify="right")
    table.add_column("Setup p95", justify="right")
    table.add_column("TTS p95", justify="right")
    table.add_column("ASR p95", justify="right")
    table.add_column("Status", justify="center")

    for tier_result, checks in results:
        all_passed = all(passed for passed, _, _ in checks.values()) if checks else True
        status_str = "[green]PASS[/green]" if all_passed else "[red]FAIL[/red]"

        table.add_row(
            tier_result.scenario,
            tier_result.tier.value,
            f"{tier_result.successful_calls}/{tier_result.total_calls}",
            f"{tier_result.p95_setup_ms:.1f}ms",
            f"{tier_result.p95_tts_ms:.1f}ms",
            f"{tier_result.p95_asr_ms:.1f}ms",
            status_str,
        )

    console.print(table)


def create_progress() -> Progress:
    """Create a rich progress bar for call execution."""
    return Progress(
        SpinnerColumn(),
        TextColumn("[bold blue]{task.description}"),
        BarColumn(),
        TaskProgressColumn(),
        TimeRemainingColumn(),
        console=console,
    )


def print_api_check_results(results: list[Any]) -> None:
    """Print API check results."""
    table = Table(title="API Endpoint Check")
    table.add_column("Method", style="cyan")
    table.add_column("Path")
    table.add_column("Status", justify="right")
    table.add_column("Latency", justify="right")
    table.add_column("Result", justify="center")

    for r in results:
        status_style = "green" if r.success else "red"
        error_str = (
            f"[red]{r.error or 'FAIL'}[/red]" if not r.success else "[green]OK[/green]"
        )
        table.add_row(
            r.method,
            r.path,
            f"[{status_style}]{r.status}[/{status_style}]",
            f"{r.latency_ms:.1f}ms",
            error_str,
        )

    console.print(table)
