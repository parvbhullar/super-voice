from __future__ import annotations

from typing import Protocol, runtime_checkable


@runtime_checkable
class TurnDetector(Protocol):
    """Hot-path turn detection — VAD + EOU semantics behind one interface.

    V1: Pipecat-backed Silero + SmartTurnAnalyzerV3 (pipecat_impl.py).
    V2: Rust+PyO3 echokit-style crate, swapped in without changing the pipeline.
    """

    async def is_speech(self, frame_pcm: bytes) -> bool:
        """True if the 20-30ms PCM frame contains speech."""
        ...

    async def is_turn_end(
        self, transcript_so_far: str, silence_ms: int
    ) -> bool:
        """True if the user has finished their turn (semantic, not just silence)."""
        ...
