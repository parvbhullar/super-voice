from pydantic import SecretStr
from supervoice.shared.speech.tts_factory import create_tts, TTSProviderConfig


def test_create_cartesia_tts():
    cfg = TTSProviderConfig(
        provider="cartesia",
        api_key=SecretStr("ct_test"),
        voice_id="abc-female-en",
    )
    tts = create_tts(cfg)
    assert tts.__class__.__name__ == "CartesiaTTSService"


def test_create_elevenlabs_tts():
    cfg = TTSProviderConfig(
        provider="elevenlabs",
        api_key=SecretStr("el_test"),
        voice_id="rachel",
    )
    tts = create_tts(cfg)
    assert tts.__class__.__name__ == "ElevenLabsTTSService"


def test_unknown_tts_provider_raises():
    cfg = TTSProviderConfig(provider="acme", api_key=SecretStr("x"), voice_id="v")
    try:
        create_tts(cfg)
    except ValueError:
        return
    raise AssertionError("expected ValueError")
