from __future__ import annotations

from importlib.resources import files
from pathlib import Path
from typing import List

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
    stt_preference: List[STTSpec]
    tts_preference: List[TTSSpec]


class VoiceProfileCatalog(BaseModel):
    profiles: List[VoiceProfile]

    @classmethod
    def load_default(cls) -> "VoiceProfileCatalog":
        text = (
            files("supervoice.voice_profile")
            .joinpath("profiles.yaml")
            .read_text()
        )
        return cls.model_validate(yaml.safe_load(text))

    @classmethod
    def load_from(cls, path: Path) -> "VoiceProfileCatalog":
        return cls.model_validate(yaml.safe_load(path.read_text()))

    def get(self, profile_id: str) -> VoiceProfile:
        for p in self.profiles:
            if p.id == profile_id:
                return p
        raise KeyError(profile_id)

    def list(self) -> list[VoiceProfile]:
        return list(self.profiles)
