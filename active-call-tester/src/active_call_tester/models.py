from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum


class Protocol(str, Enum):
    """Supported call protocols."""

    WEBSOCKET = "websocket"
    SIP = "sip"
    WEBRTC_AIORTC = "webrtc_aiortc"
    WEBRTC_BROWSER = "webrtc_browser"


class Tier(str, Enum):
    """Test tier levels."""

    SMOKE = "smoke"
    LOAD = "load"
    STRESS = "stress"


@dataclass
class TimestampedEvent:
    """A single timestamped event captured during a call."""

    name: str
    timestamp_ms: float
    data: dict[str, str | float | bool] = field(default_factory=dict)


@dataclass
class CallResult:
    """Result of a single test call."""

    scenario: str
    protocol: Protocol
    codec: str
    asr_provider: str
    tts_provider: str
    setup_latency_ms: float
    first_tts_byte_ms: float
    first_asr_result_ms: float
    teardown_ms: float
    total_duration_ms: float
    success: bool
    error: str | None = None
    events: list[TimestampedEvent] = field(default_factory=list)


@dataclass
class TierResult:
    """Aggregated result for a test tier run."""

    tier: Tier
    scenario: str
    concurrency: int
    total_calls: int
    successful_calls: int
    failed_calls: int
    results: list[CallResult] = field(default_factory=list)
    p50_setup_ms: float = 0.0
    p95_setup_ms: float = 0.0
    p99_setup_ms: float = 0.0
    p50_tts_ms: float = 0.0
    p95_tts_ms: float = 0.0
    p99_tts_ms: float = 0.0
    p50_asr_ms: float = 0.0
    p95_asr_ms: float = 0.0
    p99_asr_ms: float = 0.0
    passed_thresholds: bool = True
