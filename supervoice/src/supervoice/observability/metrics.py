from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any


@dataclass
class CallMetrics:
    """Per-call latency metrics.

    Uses `time.monotonic()` for elapsed measurements so wall-clock drift
    (NTP corrections) doesn't skew durations.
    """

    session_id: str
    _user_turn_end_t: float | None = None
    _user_audio_end_t: float | None = None
    _asr_final_t: float | None = None
    _first_agent_audio_t: float | None = None
    extras: dict[str, Any] = field(default_factory=dict)

    def mark_user_audio_end(self) -> None:
        self._user_audio_end_t = time.monotonic()

    def mark_asr_final(self) -> None:
        self._asr_final_t = time.monotonic()

    def mark_user_turn_end(self) -> None:
        self._user_turn_end_t = time.monotonic()

    def mark_first_agent_audio(self) -> None:
        self._first_agent_audio_t = time.monotonic()

    @property
    def ttfa_ms(self) -> float | None:
        if self._user_turn_end_t is None or self._first_agent_audio_t is None:
            return None
        return (self._first_agent_audio_t - self._user_turn_end_t) * 1000.0

    @property
    def asr_final_ms(self) -> float | None:
        if self._user_audio_end_t is None or self._asr_final_t is None:
            return None
        return (self._asr_final_t - self._user_audio_end_t) * 1000.0

    def snapshot(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "ttfa_ms": self.ttfa_ms,
            "asr_final_ms": self.asr_final_ms,
            **self.extras,
        }
