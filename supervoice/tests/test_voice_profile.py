import pytest

from supervoice.voice_profile.catalog import VoiceProfileCatalog


def test_load_default_catalog():
    cat = VoiceProfileCatalog.load_default()
    p = cat.get("hi-female")
    assert p.id == "hi-female"
    assert p.language == "hi"
    assert len(p.stt_preference) >= 1
    assert len(p.tts_preference) >= 1


def test_unknown_profile_raises():
    cat = VoiceProfileCatalog.load_default()
    try:
        cat.get("does-not-exist")
    except KeyError:
        return
    raise AssertionError("expected KeyError")


def test_list_profiles():
    cat = VoiceProfileCatalog.load_default()
    ids = {p.id for p in cat.all()}
    assert {"hi-female", "hi-male", "en-female", "en-male"} <= ids


def test_validate_no_placeholders_raises_for_default_catalog():
    """Default catalog ships with REPLACE_ME placeholders — must fail."""
    cat = VoiceProfileCatalog.load_default()
    with pytest.raises(RuntimeError, match="REPLACE_ME"):
        cat.validate_no_placeholders()


def test_validate_no_placeholders_passes_when_replaced(tmp_path):
    """A catalog without placeholders should validate cleanly."""
    yaml_text = """
version: 1
profiles:
  - id: en-female
    language: en
    persona: warm
    stt_preference:
      - {provider: deepgram, language: en}
    tts_preference:
      - {provider: cartesia, voice_id: real-voice-id-12345}
"""
    path = tmp_path / "profiles.yaml"
    path.write_text(yaml_text)
    cat = VoiceProfileCatalog.load_from(path)
    cat.validate_no_placeholders()  # should not raise
