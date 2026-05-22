"""Worker dispatch protocol frame definitions.

Wire format for the orchestrator <-> worker control channel. Both sides
serialize/deserialize via Pydantic models with a `type` discriminator.

This format is consumed by both the orchestrator (Stream D) and the worker
(Stream E); changes here require coordinated updates on both sides.
"""

from __future__ import annotations

from typing import Any, Literal, Union

from pydantic import BaseModel, Field


# -- Orchestrator -> Worker frames -------------------------------------------


class Registered(BaseModel):
    """Sent by orchestrator after accepting a worker's `Register` frame."""

    type: Literal["registered"] = "registered"
    heartbeat_interval_s: int = Field(ge=1, le=300)


class Dispatch(BaseModel):
    """Job dispatch instruction from orchestrator to worker."""

    type: Literal["dispatch"] = "dispatch"
    job_id: str
    session_id: str
    room: dict[str, Any]
    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict[str, Any] = Field(default_factory=dict)


# -- Worker -> Orchestrator frames -------------------------------------------


class WorkerCapabilities(BaseModel):
    """Capability advertisement embedded in `Register`."""

    voice_profiles: list[str]
    max_concurrent: int = Field(ge=1, le=10_000)


class Register(BaseModel):
    """Initial worker hello frame."""

    type: Literal["register"] = "register"
    worker_id: str
    pool: str = "default"
    capabilities: WorkerCapabilities


class Heartbeat(BaseModel):
    """Periodic liveness ping from worker."""

    type: Literal["heartbeat"] = "heartbeat"
    active_jobs: int = Field(ge=0)


class DispatchAck(BaseModel):
    """Worker acknowledgement of a `Dispatch` frame."""

    type: Literal["dispatch.ack"] = "dispatch.ack"
    job_id: str
    status: Literal["accepted", "rejected"]
    reason: str | None = None


class StateChanged(BaseModel):
    """Mid-job state transition notification."""

    type: Literal["state_changed"] = "state_changed"
    job_id: str
    state: Literal["connected", "failed", "ended"]
    details: dict[str, Any] | None = None


class JobCompleted(BaseModel):
    """Terminal job report.

    `final_state` is restricted to terminal SessionStates; non-terminal
    states (`incoming`/`ringing`/`connected`) are rejected because a
    completed job cannot still be in flight.
    """

    type: Literal["job.completed"] = "job.completed"
    job_id: str
    duration_s: float = Field(ge=0)
    final_state: Literal["ended", "failed", "rejected", "timed_out"]
    final_metric: dict[str, Any] | None = None


# -- Union + discriminator ---------------------------------------------------


DispatchFrame = Union[
    Registered,
    Dispatch,
    Register,
    Heartbeat,
    DispatchAck,
    StateChanged,
    JobCompleted,
]


_TYPE_MAP: dict[str, type[BaseModel]] = {
    "registered": Registered,
    "dispatch": Dispatch,
    "register": Register,
    "heartbeat": Heartbeat,
    "dispatch.ack": DispatchAck,
    "state_changed": StateChanged,
    "job.completed": JobCompleted,
}


def parse_frame(raw: dict[str, Any]) -> DispatchFrame:
    """Parse a raw JSON-decoded dict into a typed DispatchFrame.

    Raises:
        ValueError: if the ``type`` field is missing or unrecognized.
    """
    et = raw.get("type")
    if not isinstance(et, str):
        raise ValueError(f"missing or non-string type field: {et!r}")
    cls = _TYPE_MAP.get(et)
    if cls is None:
        raise ValueError(f"unknown dispatch frame type: {et!r}")
    return cls.model_validate(raw)  # type: ignore[return-value]


__all__ = [
    "Dispatch",
    "DispatchAck",
    "DispatchFrame",
    "Heartbeat",
    "JobCompleted",
    "Register",
    "Registered",
    "StateChanged",
    "WorkerCapabilities",
    "parse_frame",
]
