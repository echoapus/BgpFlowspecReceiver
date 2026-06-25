"""Tests for BGP session connection lifecycle behavior."""

import asyncio
from unittest.mock import patch

from bgpx.constants import MSG_UPDATE
from bgpx.events import EventBus
from bgpx.rib import FlowspecRIB
from bgpx.session import BGPSession, ESTABLISHED, OPEN_SENT, SessionConfig


PEER_IP = "192.0.2.1"


class FakeWriter:
    def __init__(self, peer_ip: str = PEER_IP):
        self.peer_ip = peer_ip
        self.closed = False
        self.wait_closed_called = False

    def get_extra_info(self, name, default=None):
        if name == "peername":
            return (self.peer_ip, 179)
        return default

    def close(self):
        self.closed = True

    async def wait_closed(self):
        self.wait_closed_called = True


class NoActiveSession(BGPSession):
    async def _active_attempts(self):
        raise AssertionError("active connect should not start")


def _session(cls=BGPSession):
    events = EventBus()
    cfg = SessionConfig(
        local_as=65000,
        router_id="192.0.2.2",
        peer_ip=PEER_IP,
        peer_as=65300,
    )
    return cls(cfg, FlowspecRIB(), events), events


def test_session_config_uses_fast_establish_defaults():
    cfg = SessionConfig(
        local_as=65000,
        router_id="192.0.2.2",
        peer_ip=PEER_IP,
        peer_as=65300,
    )

    assert cfg.reconnect_delay == 5
    assert cfg.connect_timeout == 5.0
    assert cfg.active_retry_delay == 1.0


def test_race_uses_queued_passive_without_active_attempt():
    async def run():
        session, events = _session(NoActiveSession)
        reader = object()
        writer = FakeWriter()

        await session._on_incoming(reader, writer)
        got_reader, got_writer = await session._race_connect()

        assert got_reader is reader
        assert got_writer is writer
        assert not writer.closed
        assert any(
            event["message"] == f"Passive: using queued connection from {PEER_IP}"
            for event in events.history()
        )

    asyncio.run(run())


def test_reconnect_delay_wakes_on_passive_connection():
    async def run():
        session, events = _session()
        reader = object()
        writer = FakeWriter()

        wait_task = asyncio.create_task(session._wait_reconnect_delay(30))
        await asyncio.sleep(0)
        await session._on_incoming(reader, writer)
        await asyncio.wait_for(wait_task, timeout=0.2)

        got_reader, got_writer = await session._race_connect()

        assert got_reader is reader
        assert got_writer is writer
        assert not writer.closed
        assert any(
            event["message"] == "Passive connection arrived; reconnecting immediately"
            for event in events.history()
        )

    asyncio.run(run())


def test_duplicate_passive_connection_is_dropped_while_active():
    async def run():
        session, events = _session()
        session._set_state(OPEN_SENT)
        writer = FakeWriter()

        await session._on_incoming(object(), writer)

        assert writer.closed
        assert writer.wait_closed_called
        assert session._incoming.empty()
        assert any("duplicate connection" in event["message"] for event in events.history())

    asyncio.run(run())


def test_teardown_discards_queued_passive_connection():
    async def run():
        session, _ = _session()
        writer = FakeWriter()

        await session._on_incoming(object(), writer)
        await session._teardown()

        assert writer.closed
        assert writer.wait_closed_called
        assert session._incoming.empty()

    asyncio.run(run())


def test_update_dispatch_emits_announce_and_withdraw_events():
    async def run():
        session, events = _session()
        session._set_state(ESTABLISHED)
        announce_route = {"dst-prefix": "203.0.113.0/24"}
        withdraw_route = {"dst-prefix": "198.51.100.0/24"}

        path_attributes = [{"code": 16, "name": "EXTENDED_COMMUNITIES"}]
        with patch(
            "bgpx.session.parse_update_details",
            return_value={
                "announce": {"ipv4-flowspec": [announce_route]},
                "withdraw": {"ipv4-flowspec": [withdraw_route]},
                "actions": ["discard"],
                "path_attributes": path_attributes,
            },
        ):
            await session._dispatch(MSG_UPDATE, b"", FakeWriter())

        updates = [event for event in events.history() if event["level"] == "update"]
        assert [event["type"] for event in updates] == ["announce", "withdraw"]
        assert updates[0]["message"] == "ANNOUNCE ipv4-flowspec"
        assert updates[0]["match"] == announce_route
        assert updates[0]["actions"] == ["discard"]
        assert updates[0]["path_attributes"] == path_attributes
        assert updates[1]["message"] == "WITHDRAW ipv4-flowspec"
        assert updates[1]["match"] == withdraw_route
        assert updates[1]["path_attributes"] == path_attributes
        route = session.rib.all()[0]
        assert route["path_attributes"] == path_attributes

    asyncio.run(run())


def test_update_dispatch_stores_unicast_route_metadata():
    async def run():
        session, events = _session()
        session._set_state(ESTABLISHED)
        path_attributes = [
            {"code": 2, "name": "AS_PATH", "value": [
                {"type": "AS_SEQUENCE", "asns": [65000, 65100]}
            ]},
            {"code": 8, "name": "COMMUNITIES", "value": ["65000:100"]},
        ]
        with patch(
            "bgpx.session.parse_update_details",
            return_value={
                "announce": {"ipv4-unicast": [{
                    "prefix": "203.0.113.0/24",
                    "next_hop": "192.0.2.254",
                }]},
                "withdraw": {},
                "actions": [],
                "path_attributes": path_attributes,
            },
        ):
            await session._dispatch(MSG_UPDATE, b"", FakeWriter())

        route = session.rib.all()[0]
        assert route["family"] == "unicast"
        assert route["prefix"] == "203.0.113.0/24"
        assert route["as_path"] == [65000, 65100]
        assert route["communities"] == ["65000:100"]
        announce = [e for e in events.history() if e["type"] == "announce"][0]
        assert announce["family"] == "unicast"
        assert announce["prefix"] == "203.0.113.0/24"

    asyncio.run(run())
