from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, SecretStr


STTProvider = Literal["deepgram", "cartesia"]


class STTProviderConfig(BaseModel):
    provider: STTProvider | str
    api_key: SecretStr
    language: str = "en"
    sample_rate: int = 16000


def create_stt(config: STTProviderConfig):
    """Return a Pipecat STT service for the requested provider.

    Imported lazily so a missing provider extra doesn't break startup
    for users who only need the other.
    """
    if config.provider == "deepgram":
        from pipecat.services.deepgram.stt import DeepgramSTTService

        return DeepgramSTTService(
            api_key=config.api_key.get_secret_value(),
            language=config.language,
            sample_rate=config.sample_rate,
        )
    if config.provider == "cartesia":
        from pipecat.services.cartesia.stt import CartesiaSTTService

        return CartesiaSTTService(
            api_key=config.api_key.get_secret_value(),
            language=config.language,
        )
    raise ValueError(f"unknown STT provider: {config.provider}")
