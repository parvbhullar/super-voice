"""Tests for the V2 orchestrator SessionRegistry."""

from __future__ import annotations

import asyncio

import pytest

from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.session.state import Session


@pytest.mark.asyncio
async def test_register_and_get() -> None:
    reg = SessionRegistry()
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    await reg.register(s)
    assert (await reg.get("s1", tenant_id="t1")) is s


@pytest.mark.asyncio
async def test_get_wrong_tenant_returns_none() -> None:
    reg = SessionRegistry()
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    await reg.register(s)
    assert (await reg.get("s1", tenant_id="t2")) is None


@pytest.mark.asyncio
async def test_list_tenant_scoped() -> None:
    reg = SessionRegistry()
    await reg.register(Session(session_id="s1", tenant_id="t1", metadata={}))
    await reg.register(Session(session_id="s2", tenant_id="t1", metadata={}))
    await reg.register(Session(session_id="s3", tenant_id="t2", metadata={}))
    t1_ids = {s.session_id for s in await reg.list(tenant_id="t1")}
    assert t1_ids == {"s1", "s2"}


@pytest.mark.asyncio
async def test_ttl_drain_to_ended() -> None:
    reg = SessionRegistry(reconnect_ttl_s=0.1)
    s = Session(session_id="s1", tenant_id="t1", metadata={})
    s.transition("ringing")
    s.transition("connected")
    await reg.register(s)
    await reg.mark_draining(s.session_id, tenant_id="t1")
    await asyncio.sleep(0.2)
    fetched = await reg.get("s1", tenant_id="t1")
    assert fetched is not None
    assert fetched.state == "ended"
