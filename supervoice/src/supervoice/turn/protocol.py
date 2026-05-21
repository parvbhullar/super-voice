from __future__ import annotations

from typing import Protocol, runtime_checkable


# NOTE: @runtime_checkable performs structural-name-only checks; it does NOT
# validate parameter signatures, return types, or async-ness. Implementations
# satisfying isinstance() must still honor the documented contract below.
@runtime_checkable
class TurnDetector(Protocol):
    """Hot-path turn detection — VAD + EOU semantics behind one interface.

    The detector is stateful: callers feed audio frames into ``is_speech`` as
    they arrive; the implementation buffers whatever context (mel spectrograms,
    embeddings, etc.) it needs internally. ``is_turn_end`` then queries that
    accumulated audio context together with the textual hint.

    V1: Pipecat-backed Silero + SmartTurnAnalyzerV3 (pipecat_impl.py). In v1
        the analyzers are driven by the transport, so neither method is called
        directly — they exist purely as the swap-seam contract.
    V2: Rust+PyO3 echokit-style crate, swapped in without changing the pipeline.
    """

    async def is_speech(self, frame_pcm: bytes) -> bool:
        """Detect voice activity in one audio frame.

        Args:
            frame_pcm: Raw PCM samples — 16kHz mono, 16-bit signed little-endian,
                20-30ms per frame (320-480 samples = 640-960 bytes). The
                implementation must accept this exact format; resampling and
                framing are upstream responsibilities.

        Returns:
            True if the frame contains speech.
        """
        ...

    async def is_turn_end(
        self, transcript_so_far: str, silence_ms: int
    ) -> bool:
        """Decide whether the user has finished their turn.

        Uses any audio context the implementation has accumulated via prior
        ``is_speech`` calls, together with the supplied textual hint, to make a
        semantic end-of-utterance decision (not just a silence threshold).

        Args:
            transcript_so_far: Best STT result for the current utterance —
                may be partial. Empty string if STT has produced nothing yet.
            silence_ms: Time since the most recent speech frame, in
                milliseconds.

        Returns:
            True if the turn appears complete.
        """
        ...
