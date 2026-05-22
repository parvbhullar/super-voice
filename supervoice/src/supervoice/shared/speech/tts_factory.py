from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, SecretStr


TTSProvider = Literal["cartesia", "elevenlabs"]


class TTSProviderConfig(BaseModel):
    provider: TTSProvider | str
    api_key: SecretStr
    voice_id: str
    sample_rate: int = 24000


def create_tts(config: TTSProviderConfig):
    if config.provider == "cartesia":
        from pipecat.services.cartesia.tts import CartesiaTTSService

        return CartesiaTTSService(
            api_key=config.api_key.get_secret_value(),
            voice_id=config.voice_id,
            sample_rate=config.sample_rate,
        )
    if config.provider == "elevenlabs":
        from pipecat.services.elevenlabs.tts import ElevenLabsTTSService

        return ElevenLabsTTSService(
            api_key=config.api_key.get_secret_value(),
            voice_id=config.voice_id,
            sample_rate=config.sample_rate,
        )
    raise ValueError(f"unknown TTS provider: {config.provider}")
