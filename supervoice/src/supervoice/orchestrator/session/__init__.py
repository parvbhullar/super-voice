"""V2 orchestrator session package — Session model + registry.

Re-exports the public types consumed by adjacent orchestrator components
(room engine, participant adapters, REST API).
"""

from __future__ import annotations

from .registry import SessionRegistry
from .state import Session, SessionState

__all__ = ["Session", "SessionState", "SessionRegistry"]
