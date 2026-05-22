from __future__ import annotations

from importlib.resources import files
from pathlib import Path

import yaml
from pydantic import BaseModel


class STTSpec(BaseModel):
    provider: str
    language: str


class TTSSpec(BaseModel):
    provider: str
    voice_id: str


class VoiceProfile(BaseModel):
    id: str
    language: str
    persona: str
    stt_preference: list[STTSpec]
    tts_preference: list[TTSSpec]


class VoiceProfileCatalog(BaseModel):
    version: int = 1
    profiles: list[VoiceProfile]

    @classmethod
    def load_default(cls) -> "VoiceProfileCatalog":
        text = files("supervoice.voice_profile").joinpath("profiles.yaml").read_text()
        return cls.model_validate(yaml.safe_load(text))

    @classmethod
    def load_from(cls, path: Path) -> "VoiceProfileCatalog":
        return cls.model_validate(yaml.safe_load(path.read_text()))

    def get(self, profile_id: str) -> VoiceProfile:
        for p in self.profiles:
            if p.id == profile_id:
                return p
        raise KeyError(profile_id)

    def all(self) -> list[VoiceProfile]:
        return list(self.profiles)

    def validate_no_placeholders(self) -> None:
        """Raise if any tts_preference still has a REPLACE_ME placeholder.

        Call this at boot to fail loudly when voice IDs haven't been
        configured for production deployment.
        """
        bad: list[str] = []
        for p in self.profiles:
            for tts in p.tts_preference:
                if tts.voice_id.startswith("REPLACE_ME_"):
                    bad.append(f"{p.id}:{tts.provider}:{tts.voice_id}")
        if bad:
            raise RuntimeError(
                "voice profile catalog has placeholder voice_id(s); "
                "replace with real provider IDs before production: " + ", ".join(bad)
            )
