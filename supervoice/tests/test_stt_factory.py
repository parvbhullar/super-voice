from pydantic import SecretStr
from supervoice.speech.stt_factory import create_stt, STTProviderConfig


def test_create_deepgram():
    cfg = STTProviderConfig(
        provider="deepgram", api_key=SecretStr("dg_test"), language="en"
    )
    stt = create_stt(cfg)
    assert stt.__class__.__name__ == "DeepgramSTTService"


def test_create_cartesia():
    cfg = STTProviderConfig(
        provider="cartesia", api_key=SecretStr("ct_test"), language="en"
    )
    stt = create_stt(cfg)
    assert stt.__class__.__name__ == "CartesiaSTTService"


def test_unknown_provider_raises():
    cfg = STTProviderConfig(provider="acme", api_key=SecretStr("x"), language="en")
    try:
        create_stt(cfg)
    except ValueError as e:
        assert "acme" in str(e)
    else:
        raise AssertionError("expected ValueError")
