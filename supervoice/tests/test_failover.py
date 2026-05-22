import pytest
from pydantic import SecretStr

from supervoice.shared.speech.failover import (
    resolve_stt_with_fallback,
    resolve_tts_with_fallback,
)
from supervoice.shared.voice_profile.catalog import STTSpec, TTSSpec, VoiceProfile


def test_resolve_stt_uses_first_available_provider():
    """Skip a provider not in api_keys, fall back to next."""
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[
            STTSpec(provider="missing-provider", language="en"),
            STTSpec(provider="deepgram", language="en"),
        ],
        tts_preference=[TTSSpec(provider="cartesia", voice_id="v")],
    )
    api_keys = {"deepgram": SecretStr("dg")}
    stt = resolve_stt_with_fallback(profile, api_keys)
    assert stt.__class__.__name__ == "DeepgramSTTService"


def test_resolve_stt_raises_when_no_provider_available():
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[STTSpec(provider="missing", language="en")],
        tts_preference=[TTSSpec(provider="cartesia", voice_id="v")],
    )
    with pytest.raises(RuntimeError):
        resolve_stt_with_fallback(profile, api_keys={})


def test_resolve_tts_uses_first_available_provider():
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[STTSpec(provider="deepgram", language="en")],
        tts_preference=[
            TTSSpec(provider="missing-provider", voice_id="x"),
            TTSSpec(provider="cartesia", voice_id="v"),
        ],
    )
    api_keys = {"cartesia": SecretStr("ct")}
    tts = resolve_tts_with_fallback(profile, api_keys)
    assert tts.__class__.__name__ == "CartesiaTTSService"


def test_resolve_tts_raises_when_no_provider_available():
    profile = VoiceProfile(
        id="t",
        language="en",
        persona="warm",
        stt_preference=[STTSpec(provider="deepgram", language="en")],
        tts_preference=[TTSSpec(provider="missing", voice_id="v")],
    )
    with pytest.raises(RuntimeError):
        resolve_tts_with_fallback(profile, api_keys={})
