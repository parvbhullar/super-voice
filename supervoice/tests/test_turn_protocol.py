from supervoice.shared.turn.protocol import TurnDetector


def test_protocol_is_runtime_checkable():
    class StubDetector:
        async def is_speech(self, frame_pcm: bytes) -> bool:
            return True

        async def is_turn_end(self, transcript_so_far: str, silence_ms: int) -> bool:
            return False

    d = StubDetector()
    assert isinstance(d, TurnDetector)


def test_protocol_rejects_missing_method():
    """A class missing is_turn_end must not satisfy isinstance(TurnDetector)."""

    class IncompleteDetector:
        async def is_speech(self, frame_pcm: bytes) -> bool:
            return True

    assert not isinstance(IncompleteDetector(), TurnDetector)
