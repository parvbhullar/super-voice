import time
from supervoice.session.state import SessionState


def test_session_state_defaults():
    s = SessionState(session_id="abc")
    assert s.session_id == "abc"
    assert s.is_processing is False
    assert s.idle_warning_count == 0
    assert s.shutdown is False
    assert s.transcript == []


def test_mark_processing_resets_idle():
    s = SessionState(session_id="abc")
    s.mark_idle()
    assert s.idle_since is not None
    time.sleep(0.01)
    s.mark_processing()
    assert s.is_processing is True
    assert s.idle_since is None


def test_append_transcript():
    s = SessionState(session_id="abc")
    s.append_transcript(role="user", text="hello")
    s.append_transcript(role="agent", text="hi there")
    assert len(s.transcript) == 2
    assert s.transcript[0] == {"role": "user", "text": "hello"}
