from __future__ import annotations

from loguru import logger
from pydantic import SecretStr

from supervoice.voice_profile.catalog import VoiceProfile

from .stt_factory import STTProviderConfig, create_stt
from .tts_factory import TTSProviderConfig, create_tts


def resolve_stt_with_fallback(
    profile: VoiceProfile,
    api_keys: dict[str, SecretStr],
):
    """Try each provider in the profile's stt_preference; return first success.

    Skips providers without an api_key and providers whose factory raises
    ValueError (unknown provider). Other errors propagate.
    """
    for spec in profile.stt_preference:
        key = api_keys.get(spec.provider)
        if key is None:
            logger.info(
                f"stt provider not configured, trying next: {spec.provider}"
            )
            continue
        try:
            return create_stt(
                STTProviderConfig(
                    provider=spec.provider,
                    api_key=key,
                    language=spec.language,
                )
            )
        except ValueError as e:
            logger.warning(
                f"stt provider unsupported, trying next: {spec.provider} ({e})"
            )
    raise RuntimeError(f"no STT provider available for profile {profile.id}")


def resolve_tts_with_fallback(
    profile: VoiceProfile,
    api_keys: dict[str, SecretStr],
):
    """Try each provider in the profile's tts_preference; return first success."""
    for spec in profile.tts_preference:
        key = api_keys.get(spec.provider)
        if key is None:
            logger.info(
                f"tts provider not configured, trying next: {spec.provider}"
            )
            continue
        try:
            return create_tts(
                TTSProviderConfig(
                    provider=spec.provider,
                    api_key=key,
                    voice_id=spec.voice_id,
                )
            )
        except ValueError as e:
            logger.warning(
                f"tts provider unsupported, trying next: {spec.provider} ({e})"
            )
    raise RuntimeError(f"no TTS provider available for profile {profile.id}")
