from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any, Literal


@dataclass
class SessionState:
    """Per-call mutable state. One instance lives for the duration of a call."""

    session_id: str
    is_processing: bool = False
    idle_since: float | None = None
    idle_warning_count: int = 0
    shutdown: bool = False
    transcript: list[dict[str, Any]] = field(default_factory=list)
    started_at: float = field(default_factory=time.time)
    ended_at: float | None = None
    voice_profile_id: str | None = None

    def mark_processing(self) -> None:
        self.is_processing = True
        self.idle_since = None

    def mark_idle(self) -> None:
        self.is_processing = False
        self.idle_since = time.time()

    def append_transcript(
        self, role: Literal["user", "agent", "system"], text: str
    ) -> None:
        self.transcript.append({"role": role, "text": text})

    def end(self) -> None:
        self.shutdown = True
        self.ended_at = time.time()
