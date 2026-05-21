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
