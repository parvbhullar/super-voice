"""Agent bridge processor.

Replaces Pipecat's in-process LLM service in the pipeline. In v0 (echo mode)
this simply echoes a user's transcript back as agent text, framed by the
``LLMFullResponseStartFrame`` / ``LLMFullResponseEndFrame`` bookends that the
downstream TTS service expects.

In v1 (Task 15) the same processor will ship transcripts over WSS to a remote
Agent Bridge and stream the agent's text back through the same contract.
"""

from __future__ import annotations

from pipecat.frames.frames import (
    Frame,
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    TextFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor


class AgentBridgeProcessor(FrameProcessor):
    """Pipecat processor that owns the LLM boundary.

    Args:
        echo_mode: When ``True`` (v0), transcripts are echoed back as agent
            text. When ``False``, the processor is a pass-through placeholder
            until the WSS bridge lands in Task 15.
    """

    def __init__(self, echo_mode: bool = False) -> None:
        super().__init__()
        self._echo_mode = echo_mode

    async def process_frame(
        self, frame: Frame, direction: FrameDirection
    ) -> None:
        await super().process_frame(frame, direction)

        if isinstance(frame, TranscriptionFrame) and self._echo_mode:
            await self.push_frame(LLMFullResponseStartFrame())
            await self.push_frame(TextFrame(f"You said: {frame.text}"))
            await self.push_frame(LLMFullResponseEndFrame())
            return

        # Pass-through for everything else.
        await self.push_frame(frame, direction)
