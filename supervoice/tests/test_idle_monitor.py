import asyncio

import pytest

from supervoice.session.idle_monitor import IdleMonitor
from supervoice.session.state import SessionState


@pytest.mark.asyncio
async def test_idle_monitor_warns_then_disconnects():
    state = SessionState(session_id="x")
    state.mark_idle()
    warnings: list[int] = []
    disconnects: list[bool] = []

    monitor = IdleMonitor(
        state=state,
        warning_at_s=0.1,
        disconnect_at_s=0.2,
        on_warning=lambda lvl: warnings.append(lvl),
        on_disconnect=lambda: disconnects.append(True),
        poll_interval_s=0.02,
    )
    task = asyncio.create_task(monitor.run())
    await asyncio.sleep(0.35)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass

    assert len(warnings) >= 1
    assert disconnects == [True]


@pytest.mark.asyncio
async def test_idle_monitor_skips_when_processing():
    state = SessionState(session_id="x")
    state.mark_processing()  # never goes idle
    disconnects: list[bool] = []

    monitor = IdleMonitor(
        state=state,
        warning_at_s=0.05,
        disconnect_at_s=0.1,
        on_warning=lambda _: None,
        on_disconnect=lambda: disconnects.append(True),
        poll_interval_s=0.02,
    )
    task = asyncio.create_task(monitor.run())
    await asyncio.sleep(0.2)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    assert disconnects == []
