from pydantic import SecretStr

from supervoice.worker.pipeline.builder import PipelineConfig, build_pipeline
from supervoice.shared.speech.stt_factory import STTProviderConfig
from supervoice.shared.speech.tts_factory import TTSProviderConfig


def test_pipeline_has_expected_processors() -> None:
    cfg = PipelineConfig(
        stt=STTProviderConfig(
            provider="deepgram", api_key=SecretStr("x"), language="en"
        ),
        tts=TTSProviderConfig(
            provider="cartesia", api_key=SecretStr("x"), voice_id="v"
        ),
        echo_mode=True,
        transport=None,
    )
    pipeline, bridge = build_pipeline(cfg)
    # Pipecat 1.2.1 exposes a public `processors` property over `_processors`.
    # The list includes implicit source/sink wrappers around our processors.
    names = [p.__class__.__name__ for p in pipeline.processors]
    assert "DeepgramSTTService" in names
    assert "AgentBridgeProcessor" in names
    assert "CartesiaTTSService" in names
    assert "TTSSanitizeFilter" in names
    # Verify order of our four processors
    our = [
        n
        for n in names
        if n
        in {
            "DeepgramSTTService",
            "AgentBridgeProcessor",
            "TTSSanitizeFilter",
            "CartesiaTTSService",
        }
    ]
    assert our == [
        "DeepgramSTTService",
        "AgentBridgeProcessor",
        "TTSSanitizeFilter",
        "CartesiaTTSService",
    ]
    assert bridge.__class__.__name__ == "AgentBridgeProcessor"
