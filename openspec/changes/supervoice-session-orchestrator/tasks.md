# supervoice V2 — Implementation Tasks

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Re-architect supervoice into Orchestrator + Speech-Worker split with a Session-centric public API, per the proposal at `proposal.md` and design at `design.md`.

**Architecture:** Python 3.12 + FastAPI + asyncio. Two processes: Orchestrator (REST + room engine + worker dispatch + session registry) and Speech Worker (PipeCat pipeline + bridge WSS to dev's runner). Communication over a JSON-frame WSS dispatch protocol. LiveKit (self-hosted) as the default Room engine; in_process_bus for dev.

**Tech Stack:** Python 3.12, uv, FastAPI, Pydantic v2, pytest+pytest-asyncio, ruff, pyrefly, websockets, loguru. LiveKit Server SDK. Pipecat 1.2.x (carried from V1). All STT/TTS/voice-profile/sanitize/turn modules from V1 stay.

**Working dir:** `/Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/supervoice/`

**Out of scope (deferred):** transfer-with-history, recording, multi-region, worker auto-scale, mid-session voice swap, outbound origination, SDK, multi-session-per-call.

**Reference docs:**
- `proposal.md` — what changes
- `design.md` — boundary-layer details (trait shapes, wire formats, mechanics)
- `../../../supervoice/docs/plans/2026-05-22-supervoice-v2-twopager.md` — exec summary
- `../../../supervoice/docs/plans/2026-05-22-supervoice-v2-flows.md` — 8 ASCII diagrams

---

## Layout

```
supervoice/src/supervoice/
  orchestrator/      ← new: orchestrator service
  worker/            ← new: speech worker service
  shared/            ← extracted from current top-level code
```

V1's modules move:
- `bridge/`, `pipeline/`, `session/handler.py` → `worker/`
- `speech/`, `voice_profile/`, `turn/`, `observability/` → `shared/`
- `session/state.py`, `session/idle_monitor.py` → conceptually extended; orchestrator gets its own `session/state.py`; idle_monitor stays in `worker/` as a per-job timer

---

## Phase 1 — Session model + Room engine (Week 1)

### Task 1: Refactor — move V1 code into `shared/` and `worker/` skeleton

**Files (moves; track via git mv):**

- `src/supervoice/bridge/` → `src/supervoice/worker/bridge/`
- `src/supervoice/pipeline/` → `src/supervoice/worker/pipeline/`
- `src/supervoice/session/handler.py` → `src/supervoice/worker/job_runner.py` (and rename functions)
- `src/supervoice/session/state.py` → STAYS for now; will be rewritten in Task 2
- `src/supervoice/session/idle_monitor.py` → `src/supervoice/worker/idle_monitor.py`
- `src/supervoice/speech/` → `src/supervoice/shared/speech/`
- `src/supervoice/voice_profile/` → `src/supervoice/shared/voice_profile/`
- `src/supervoice/turn/` → `src/supervoice/shared/turn/`
- `src/supervoice/observability/` → `src/supervoice/shared/observability/`
- `src/supervoice/config.py` → `src/supervoice/shared/config.py`
- `src/supervoice/main.py` → `src/supervoice/worker/main.py` (will be split later)

**Step 1: Create the new directory tree**
```bash
cd /Users/parvbhullar/Drives/Vault/Projects/Unpod/super-voice/supervoice
mkdir -p src/supervoice/{orchestrator,worker,shared}
```

**Step 2: Move modules using `git mv`** (preserves history)
```bash
git mv src/supervoice/bridge src/supervoice/worker/bridge
git mv src/supervoice/pipeline src/supervoice/worker/pipeline
git mv src/supervoice/speech src/supervoice/shared/speech
git mv src/supervoice/voice_profile src/supervoice/shared/voice_profile
git mv src/supervoice/turn src/supervoice/shared/turn
git mv src/supervoice/observability src/supervoice/shared/observability
git mv src/supervoice/config.py src/supervoice/shared/config.py
git mv src/supervoice/session/idle_monitor.py src/supervoice/worker/idle_monitor.py
```

**Step 3: Add `__init__.py` to new packages**
```bash
touch src/supervoice/orchestrator/__init__.py
touch src/supervoice/worker/__init__.py
touch src/supervoice/shared/__init__.py
```

**Step 4: Update imports across the codebase**

Run a find/replace:
- `from supervoice.bridge` → `from supervoice.worker.bridge`
- `from supervoice.pipeline` → `from supervoice.worker.pipeline`
- `from supervoice.speech` → `from supervoice.shared.speech`
- `from supervoice.voice_profile` → `from supervoice.shared.voice_profile`
- `from supervoice.turn` → `from supervoice.shared.turn`
- `from supervoice.observability` → `from supervoice.shared.observability`
- `from supervoice.config` → `from supervoice.shared.config`
- `from supervoice.session.idle_monitor` → `from supervoice.worker.idle_monitor`

Tests need the same updates. Use ripgrep + sed.

**Step 5: Run full test suite**
```bash
uv run pytest -v
```
Expected: 65 passed (no behavior change; just imports).

**Step 6: Commit**
```bash
git add -u
git commit -m "refactor(supervoice): move V1 modules into worker/ and shared/ packages"
```

**Acceptance:** All 65 existing tests still pass. No `from supervoice.bridge` / `from supervoice.pipeline` / etc. left in the codebase.

---

### Task 2: Session model + state machine

**Files:**
- Create: `src/supervoice/orchestrator/session/__init__.py`
- Create: `src/supervoice/orchestrator/session/state.py`
- Create: `tests/orchestrator/test_session_state.py`

**Step 1: Write the failing test**

```python
# tests/orchestrator/test_session_state.py
import pytest
from supervoice.orchestrator.session.state import Session, SessionState


def test_session_initial_state():
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    assert s.state == "incoming"
    assert s.external_call_id is None


def test_valid_transitions():
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    assert s.state == "ringing"
    s.transition("connected")
    assert s.state == "connected"
    s.transition("ended")
    assert s.state == "ended"


def test_invalid_transition_raises():
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    with pytest.raises(ValueError, match="invalid transition"):
        s.transition("connected")   # incoming → connected (must go through ringing)


def test_terminal_state_blocks_further_transitions():
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    s.transition("ended")
    with pytest.raises(ValueError, match="terminal"):
        s.transition("connected")


def test_state_history_recorded():
    s = Session(session_id="01J9", tenant_id="t-1", metadata={})
    s.transition("ringing")
    s.transition("connected")
    assert [t[0] for t in s.state_history] == ["incoming", "ringing", "connected"]
```

**Step 2: Run, expect failure**
```bash
uv run pytest tests/orchestrator/test_session_state.py -v
```

**Step 3: Implement**

```python
# src/supervoice/orchestrator/session/state.py
from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Literal


SessionState = Literal[
    "incoming", "ringing", "connected", "rejected", "timed_out",
    "failed", "ended",
]

_TRANSITIONS: dict[SessionState, set[SessionState]] = {
    "incoming":  {"ringing", "rejected", "failed", "ended"},
    "ringing":   {"connected", "rejected", "timed_out", "failed", "ended"},
    "connected": {"failed", "ended"},
    "rejected":  set(),    # terminal
    "timed_out": set(),    # terminal
    "failed":    set(),    # terminal
    "ended":     set(),    # terminal
}


@dataclass
class Session:
    session_id: str
    tenant_id: str
    metadata: dict
    state: SessionState = "incoming"
    external_call_id: str | None = None
    callback_url: str | None = None
    room_handle: object | None = None
    job_id: str | None = None
    created_at: float = field(default_factory=time.monotonic)
    state_history: list[tuple[SessionState, float]] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.state_history.append((self.state, self.created_at))

    def transition(self, new_state: SessionState) -> None:
        valid = _TRANSITIONS.get(self.state, set())
        if not valid:
            raise ValueError(
                f"cannot transition from terminal state {self.state!r}"
            )
        if new_state not in valid:
            raise ValueError(
                f"invalid transition {self.state!r} → {new_state!r} "
                f"(valid: {sorted(valid)})"
            )
        self.state = new_state
        self.state_history.append((new_state, time.monotonic()))
```

**Step 4: Run, expect pass**
```bash
uv run pytest tests/orchestrator/test_session_state.py -v
```

**Step 5: Commit**
```bash
git add src/supervoice/orchestrator/session/ tests/orchestrator/
git commit -m "feat(orchestrator): Session model with state machine"
```

---

### Task 3: Session Registry with reconnect TTL

**Files:**
- Create: `src/supervoice/orchestrator/session/registry.py`
- Create: `tests/orchestrator/test_session_registry.py`

**Step 1: Write failing tests** — register/get/list, tenant scoping, TTL drain → ended.

```python
# tests/orchestrator/test_session_registry.py
import asyncio
import pytest
from supervoice.orchestrator.session.state import Session
from supervoice.orchestrator.session.registry import SessionRegistry


@pytest.mark.asyncio
async def test_register_and_get():
    reg = SessionRegistry()
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    await reg.register(s)
    assert (await reg.get("s1", tenant_id="t1")) is s


@pytest.mark.asyncio
async def test_get_wrong_tenant_returns_none():
    reg = SessionRegistry()
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    await reg.register(s)
    assert (await reg.get("s1", tenant_id="t2")) is None


@pytest.mark.asyncio
async def test_list_tenant_scoped():
    reg = SessionRegistry()
    await reg.register(Session(session_id="s1", tenant_id="t1", metadata={}))
    await reg.register(Session(session_id="s2", tenant_id="t1", metadata={}))
    await reg.register(Session(session_id="s3", tenant_id="t2", metadata={}))
    t1_ids = {s.session_id for s in await reg.list(tenant_id="t1")}
    assert t1_ids == {"s1", "s2"}


@pytest.mark.asyncio
async def test_ttl_drain_to_ended():
    reg = SessionRegistry(reconnect_ttl_s=0.1)
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    s.transition("ringing")
    s.transition("connected")
    await reg.register(s)
    await reg.mark_draining(s.session_id, tenant_id="t1")
    await asyncio.sleep(0.2)
    fetched = await reg.get("s1", tenant_id="t1")
    assert fetched.state == "ended"
```

**Step 2: Implement** — async `SessionRegistry` with a dict keyed by `(tenant_id, session_id)`, background sweeper for TTL drain.

```python
# src/supervoice/orchestrator/session/registry.py
from __future__ import annotations

import asyncio
import time
from typing import Iterable

from .state import Session


class SessionRegistry:
    """In-memory session storage with tenant isolation and TTL drain.

    Keys: (tenant_id, session_id) → Session.
    Drain timer transitions sessions from "draining" to "ended" after TTL.
    """

    def __init__(self, reconnect_ttl_s: float = 30.0) -> None:
        self._sessions: dict[tuple[str, str], Session] = {}
        self._drain_started: dict[tuple[str, str], float] = {}
        self._reconnect_ttl_s = reconnect_ttl_s
        self._lock = asyncio.Lock()

    async def register(self, session: Session) -> None:
        async with self._lock:
            self._sessions[(session.tenant_id, session.session_id)] = session

    async def get(self, session_id: str, *, tenant_id: str) -> Session | None:
        async with self._lock:
            session = self._sessions.get((tenant_id, session_id))
            if session is None:
                return None
            self._maybe_finalize_drain(session)
            return session

    async def list(self, *, tenant_id: str) -> list[Session]:
        async with self._lock:
            return [
                s for (t, _), s in self._sessions.items() if t == tenant_id
            ]

    async def mark_draining(self, session_id: str, *, tenant_id: str) -> None:
        async with self._lock:
            key = (tenant_id, session_id)
            if key in self._sessions:
                self._drain_started[key] = time.monotonic()
                # State transitions are caller's responsibility; we just timestamp.

    def _maybe_finalize_drain(self, session: Session) -> None:
        key = (session.tenant_id, session.session_id)
        start = self._drain_started.get(key)
        if start is None:
            return
        if time.monotonic() - start >= self._reconnect_ttl_s:
            if session.state not in {"ended", "rejected", "timed_out", "failed"}:
                session.transition("ended")
            self._drain_started.pop(key, None)
```

(For the test that expects `state == "ended"` after TTL expiry, the call to `await reg.get(...)` triggers `_maybe_finalize_drain`. Acceptable for in-memory; a background sweeper is V2.)

**Step 3: Run, expect pass; commit.**

```bash
git add src/supervoice/orchestrator/session/registry.py tests/orchestrator/test_session_registry.py
git commit -m "feat(orchestrator): SessionRegistry with tenant scope + reconnect TTL"
```

---

### Task 4: RoomEngine Protocol + types

**Files:**
- Create: `src/supervoice/orchestrator/room/__init__.py`
- Create: `src/supervoice/orchestrator/room/engine.py`
- Create: `tests/orchestrator/test_room_engine_protocol.py`

**Step 1: Write the protocol** (refer to `design.md` §1.2 for the canonical shape).

```python
# src/supervoice/orchestrator/room/engine.py
from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, Protocol


ParticipantType = Literal["sip", "webrtc", "livekit"]


@dataclass(frozen=True)
class RoomOpts:
    session_id: str
    metadata: dict
    max_participants: int = 16
    empty_timeout_s: int = 30


@dataclass(frozen=True)
class RoomHandle:
    room_id: str
    engine_type: str
    engine_handle: object


@dataclass(frozen=True)
class ParticipantHandle:
    participant_id: str
    type: ParticipantType
    engine_handle: object


class RoomEngine(Protocol):
    async def create_room(self, opts: RoomOpts) -> RoomHandle: ...
    async def get_room(self, room_id: str) -> RoomHandle | None: ...
    async def destroy_room(
        self, room: RoomHandle, *, graceful: bool = True
    ) -> None: ...
    async def add_media_participant(
        self, room: RoomHandle, type: ParticipantType, config: dict
    ) -> ParticipantHandle: ...
    async def remove_participant(
        self, room: RoomHandle, participant: ParticipantHandle
    ) -> None: ...
    async def mute_participant(
        self, room: RoomHandle, participant: ParticipantHandle, muted: bool
    ) -> None: ...
    async def move_participants(
        self,
        from_room: RoomHandle,
        to_room: RoomHandle,
        participants: list[ParticipantHandle],
    ) -> list[ParticipantHandle]: ...
```

**Step 2: Protocol-conformance test** with a stub.

**Acceptance:** Stub satisfies isinstance check (`@runtime_checkable` is optional here since Protocol structural matching is sufficient for our type checker).

**Commit:**
```bash
git commit -m "feat(orchestrator): RoomEngine Protocol + RoomOpts/RoomHandle/ParticipantHandle"
```

---

### Task 5: `in_process_engine` implementation

**Files:**
- Create: `src/supervoice/orchestrator/room/in_process_engine.py`
- Create: `tests/orchestrator/test_in_process_engine.py`

**Behavior:**
- `create_room` returns an in-memory Room object holding up to 2 participants
- `add_media_participant` appends to participants list (max 2 — raise `RuntimeError` on 3rd)
- `remove_participant`, `destroy_room`, `mute_participant` mutate state
- `move_participants` raises `NotImplementedError`
- audio fan-out is symbolic in V1 — no actual audio bus needed for the tests at this layer (audio is tested at worker level)

**TDD:**
```python
# Test 1: create room, add 2 participants, fail on 3rd
# Test 2: destroy_room marks the engine_handle as closed
# Test 3: move_participants raises NotImplementedError
# Test 4: get_room returns None for unknown id
```

**Commit:**
```bash
git commit -m "feat(orchestrator): in_process_engine (1:1 rooms, dev/test use)"
```

---

### Task 6: ParticipantAdapter Protocol + stub adapters

**Files:**
- Create: `src/supervoice/orchestrator/participants/__init__.py`
- Create: `src/supervoice/orchestrator/participants/adapter.py`
- Create: `src/supervoice/orchestrator/participants/sip_adapter.py` (stub)
- Create: `src/supervoice/orchestrator/participants/webrtc_adapter.py` (stub)
- Create: `src/supervoice/orchestrator/participants/livekit_adapter.py` (stub)
- Create: `tests/orchestrator/test_participant_adapters.py`

**Step 1: Protocol** (see `design.md` §1.3).

**Step 2: Stub adapters** — each has `attach` and `detach`; raise `NotImplementedError` for now (real impls in later tasks). Structural conformance test.

**Commit:**
```bash
git commit -m "feat(orchestrator): ParticipantAdapter Protocol + stub adapters for sip/webrtc/livekit"
```

---

### Task 7: WebRTC adapter (refactor)

**Files:**
- Modify: `src/supervoice/orchestrator/participants/webrtc_adapter.py`
- Reference: `src/supervoice/worker/pipeline/transport.py` (current V1 code) for SmallWebRTC wiring

**Behavior:** `attach()` performs SDP exchange, returns ParticipantHandle whose `engine_handle` is the `SmallWebRTCConnection`. Reuses the v1 `create_webrtc_transport` logic but lifts it into the adapter shape.

Tests use a mock SDP offer.

**Commit:**
```bash
git commit -m "feat(orchestrator): WebRtcAdapter refactored from V1 SmallWebRTC path"
```

---

## Phase 2 — Worker dispatch protocol (Week 2)

### Task 8: Dispatch frame types

**Files:**
- Create: `src/supervoice/shared/dispatch_protocol.py`
- Create: `tests/shared/test_dispatch_frames.py`

The protocol module lives in `shared/` because both orchestrator and worker consume it.

**Frames** (see `design.md` §1.4): `Register`, `Registered`, `Heartbeat`, `Dispatch`, `DispatchAck`, `StateChanged`, `JobCompleted`. Pydantic models with `type` Literal discriminators.

`parse_frame(dict) -> DispatchFrame` discriminator function.

TDD: roundtrip serialize/deserialize for each frame; unknown type raises ValueError; missing required field raises ValidationError.

**Commit:** `feat(shared): worker dispatch protocol frame types`

---

### Task 9: Worker registry (in-memory)

**Files:**
- Create: `src/supervoice/orchestrator/worker_registry/__init__.py`
- Create: `src/supervoice/orchestrator/worker_registry/registry.py`
- Create: `tests/orchestrator/test_worker_registry.py`

**Behavior:**
- `register(worker_id, pool, capabilities)` — stores worker handle
- `deregister(worker_id)`
- `pick(voice_profile_id, pool)` — returns least-loaded worker matching profile + pool, or None
- `update_load(worker_id, active_jobs)` — heartbeat-driven
- `mark_dispatched(worker_id, job_id)` / `mark_completed(worker_id, job_id)` — track active jobs
- Heartbeat timeout: worker missing heartbeats for 30s → deregistered

TDD covers: register/pick/deregister, least-loaded selection, capability filtering, timeout deregister.

**Commit:** `feat(orchestrator): WorkerRegistry with capability-aware least-loaded selection`

---

### Task 10: Dispatch WSS server endpoint

**Files:**
- Create: `src/supervoice/orchestrator/worker_registry/dispatch.py`
- Create: `tests/orchestrator/test_dispatch_endpoint.py`

**Endpoint:** `WS /v1/internal/workers` — accepts worker connections (shared-secret auth via header).

On connection:
1. Read first frame, expect `Register`. Verify shared secret. If invalid → close 1008.
2. Send `Registered`.
3. Loop: read frames; on `Heartbeat`, update load; on `DispatchAck`/`StateChanged`/`JobCompleted`, route to handler.
4. On disconnect, deregister.

Public API on the orchestrator side: `WorkerDispatcher.dispatch(session_id, ...) -> dispatch result`. Internally: pick worker, send `Dispatch` frame, await `DispatchAck` with 3s timeout, return result.

**TDD:** Use an in-memory pair of asyncio.Queue's as the "WSS" for unit tests. Verify register flow, dispatch+ack happy path, dispatch timeout, dispatch reject.

**Commit:** `feat(orchestrator): dispatch WSS endpoint + WorkerDispatcher`

---

### Task 11: Worker service skeleton

**Files:**
- Create: `src/supervoice/worker/main.py`
- Create: `src/supervoice/worker/registration.py`
- Create: `src/supervoice/worker/job_runner.py`
- Create: `tests/worker/test_worker_registration.py`
- Create: `tests/worker/test_worker_job_runner.py`

**`worker/main.py`:** CLI entrypoint. Reads worker config, opens WSS to orchestrator, runs registration loop, accepts dispatch frames, spawns `job_runner` per accepted job.

**`worker/registration.py`:** Maintains the WSS connection. Sends `Register` on open, periodic `Heartbeat`, handles reconnect.

**`worker/job_runner.py`:** Per-job lifecycle:
1. On `Dispatch` accepted: spawn `AgentAdapter` (Task 12)
2. Track active jobs; send `StateChanged` on lifecycle events
3. On job end: send `JobCompleted`, free slot

TDD against the in-memory dispatcher pair from Task 10.

**Commit:** `feat(worker): service skeleton — register, heartbeat, job runner`

---

### Task 12: AgentAdapter (lift current Pipecat path)

**Files:**
- Create: `src/supervoice/worker/agent_adapter.py`
- Modify: `src/supervoice/worker/job_runner.py` (wire AgentAdapter into accepted jobs)
- Reference (for code to lift): `src/supervoice/worker/pipeline/builder.py`, `src/supervoice/worker/bridge/processor.py`, `src/supervoice/worker/bridge/client.py`

**Behavior:**
- `attach(job_ctx)`: builds PipeCat pipeline using `voice_profile_id` → STT/TTS, joins LiveKit room with `room.url/token`, opens HMAC-signed bridge WSS to `runner_url`, sends `call.started`.
- `detach(reason)`: sends `call.ended`, closes bridge, leaves room.
- `receive_verb(verb)`: dispatches bridge verbs to handlers.

This is a refactor of the current V1 `run_call_with_profile` logic. Keep the bridge protocol, processor, sanitize chain; change the entrypoint.

**TDD:** Use a mock LiveKit transport and a mock bridge server (already exists at `tests/fixtures/mock_bridge.py`).

**Commit:** `feat(worker): AgentAdapter refactor of V1 Pipecat path`

---

### Task 13: End-to-end dispatch in-process smoke

**File:** `tests/integration/test_dispatch_end_to_end.py`

Single test that, in one process:
- Spins up orchestrator's `WorkerDispatcher`
- Spins up a `worker/main.py` instance pointed at it
- Triggers a dispatch via `WorkerDispatcher.dispatch(...)`
- Verifies the worker accepts, AgentAdapter attaches (against mock_bridge fixture + a mock LK transport), `StateChanged: connected` arrives
- Triggers job end; verifies `JobCompleted`

Acceptance: ~3s test, no real LiveKit, no real network. **This validates the protocol contract end-to-end.**

**Commit:** `test: end-to-end dispatch smoke (orchestrator ↔ worker, in-process)`

---

## Phase 3 — Public REST API + LiveKit (Week 3)

### Task 14: Auth middleware + tenant context

**Files:**
- Create: `src/supervoice/orchestrator/api/auth.py`
- Create: `tests/orchestrator/test_auth.py`

API-secret + JWT bearer + tenant extraction. `Depends(get_auth_context)` returns `AuthContext(tenant_id, admin: bool)`.

TDD covers: API-secret happy path, invalid secret → 401, JWT fallback, admin flag from token claim.

**Commit:** `feat(orchestrator): auth middleware (API-secret + JWT + tenant context)`

---

### Task 15: `POST /v1/dispatch` endpoint

**Files:**
- Create: `src/supervoice/orchestrator/api/dispatch.py`
- Create: `tests/orchestrator/test_api_dispatch.py`

**Body schema:** see `proposal.md`'s POST /v1/dispatch section.

**Logic:**
1. Auth → tenant_id
2. Idempotency-Key check
3. Number-mapping lookup (Task 16) for `(tenant_id, to_number)` → fail with 404 if not configured
4. Create Session(state=incoming)
5. RoomEngine.create_room
6. Engine.add_media_participant(type=sip) with SDP offer → sdp_answer
7. WorkerDispatcher.dispatch(session_id, room_handle, voice_profile_id, runner_url, agent_secret, metadata)
8. On dispatch.accepted: Session → ringing
9. Return 201 with session_id, sdp_answer, room.url+token, state_url, external_call_id echo

TDD with mocks for engine + dispatcher + mapping.

**Commit:** `feat(orchestrator): POST /v1/dispatch — single-endpoint session creation`

---

### Task 16: Number-mapping cache

**Files:**
- Create: `src/supervoice/orchestrator/mapping/__init__.py`
- Create: `src/supervoice/orchestrator/mapping/cache.py`
- Create: `src/supervoice/orchestrator/mapping/sync.py`
- Create: `tests/orchestrator/test_mapping.py`

**Cache** (`cache.py`): in-memory dict `(tenant_id, to_number) → AgentConfig`. TTL 5 min on individual entries (re-fetch on stale).

**Sync** (`sync.py`):
- `initial_sync(unpod_url, shared_secret)` — pulled at startup
- `POST /v1/internal/mappings/sync` webhook handler — accepts upsert/delete from unpod

For V1, both can be stubbed (return empty mapping; webhook accepts but no-op) until unpod is integrated. Mark TODOs.

**Commit:** `feat(orchestrator): number→agent mapping cache + sync stubs`

---

### Task 17: `GET /v1/sessions/{id}` + `POST /v1/sessions/{id}/end`

**Files:**
- Create: `src/supervoice/orchestrator/api/sessions.py`
- Create: `tests/orchestrator/test_api_sessions.py`

**Endpoints:**
- `GET` returns session state + room info + participants + job status + external_call_id
- `POST .../end` transitions Session to `draining` (and eventually `ended` via TTL); destroys room; sends job-end to worker.

TDD: happy paths + tenant 404 + idempotency on end.

**Commit:** `feat(orchestrator): GET /v1/sessions/{id} + POST .../end`

---

### Task 18: `POST /v1/sessions/{id}/transfer`

**Files:**
- Modify: `src/supervoice/orchestrator/api/sessions.py`
- Create: `src/supervoice/orchestrator/operations/transfer.py`
- Create: `tests/orchestrator/test_api_transfer.py`

**Body:** `{ to: {type:"sip"|"agent", config}, mode: "cold"|"warm", warm_handoff_ms? }`

**Logic:**
- Add the new participant (or dispatch new agent if `to.type == "agent"`)
- If `mode == "warm"`: wait `warm_handoff_ms`
- Remove the old worker (`agent.end_call` to runner, free job slot) OR old participant
- Return 200 with new participant/job info

TDD cold + warm modes with mocks.

**Commit:** `feat(orchestrator): POST /v1/sessions/{id}/transfer`

---

### Task 19: `POST /v1/sessions/merge`

**Files:**
- Modify: `src/supervoice/orchestrator/api/sessions.py`
- Create: `src/supervoice/orchestrator/operations/merge.py`
- Create: `tests/orchestrator/test_api_merge.py`

**Body:** `{ primary_session_id, secondary_session_ids[], drop_participants?, drop_dispatches? }`

**Logic** (per `design.md` §2): per secondary session, notify worker `call.migrated_to`, detach, move participants, destroy room. Partial-success → 207.

TDD: happy path with one secondary, drop list, partial failure.

**Commit:** `feat(orchestrator): POST /v1/sessions/merge — cross-session merge`

---

### Task 20: LiveKit engine

**Files:**
- Create: `src/supervoice/orchestrator/room/livekit_engine.py`
- Create: `tests/orchestrator/test_livekit_engine.py`

**Behavior:** implements `RoomEngine` against the LiveKit Server SDK. `create_room` issues `RoomServiceClient.create_room`. `add_media_participant(type="sip")` uses LiveKit-SIP `CreateSIPParticipant`. `move_participants` does remove + add via SDK calls.

Tests use a mock LiveKit client (mock out the SDK methods); real integration test against a local LiveKit server can be added later.

**Commit:** `feat(orchestrator): LiveKit engine implementation`

---

### Task 21: `/call` migrated as shim

**Files:**
- Modify: `src/supervoice/orchestrator/main.py` (FastAPI app — new)
- Replace: existing `src/supervoice/worker/main.py`'s `/call` WS endpoint with a shim that calls `POST /v1/dispatch` internally

The orchestrator's main.py:
- Mounts auth middleware
- Mounts the dispatch / sessions / admin routers
- Keeps `/call` WS as a thin shim that:
  1. Receives WebRTC offer JSON
  2. Internally calls `POST /v1/dispatch` with `direction: incoming, transport_type: webrtc, sdp_offer`
  3. Returns `sdp_answer` on the WS
  4. Holds the WS open until session ends

Existing 65 tests that hit `/call` should still pass.

**Commit:** `feat(orchestrator): orchestrator FastAPI app + /call shim`

---

## Phase 4 — Bridge protocol v2 (Week 4)

### Task 22: Protocol handshake + version negotiation

**Files:**
- Modify: `src/supervoice/worker/bridge/protocol.py`
- Create: `tests/worker/test_bridge_handshake.py`

Add `HelloEvent` (runner → worker) and `HelloAckEvent` (worker → runner). On WSS open, runner sends hello with `protocol_version` + supported_events + supported_verbs. Worker responds with hello.ack + negotiated set + `call_id` (= session_id) + `session_id` + `job_id` + `room_id`.

If `protocol_version == 1`, worker degrades to v1 4-event set.

**Commit:** `feat(bridge): v2 protocol handshake + version negotiation`

---

### Task 23: HMAC runner connection

**Files:**
- Modify: `src/supervoice/worker/bridge/client.py`
- Create: `tests/worker/test_bridge_hmac.py`

When opening WSS to `runner_url`, append `?session_id&job_id&nonce&ts&signature`. Compute signature per `design.md` §3.1.

**Commit:** `feat(bridge): HMAC-signed runner connection`

---

### Task 24: `error` event upstream

**Files:**
- Modify: `src/supervoice/worker/bridge/protocol.py` (add `ErrorEvent`)
- Modify: `src/supervoice/worker/bridge/processor.py` (emit on STT/TTS/transport failures)
- Tests

Wire STT/TTS exception paths to emit `ErrorEvent {severity, source, code, message, retriable}` upstream. Failures are no longer silent.

**Commit:** `feat(bridge): error event upstream`

---

### Task 25: `metric` event upstream

**Files:**
- Modify: `src/supervoice/worker/bridge/processor.py`
- Modify: `src/supervoice/shared/observability/metrics.py` (already has CallMetrics)

Emit periodic `metric` snapshot every 10s with TTFA, ASR latency, TTS latency, turns, cost (cost left as placeholder until billing wires in).

**Commit:** `feat(bridge): periodic metric event`

---

### Task 26: New verbs — agent.say, agent.transfer, agent.dispatch, agent.merge, agent.end_call

**Files:**
- Modify: `src/supervoice/worker/bridge/protocol.py` (verb schemas)
- Modify: `src/supervoice/worker/bridge/processor.py` (verb handlers)
- Tests for each

Each verb actuates the corresponding orchestrator REST call (worker keeps an internal HTTP client to its orchestrator). `agent.say` is local (just calls TTS directly with verbatim text, bypass sanitize).

**Commit:** `feat(bridge): v2 verbs — say, transfer, dispatch, merge, end_call`

---

### Task 27: V1 compat mode

**File:** `tests/worker/test_bridge_v1_compat.py`

Ensure that if runner advertises `protocol_version: 1`, the bridge degrades:
- Only emits the v1 4-event set
- Rejects v2-only verbs
- Existing v1 runner tests (from Phase 1 V1 build) still pass

**Commit:** `test(bridge): v1 compat regression`

---

## Phase 5 — SIP + dev mode (Week 5)

### Task 28: SipAdapter

**Files:**
- Modify: `src/supervoice/orchestrator/participants/sip_adapter.py`
- Create: `tests/orchestrator/test_sip_adapter.py`

`attach()` uses LiveKit-SIP via `RoomServiceClient.create_sip_participant(...)`. Tests against mocked LiveKit client.

**Commit:** `feat(orchestrator): SipAdapter via LiveKit-SIP`

---

### Task 29: Telephony integration stub

**Files:**
- Create: `tests/integration/mock_telephony.py` (driver script)
- Create: `tests/integration/test_inbound_call_e2e.py`

Mock telephony script sends `POST /v1/dispatch` with a fake SDP offer, polls `/v1/sessions/{id}` for state transitions, calls `POST .../end`. Full flow tested against the orchestrator + worker stack with mocks for LiveKit and bridge.

**Commit:** `test(integration): mock telephony driving inbound call end-to-end`

---

### Task 30: Dev mode — `--single-process` + audio injection

**Files:**
- Modify: `src/supervoice/orchestrator/main.py` (add `--single-process` CLI flag)
- Create: `src/supervoice/orchestrator/api/dev.py`
- Create: `tests/integration/test_dev_mode.py`

`--single-process`: orchestrator process also spawns one in-process worker via in-memory dispatcher pair (no external WSS needed for dispatch).

`--dev-mode`: enables `/v1/dev/inject-audio` endpoint. Accepts multipart upload of a wav file, injects into the in_process_engine as a synthetic participant.

End-to-end test: dispatch + dev-inject-audio + verify mock runner receives `user.text`.

**Commit:** `feat(orchestrator): dev mode — single-process + audio injection`

---

## Phase 6 — Polish + reliability (Week 6)

### Task 31: Tenant isolation regression suite

**File:** `tests/orchestrator/test_tenant_isolation.py`

For every endpoint: verify cross-tenant access returns 404. Verify GET listing endpoints don't leak cross-tenant resources.

**Commit:** `test(orchestrator): tenant isolation regression`

---

### Task 32: Reconnect TTL regression

**File:** `tests/orchestrator/test_reconnect_ttl.py`

Verify: session enters draining → reconnect within TTL revives to connected; reconnect after TTL → 404; idle ended sessions are GC'd from registry.

**Commit:** `test(orchestrator): reconnect TTL regression`

---

### Task 33: Worker rejection paths

**File:** `tests/orchestrator/test_dispatch_rejection.py`

Verify: all workers reject → session transitions to `rejected` with reason. Some workers reject, one accepts → session goes through. Dispatch budget timeout → `timed_out`.

**Commit:** `test(orchestrator): worker rejection paths`

---

### Task 34: Cleanup-on-failure

**File:** `tests/orchestrator/test_cleanup_on_failure.py`

Verify: an exception during adapter.detach() doesn't skip other adapters. engine.destroy_room failure doesn't prevent session.state = "ended". worker job completion fail doesn't block teardown.

**Commit:** `test(orchestrator): cleanup-on-failure independence`

---

### Task 35: Number-mapping sync wiring (against mock unpod)

**Files:**
- Create: `tests/fixtures/mock_unpod.py`
- Modify: `src/supervoice/orchestrator/mapping/sync.py` (un-stub)

Mock unpod fixture serves agent configs. Orchestrator does initial sync on startup; webhook handler accepts updates.

**Commit:** `feat(orchestrator): number-mapping sync against mock unpod`

---

### Task 36: Observability — request_id propagation + structured logs

**Files:**
- Modify: `src/supervoice/orchestrator/api/main.py` (middleware)
- Modify: `src/supervoice/shared/observability/logging.py` (new file)

Generate `request_id` per request; propagate via loguru context. Every log line carries `request_id`, `session_id`, `tenant_id`, `external_call_id` when in scope.

**Commit:** `feat(observability): request_id + structured log context`

---

## Phase 7 — Docs + design-partner readiness (Week 7)

### Task 37: API reference (OpenAPI)

**File:** `supervoice/docs/api/openapi.yaml`

Generated from FastAPI app; supplement with usage examples per endpoint. Verify `/v1/dispatch`, `/v1/sessions/*`, `/v1/dev/inject-audio` all have request/response schemas with examples.

**Commit:** `docs(supervoice): OpenAPI reference for V2 public API`

---

### Task 38: Bridge protocol v2 spec

**File:** `supervoice/docs/api/bridge-protocol-v2.md`

Standalone wire-format spec. Frame schemas, handshake, HMAC, version negotiation, error semantics. Pull from `design.md` §6 and expand with sequence diagrams.

**Commit:** `docs(supervoice): bridge protocol v2 wire format spec`

---

### Task 39: Worker authoring guide

**File:** `supervoice/docs/guides/worker-authoring.md`

How to build a custom worker (e.g., for a new voice profile family or a custom STT backend). Registration protocol, capabilities advertising, AgentAdapter contract.

**Commit:** `docs(supervoice): worker authoring guide`

---

### Task 40: Dev-mode quickstart

**File:** `supervoice/docs/guides/dev-mode-quickstart.md`

The 5-minute hello-world. Three terminals, sample wav, expected output. Plus a `scripts/dev.sh` that bundles the commands.

**Commit:** `docs(supervoice): dev-mode quickstart + scripts/dev.sh`

---

### Task 41: Telephony integration runbook

**File:** `supervoice/docs/guides/telephony-integration.md`

How telephony talks to supervoice. POST /v1/dispatch shape, SDP handling, webhook events, error responses, SIP-leg bridging via LK-SIP.

**Commit:** `docs(supervoice): telephony integration runbook`

---

### Task 42: Final quality gate

**Step 1:** Lint + format
```bash
cd supervoice
uv run ruff check . --fix
uv run ruff format .
```

**Step 2:** Type check
```bash
uv run pyrefly check
```
Fix or `# pyrefly: ignore[code]` justified cases.

**Step 3:** Full test pass
```bash
uv run pytest -v --tb=short
```

Expected: ~120-135 tests pass.

**Step 4:** Manual smoke
- Start orchestrator + worker (two terminals OR `--single-process`)
- `curl /v1/dispatch` with a fake SDP — verify response
- `curl /v1/sessions/{id}` — verify state
- `curl /v1/dev/inject-audio` — verify wav drives the pipeline

**Step 5:** Commit

```bash
git add -u
git commit -m "chore(supervoice): final lint + format + type pass for V2"
```

---

## Acceptance summary

V2 is shippable to first design partner when:

- [ ] All 42 tasks complete with green tests
- [ ] `POST /v1/dispatch` accepts a fake SDP and returns a session_id
- [ ] Orchestrator + one worker can be run as separate processes OR with `--single-process`
- [ ] `--dev-mode` + `inject-audio` works end-to-end with mock runner in <5 minutes from clean checkout
- [ ] V1 bridge protocol (`protocol_version: 1`) still works (compat mode)
- [ ] OpenAPI doc + bridge protocol spec + 3 guides published
- [ ] Tenant isolation, reconnect TTL, worker rejection, cleanup-on-failure regression suites pass

Total: ~37 working days (~7 weeks) for one engineer. Buffer for LiveKit self-hosting learnings: +1 week.

---

## Post-V1 trigger points (track, do not implement here)

1. **Transfer with history preservation** — V2 follow-up; bridge gains `agent.context_snapshot` event.
2. **Recording stream** — LiveKit Egress integration; surfaces as `recording.*` verbs.
3. **Worker auto-scaling** — orchestrator emits worker-pool metrics; ops decides scaling policy.
4. **Multi-region orchestrator** — geo-routing in unpod points to nearest orchestrator.
5. **Mid-call language switch** — PATCH /v1/dispatch/{did}; needs STT confidence signal.

## Reference skills

- @superpowers:executing-plans — task-by-task execution
- @superpowers:test-driven-development — every task is TDD where applicable
- @superpowers:verification-before-completion — before each commit
- @python-development:python-testing-patterns — pytest-asyncio + FastAPI test patterns
- @python-development:uv-package-manager — uv commands
