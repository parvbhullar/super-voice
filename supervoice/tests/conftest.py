"""Shared pytest fixtures for supervoice tests."""
import pytest_asyncio

from fixtures.mock_bridge import start_mock_bridge


@pytest_asyncio.fixture
async def mock_bridge():
    """Async fixture: yields a ws:// URL for a running mock Agent Bridge."""
    server, url = await start_mock_bridge()
    try:
        yield url
    finally:
        server.close()
        await server.wait_closed()
