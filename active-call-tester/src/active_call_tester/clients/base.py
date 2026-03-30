from __future__ import annotations

from typing import Any, Protocol as TypingProtocol

from active_call_tester.models import CallResult


class BaseClient(TypingProtocol):
    """Protocol defining the interface for call clients."""

    async def connect(self, config: Any) -> None:
        """Establish connection using the given config."""
        ...

    async def execute_call(
        self,
        call_config: Any,
    ) -> CallResult:
        """Execute a single test call and return the result."""
        ...

    async def disconnect(self) -> None:
        """Disconnect and clean up resources."""
        ...
