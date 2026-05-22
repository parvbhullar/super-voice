import pytest

from pipecat.frames.frames import (
    LLMFullResponseEndFrame,
    LLMFullResponseStartFrame,
    TextFrame,
    TranscriptionFrame,
)
from pipecat.processors.frame_processor import FrameDirection

from supervoice.worker.bridge.processor import AgentBridgeProcessor


@pytest.mark.asyncio
async def test_echo_mode_emits_transcript_as_agent_text() -> None:
    proc = AgentBridgeProcessor(echo_mode=True)

    pushed: list = []

    async def capture(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = capture  # type: ignore[assignment]

    frame = TranscriptionFrame(text="hello world", user_id="u1", timestamp="t1")
    await proc.process_frame(frame, FrameDirection.DOWNSTREAM)

    text_frames = [f for f in pushed if isinstance(f, TextFrame)]
    assert len(text_frames) >= 1
    assert "hello world" in text_frames[-1].text

    # Verify the 3-frame sequence matches in-process LLM contract.
    assert any(isinstance(f, LLMFullResponseStartFrame) for f in pushed)
    assert any(isinstance(f, LLMFullResponseEndFrame) for f in pushed)


@pytest.mark.asyncio
async def test_echo_mode_disabled_passes_through() -> None:
    proc = AgentBridgeProcessor(echo_mode=False)

    pushed: list = []

    async def capture(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = capture  # type: ignore[assignment]

    frame = TranscriptionFrame(text="ignored", user_id="u1", timestamp="t1")
    await proc.process_frame(frame, FrameDirection.DOWNSTREAM)

    # Non-echo: pure pass-through. No echo TextFrame synthesised (note that
    # TranscriptionFrame inherits from TextFrame, so check via exact type).
    assert not any(type(f) is TextFrame for f in pushed)
    assert any(isinstance(f, TranscriptionFrame) for f in pushed)
    assert len(pushed) == 1


@pytest.mark.asyncio
async def test_non_transcription_frame_passes_through_in_echo_mode() -> None:
    proc = AgentBridgeProcessor(echo_mode=True)

    pushed: list = []

    async def capture(frame, direction=FrameDirection.DOWNSTREAM):
        pushed.append(frame)

    proc.push_frame = capture  # type: ignore[assignment]

    frame = TextFrame("downstream-only")
    await proc.process_frame(frame, FrameDirection.DOWNSTREAM)

    # Should pass through unchanged, no Start/End wrap.
    assert pushed == [frame] or (len(pushed) == 1 and pushed[0] is frame)
