"""Request-scoped logging with request_id propagation via contextvars."""

from __future__ import annotations

import uuid
from contextvars import ContextVar
from typing import Any

from loguru import logger

request_id_var: ContextVar[str | None] = ContextVar(
    "request_id", default=None
)


def get_request_id() -> str | None:
    """Return the current request_id, or ``None`` outside a request."""
    return request_id_var.get()


def new_request_id() -> str:
    """Generate a new random request id."""
    return str(uuid.uuid4())


def configure_logging() -> None:
    """Configure loguru to include request_id in every log line."""

    def _add_request_id(record: dict[str, Any]) -> None:
        record["extra"]["request_id"] = request_id_var.get() or "-"

    logger.configure(patcher=_add_request_id)
    logger.remove()
    logger.add(
        lambda msg: print(msg, end=""),
        format=(
            "{time:HH:mm:ss} | {level:<7} | "
            "{extra[request_id]} | {message}"
        ),
        level="DEBUG",
    )


__all__ = [
    "configure_logging",
    "get_request_id",
    "new_request_id",
    "request_id_var",
]
