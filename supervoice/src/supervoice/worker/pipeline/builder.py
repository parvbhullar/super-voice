"""Pipeline builder for the supervoice processor chain."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from pipecat.frames.frames import Frame, TextFrame
from pipecat.pipeline.pipeline import Pipeline
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor

from supervoice.worker.bridge.processor import AgentBridgeProcessor
from supervoice.shared.speech.sanitize import sanitize_for_tts
from supervoice.shared.speech.stt_factory import STTProviderConfig, create_stt
from supervoice.shared.speech.tts_factory import TTSProviderConfig, create_tts


class TTSSanitizeFilter(FrameProcessor):
    """Strip markdown/URLs from TextFrames before they hit TTS."""

    async def process_frame(self, frame: Frame, direction: FrameDirection) -> None:
        await super().process_frame(frame, direction)
        # NOTE: TranscriptionFrame inherits from TextFrame — use exact-type
        # check so we only sanitize agent text, not user transcripts.
        if type(frame) is TextFrame:
            frame = TextFrame(sanitize_for_tts(frame.text))
        await self.push_frame(frame, direction)


@dataclass
class PipelineConfig:
    """Configuration for the supervoice pipeline."""

    stt: STTProviderConfig
    tts: TTSProviderConfig
    transport: Any = None  # Pipecat transport; passed through to pipeline
    echo_mode: bool = False


def build_pipeline(
    config: PipelineConfig,
) -> tuple[Pipeline, AgentBridgeProcessor]:
    """Construct the processor chain.

    Order:
        transport.input
          -> STT
          -> AgentBridgeProcessor (echo or WSS)
          -> TTSSanitizeFilter
          -> TTS
          -> transport.output
    """
    stt = create_stt(config.stt)
    tts = create_tts(config.tts)
    bridge = AgentBridgeProcessor(echo_mode=config.echo_mode)

    processors: list[Any] = [stt, bridge, TTSSanitizeFilter(), tts]
    if config.transport is not None:
        processors = [
            config.transport.input(),
            *processors,
            config.transport.output(),
        ]

    return Pipeline(processors), bridge
