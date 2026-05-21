"""Pipecat-backed TurnDetector for supervoice (V1).

Wraps Pipecat's bundled Silero VAD + LocalSmartTurnAnalyzerV3. In v1, these
analyzers are passed directly to the Pipecat transport (which drives them
internally); the pipeline builder fetches them via ``.vad`` / ``.turn``.

The ``is_speech`` / ``is_turn_end`` Protocol methods are intentional
NotImplementedError stubs — they become relevant when the V2 Rust impl ships
and the seam needs to be driven manually.
"""

from __future__ import annotations

from pipecat.audio.turn.smart_turn.local_smart_turn_v3 import (
    LocalSmartTurnAnalyzerV3,
)
from pipecat.audio.vad.silero import SileroVADAnalyzer

from .protocol import TurnDetector


class PipecatTurnDetector:
    """V1 implementation: wraps Pipecat's bundled Silero VAD + SmartTurn EOU.

    These analyzers are normally passed directly to the transport. We keep
    references here so the pipeline-builder can fetch them via the protocol.
    """

    def __init__(self, vad_stop_secs: float = 0.2) -> None:
        self.vad = SileroVADAnalyzer()
        self.vad.params.stop_secs = vad_stop_secs
        self.turn = LocalSmartTurnAnalyzerV3()

    async def is_speech(self, frame_pcm: bytes) -> bool:
        """Stub — Pipecat transport drives VAD internally in v1."""
        raise NotImplementedError(
            "Pipecat v1 drives VAD inside transport; use .vad directly."
        )

    async def is_turn_end(
        self, transcript_so_far: str, silence_ms: int
    ) -> bool:
        """Stub — Pipecat transport drives EOU internally in v1."""
        raise NotImplementedError(
            "Pipecat v1 drives EOU inside transport; use .turn directly."
        )


if __name__ == "__main__":
    # Static check that our class satisfies the Protocol structurally.
    # Gated under __main__ so model-loading doesn't run on import.
    _: TurnDetector = PipecatTurnDetector()  # type: ignore[assignment]
