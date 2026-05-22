import pytest
from unittest.mock import MagicMock
from pydantic import SecretStr

from supervoice.worker.pipeline.builder import build_pipeline, PipelineConfig
from supervoice.shared.speech.stt_factory import STTProviderConfig
from supervoice.shared.speech.tts_factory import TTSProviderConfig


def test_pipeline_assembly_for_all_default_profiles():
    """Smoke test: build the pipeline for each profile without crashing."""
    from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog
    from supervoice.shared.speech.failover import (
        resolve_stt_with_fallback,
        resolve_tts_with_fallback,
    )

    catalog = VoiceProfileCatalog.load_default()
    api_keys = {
        "deepgram": SecretStr("dg"),
        "cartesia": SecretStr("ct"),
        "elevenlabs": SecretStr("el"),
    }
    for profile in catalog.all():
        stt = resolve_stt_with_fallback(profile, api_keys)
        tts = resolve_tts_with_fallback(profile, api_keys)
        assert stt is not None
        assert tts is not None
        assert stt.__class__.__name__.endswith("STTService")
        assert tts.__class__.__name__.endswith("TTSService")


@pytest.mark.asyncio
async def test_full_pipeline_constructs_with_bridge(mock_bridge):
    """E2E: bridge client + pipeline assembly + processor lifecycle."""
    from supervoice.worker.bridge.client import AgentBridgeClient

    client = AgentBridgeClient(url=mock_bridge)
    try:
        await client.connect()

        transport = MagicMock()
        transport.input = MagicMock(return_value=MagicMock())
        transport.output = MagicMock(return_value=MagicMock())

        cfg = PipelineConfig(
            stt=STTProviderConfig(
                provider="deepgram", api_key=SecretStr("x"), language="en"
            ),
            tts=TTSProviderConfig(
                provider="cartesia", api_key=SecretStr("x"), voice_id="v"
            ),
            transport=transport,
            echo_mode=False,
        )
        pipeline, bridge = build_pipeline(cfg)
        bridge.attach_client(client)
        await bridge.start()

        names = [p.__class__.__name__ for p in pipeline.processors]
        assert "AgentBridgeProcessor" in names
        assert "TTSSanitizeFilter" in names
        assert "DeepgramSTTService" in names
        assert "CartesiaTTSService" in names

        await bridge.stop()
    finally:
        await client.close()
