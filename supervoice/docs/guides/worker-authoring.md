# Worker Authoring Guide

How to build a custom speech worker for supervoice V2 -- for example, to
add a new STT backend, a specialized voice profile, or a custom pipeline
stage.

---

## What is a speech worker?

A **speech worker** is a standalone process that runs one or more PipeCat
speech pipelines on behalf of the orchestrator. When the orchestrator
receives a call dispatch, it selects a worker from the pool, sends it a
`Dispatch` frame over a WebSocket control channel, and the worker spins up
an `AgentAdapter` that drives STT, TTS, and the developer's runner for
that session.

Workers are stateless across jobs -- each dispatched job is self-contained.
A single worker process can serve multiple concurrent jobs up to its
configured `--max-concurrent` limit.

---

## Architecture: orchestrator <-> worker

```
 Telephony / Client
       |
       v
 +-----------------+          WSS control channel         +----------------+
 |  Orchestrator   | <----------------------------------> |  Speech Worker |
 |  (REST API +    |   Register / Registered / Heartbeat  |  (PipeCat +    |
 |   session mgmt) |   Dispatch / DispatchAck             |   bridge WSS)  |
 +-----------------+   StateChanged / JobCompleted         +----------------+
       |                                                         |
       v                                                         v
   Room engine                                             Dev's runner
   (LiveKit or                                             (bridge WSS)
    in-process)
```

The orchestrator and worker communicate exclusively through the **dispatch
protocol** -- a JSON-framed WebSocket channel. The worker never touches the
REST API directly.

---

## The dispatch protocol

Defined in `src/supervoice/shared/dispatch_protocol.py`. All frames are
JSON objects with a `type` discriminator field.

### Frame types

**Worker -> Orchestrator:**

| Frame | `type` | Purpose |
|---|---|---|
| `Register` | `"register"` | Initial hello. Carries `worker_id`, `pool`, and `capabilities`. |
| `Heartbeat` | `"heartbeat"` | Periodic liveness ping. Carries `active_jobs` count. |
| `DispatchAck` | `"dispatch.ack"` | Accept or reject a dispatched job. Fields: `job_id`, `status` (`"accepted"` / `"rejected"`), optional `reason`. |
| `StateChanged` | `"state_changed"` | Mid-job state transition. Fields: `job_id`, `state` (`"connected"` / `"failed"` / `"ended"`), optional `details`. |
| `JobCompleted` | `"job.completed"` | Terminal job report. Fields: `job_id`, `duration_s`, `final_state` (`"ended"` / `"failed"` / `"rejected"` / `"timed_out"`). |

**Orchestrator -> Worker:**

| Frame | `type` | Purpose |
|---|---|---|
| `Registered` | `"registered"` | Confirms registration. Carries `heartbeat_interval_s`. |
| `Dispatch` | `"dispatch"` | Job assignment. Fields: `job_id`, `session_id`, `room`, `voice_profile_id`, `runner_url`, `agent_secret`, `metadata`. |

### Lifecycle sequence

```
Worker                          Orchestrator
  |                                  |
  |--- Register ------------------>  |
  |<-- Registered (heartbeat=10s) -- |
  |                                  |
  |--- Heartbeat (active=0) ------> |  (every heartbeat_interval_s)
  |                                  |
  |<-- Dispatch (job_id, ...) ------ |  (when a call arrives)
  |--- DispatchAck (accepted) ----> |
  |                                  |
  |--- StateChanged (connected) --> |  (after attach succeeds)
  |                                  |
  |  ... job runs ...                |
  |                                  |
  |--- JobCompleted (ended) ------> |  (pipeline finishes)
  |                                  |
  |--- Heartbeat (active=0) ------> |
```

### WorkerCapabilities

Embedded in the `Register` frame:

```python
class WorkerCapabilities(BaseModel):
    voice_profiles: list[str]      # e.g. ["hi-female", "en-female"]
    max_concurrent: int            # 1..10000
```

The orchestrator uses `voice_profiles` for routing: a dispatch for
`voice_profile_id="hi-female"` will only be sent to a worker that
advertises `hi-female` in its capabilities.

---

## Writing a custom AgentAdapter

The `AgentAdapter` (in `src/supervoice/worker/agent_adapter.py`) owns the
lifecycle of a single dispatched job. Its API:

```python
class AgentAdapter:
    def __init__(
        self,
        ctx: JobContext,
        *,
        api_keys: dict[str, SecretStr],
        catalog: VoiceProfileCatalog,
        transport_factory: Callable[[], Any] | None = None,
        bridge_client_factory: BridgeClientFactory = ...,
        pipeline_builder: PipelineBuilder = build_pipeline,
        runner_factory: Callable[[], PipelineRunner] = PipelineRunner,
    ) -> None: ...

    async def attach(self) -> None: ...
    async def wait_for_end(self) -> None: ...
    async def detach(self, reason: str = "ended") -> None: ...
```

The `JobContext` dataclass carries everything from the `Dispatch` frame:

```python
@dataclass(frozen=True)
class JobContext:
    job_id: str
    session_id: str
    room: dict[str, Any]
    voice_profile_id: str
    runner_url: str
    agent_secret: str
    metadata: dict[str, Any] = field(default_factory=dict)
```

### Customization points

To add a new STT or TTS backend, you do NOT need to modify the
`AgentAdapter` itself. Instead:

1. **Register your provider** in the STT/TTS factory
   (`src/supervoice/shared/speech/stt_factory.py` or `tts_factory.py`).
2. **Add a voice profile** that references your provider in the voice
   profile catalog (`src/supervoice/shared/voice_profile/`).
3. **Set the API key** environment variable (see below).

If you need to replace the entire pipeline (not just swap a provider),
inject a custom `pipeline_builder` into the `AgentAdapter` constructor
via a custom `AdapterFactory` on the `JobRunner`:

```python
from supervoice.worker.job_runner import JobRunner, AdapterFactory
from supervoice.worker.agent_adapter import AgentAdapter, JobContext

def my_adapter_factory(ctx: JobContext) -> AgentAdapter:
    return AgentAdapter(
        ctx=ctx,
        api_keys=my_api_keys,
        catalog=my_catalog,
        pipeline_builder=my_custom_pipeline_builder,
    )

job_runner = JobRunner(
    max_concurrent=10,
    api_keys=my_api_keys,
    catalog=my_catalog,
    upstream_send=upstream_send,
    adapter_factory=my_adapter_factory,
)
```

### Job lifecycle (inside JobRunner)

The `JobRunner` (`src/supervoice/worker/job_runner.py`) manages the
accept/run/complete cycle:

1. **accept(dispatch)** -- capacity check, build `JobContext`, create
   `AgentAdapter`, store in `_active` dict, spawn lifecycle task.
2. **_run_job(job_id, adapter)** -- calls `adapter.attach()`, sends
   `StateChanged(state="connected")` upstream, awaits
   `adapter.wait_for_end()`, then `adapter.detach()`, removes from
   `_active`, sends `JobCompleted` upstream.
3. **shutdown()** -- detaches all active jobs and awaits their tasks.

---

## CLI flags

The worker entrypoint is `src/supervoice/worker/main.py`:

```bash
uv run python -m supervoice.worker.main \
    --orchestrator-url ws://orchestrator:8090/v1/internal/workers \
    --shared-secret <secret> \
    --pool default \
    --voice-profiles hi-female,en-female \
    --max-concurrent 50 \
    --worker-id w-custom-001
```

| Flag | Required | Default | Description |
|---|---|---|---|
| `--orchestrator-url` | Yes | -- | WebSocket URL of the orchestrator's internal worker endpoint. |
| `--shared-secret` | Yes | -- | Shared secret for the WSS `Authorization: Bearer` header. |
| `--pool` | No | `"default"` | Worker pool name for routing. |
| `--voice-profiles` | Yes | -- | Comma-separated voice profile IDs this worker can serve. |
| `--max-concurrent` | No | `50` | Maximum concurrent jobs. |
| `--worker-id` | No | auto-generated | Stable worker ID. Defaults to `w-<uuid-hex[:12]>`. |

### Environment variables for API keys

| Variable | Provider |
|---|---|
| `DEEPGRAM_API_KEY` | Deepgram (STT) |
| `CARTESIA_API_KEY` | Cartesia (TTS) |
| `ELEVENLABS_API_KEY` | ElevenLabs (TTS) |

The `build_api_keys_from_env()` function in `worker/main.py` reads these
at startup.

---

## Testing your worker locally

The fastest way to test is **single-process dev mode**, which runs
orchestrator and worker in one process with in-memory queues (no real
WebSocket):

```bash
# Terminal 1: start supervoice
export SUPERVOICE_API_SECRETS="dev-mode:dev-secret"
export DEEPGRAM_API_KEY="your-key"
export CARTESIA_API_KEY="your-key"

uv run uvicorn supervoice.orchestrator.main:app --host 0.0.0.0 --port 8080
```

Where `app` is bound to `create_single_process_app()`.

For testing the worker in isolation against a real orchestrator, start
the orchestrator separately and point the worker at it:

```bash
# Terminal 1: orchestrator
uv run uvicorn supervoice.orchestrator.main:app --port 8090

# Terminal 2: your custom worker
uv run python -m supervoice.worker.main \
    --orchestrator-url ws://localhost:8090/v1/internal/workers \
    --shared-secret your-shared-secret \
    --voice-profiles en-female \
    --max-concurrent 5
```

### Unit testing

The registration protocol is testable without a real WebSocket. The
`WorkerRegistration` class accepts a `link_factory` parameter -- inject
an in-memory `WorkerLink` implementation:

```python
class FakeLink:
    async def send(self, frame: dict) -> None:
        self.sent.append(frame)

    async def recv(self) -> dict:
        return await self.inbound.get()

    async def close(self) -> None:
        pass
```

See `tests/worker/test_registration.py` for examples.

---

## Deploying to production

1. **Container image** -- The worker runs as a standard Python process.
   Package it with `uv` and your `pyproject.toml`.

2. **Scaling** -- Run N worker replicas behind the orchestrator. Each
   worker registers independently over WSS. The orchestrator load-balances
   dispatches across workers based on `active_jobs` counts from heartbeats
   and `voice_profiles` from capabilities.

3. **Pool routing** -- Use `--pool` to segment workers by purpose (e.g.,
   `--pool gpu` for GPU-accelerated workers, `--pool default` for
   standard). The orchestrator dispatches to the pool specified in the
   agent config.

4. **Health** -- The worker's liveness is maintained by the heartbeat
   loop. If heartbeats stop for longer than the orchestrator's
   `heartbeat_timeout_s`, the worker is evicted from the registry.

5. **Graceful shutdown** -- On SIGTERM, call `worker.shutdown()` which
   detaches all active jobs and awaits their lifecycle tasks before
   exiting.

6. **Secrets** -- The `--shared-secret` must match the orchestrator's
   configured worker secret. Provider API keys are read from environment
   variables at startup.
