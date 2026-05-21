import time
from supervoice.observability.metrics import CallMetrics


def test_records_ttfa():
    m = CallMetrics(session_id="x")
    m.mark_user_turn_end()
    time.sleep(0.05)
    m.mark_first_agent_audio()
    assert m.ttfa_ms is not None
    assert 30 < m.ttfa_ms < 200


def test_records_asr_final_latency():
    m = CallMetrics(session_id="x")
    m.mark_user_audio_end()
    time.sleep(0.03)
    m.mark_asr_final()
    assert m.asr_final_ms is not None
    assert 20 < m.asr_final_ms < 150


def test_snapshot_contains_session_id():
    m = CallMetrics(session_id="abc")
    snap = m.snapshot()
    assert snap["session_id"] == "abc"


def test_snapshot_ttfa_none_before_marks():
    m = CallMetrics(session_id="x")
    snap = m.snapshot()
    assert snap["ttfa_ms"] is None
    assert snap["asr_final_ms"] is None
