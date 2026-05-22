from __future__ import annotations

import asyncio
import time
from typing import Callable

from .state import SessionState


class IdleMonitor:
    """Tracks idle time on a SessionState; fires warning + disconnect callbacks.

    Skips checks while the session is processing — `is_processing=True` means
    something is actively being computed (LLM call, TTS synthesis, etc.) and
    idle timeout should not fire.

    The monitor exits cleanly when:
      - `disconnect_at_s` elapses (calls on_disconnect, returns)
      - the SessionState.shutdown flag is True
      - the task is cancelled
    """

    def __init__(
        self,
        state: SessionState,
        warning_at_s: float,
        disconnect_at_s: float,
        on_warning: Callable[[int], None],
        on_disconnect: Callable[[], None],
        poll_interval_s: float = 1.0,
    ) -> None:
        self._state = state
        self._warn_at = warning_at_s
        self._disconnect_at = disconnect_at_s
        self._on_warning = on_warning
        self._on_disconnect = on_disconnect
        self._poll = poll_interval_s

    async def run(self) -> None:
        while not self._state.shutdown:
            if self._state.is_processing or self._state.idle_since is None:
                await asyncio.sleep(self._poll)
                continue
            elapsed = time.time() - self._state.idle_since
            if elapsed >= self._warn_at and self._state.idle_warning_count == 0:
                self._state.idle_warning_count = 1
                self._on_warning(1)
            if elapsed >= self._disconnect_at:
                self._on_disconnect()
                return
            await asyncio.sleep(self._poll)
