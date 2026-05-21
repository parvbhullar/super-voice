"""WebRTC transport adapter for supervoice.

Thin factory that builds a Pipecat ``SmallWebRTCTransport`` configured with
audio params suitable for the supervoice pipeline. VAD + SmartTurn EOU
analyzers are passed through the returned object's accompanying detector so
the pipeline builder can wire them into the processor chain.

Note: in Pipecat 1.2.1, VAD/turn analyzers are no longer fields on
``TransportParams``; they are attached to dedicated pipeline processors. We
keep the detector reference on the function return as a tuple so callers can
mount VAD/turn processors at the appropriate pipeline stage.
"""

from __future__ import annotations

from functools import lru_cache

from pipecat.transports.base_transport import TransportParams
from pipecat.transports.smallwebrtc.connection import SmallWebRTCConnection
from pipecat.transports.smallwebrtc.transport import SmallWebRTCTransport

from supervoice.turn.pipecat_impl import PipecatTurnDetector

# Pipeline sample rates (Hz). 16kHz in matches Deepgram/Silero defaults; 24kHz
# out matches Cartesia's native output rate.
AUDIO_IN_SAMPLE_RATE = 16000
AUDIO_OUT_SAMPLE_RATE = 24000


@lru_cache(maxsize=1)
def _default_detector() -> PipecatTurnDetector:
    """Process-wide cached default detector.

    The ``PipecatTurnDetector`` constructor loads two ONNX models (Silero VAD
    + SmartTurn EOU), which takes hundreds of milliseconds. Caching avoids
    re-paying that cost on every call when no custom detector is supplied.
    """
    return PipecatTurnDetector()


def create_webrtc_transport(
    connection: SmallWebRTCConnection,
    detector: PipecatTurnDetector | None = None,
) -> tuple[SmallWebRTCTransport, PipecatTurnDetector]:
    """Build a WebRTC transport plus the VAD/SmartTurn detector to wire.

    Args:
        connection: An established (or pending) ``SmallWebRTCConnection``.
        detector: Optional pre-built detector. If ``detector`` is ``None``,
            a process-wide cached singleton is used (loaded once, reused
            across calls). Production callers MAY pass their own pre-warmed
            detector to avoid sharing.

    Returns:
        ``(transport, detector)`` — the detector is returned so the pipeline
        builder can mount VAD and turn analyzers in the processor chain
        (Pipecat 1.2.1 no longer accepts them on ``TransportParams``).
    """
    if detector is None:
        detector = _default_detector()
    params = TransportParams(
        audio_in_enabled=True,
        audio_out_enabled=True,
        audio_in_sample_rate=AUDIO_IN_SAMPLE_RATE,
        audio_out_sample_rate=AUDIO_OUT_SAMPLE_RATE,
    )
    transport = SmallWebRTCTransport(
        webrtc_connection=connection,
        params=params,
    )
    return transport, detector
