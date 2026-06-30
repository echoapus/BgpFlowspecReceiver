"""BGP session FSM — always listens AND actively connects; first to succeed wins."""

import asyncio
import logging
from dataclasses import dataclass

from bgpx.constants import (
    BGP_HEADER_LEN,
    MSG_OPEN, MSG_UPDATE, MSG_NOTIFICATION, MSG_KEEPALIVE,
)
from bgpx.events import EventBus
from bgpx.message.builder import build_open, build_keepalive
from bgpx.message.parser import parse_header, parse_open, parse_update_details
from bgpx.rib import FlowspecRIB

log = logging.getLogger(__name__)

IDLE           = "IDLE"
CONNECT        = "CONNECT"
OPEN_SENT      = "OPEN_SENT"
OPEN_CONFIRMED = "OPEN_CONFIRMED"
ESTABLISHED    = "ESTABLISHED"
LISTENING      = "LISTENING"


@dataclass
class SessionConfig:
    local_as:        int
    router_id:       str
    peer_ip:         str
    peer_as:         int
    hold_time:       int           = 90
    reconnect_delay: int           = 5
    connect_timeout: float         = 5.0
    active_retry_delay: float      = 1.0
    listen_port:     int           = 179
    json_output:     str | None = None


@dataclass
class _Connection:
    reader:  asyncio.StreamReader
    writer:  asyncio.StreamWriter
    role:    str
    peer_ip: str


class BGPSession:
    def __init__(
        self,
        config: SessionConfig,
        rib:    FlowspecRIB,
        events: EventBus | None = None,
    ):
        self.config     = config
        self.rib        = rib
        self._events    = events
        self._state     = IDLE
        self._peer_info: dict = {}
        self._hold_time = config.hold_time
        self._ka_task:  asyncio.Task | None = None
        self._running   = True

        # Queue fed by the persistent passive server
        # Size=1 so we don't accumulate stale connections
        self._incoming: asyncio.Queue[_Connection] = asyncio.Queue(maxsize=1)
        self._incoming_event = asyncio.Event()

    # ── Public ────────────────────────────────────────────────────────────────

    @property
    def state(self) -> str:
        return self._state

    @property
    def peer_info(self) -> dict:
        return self._peer_info

    async def run(self):
        """
        Two permanent background tasks run concurrently:
          1. _passive_server  — listens on listen_port forever, feeds _incoming queue
          2. _active_loop     — repeatedly races active-connect vs incoming connection
        """
        server_task = asyncio.create_task(self._passive_server())
        try:
            await self._session_loop()
        finally:
            server_task.cancel()
            try:
                await server_task
            except (asyncio.CancelledError, Exception):
                pass

    def stop(self):
        self._running = False

    # ── Persistent passive server ─────────────────────────────────────────────

    async def _passive_server(self):
        """Always listen on listen_port. Put peer connections in the queue."""
        port = self.config.listen_port
        try:
            server = await asyncio.start_server(
                self._on_incoming, "0.0.0.0", port,
            )
        except PermissionError:
            self._emit(
                "session",
                f"Cannot bind port {port} — passive disabled "
                f"(run with sudo or: sudo setcap cap_net_bind_service+ep $(readlink -f $(which python3)))",
                level="warning",
            )
            log.warning(f"Cannot bind port {port} — passive mode disabled")
            return
        except asyncio.CancelledError:
            return

        self._emit("session", f"Passive listener started on 0.0.0.0:{port}")
        log.info(f"Passive listener on 0.0.0.0:{port}")

        try:
            async with server:
                await server.serve_forever()
        except asyncio.CancelledError:
            pass

    async def _on_incoming(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ):
        peer_ip = writer.get_extra_info("peername", ("unknown",))[0]

        if peer_ip != self.config.peer_ip:
            log.warning(f"Passive: rejected connection from {peer_ip} (not peer)")
            await self._close_writer(writer)
            return

        log.info(f"Passive: incoming connection from {peer_ip}")
        self._emit("session", f"Passive: incoming connection from {peer_ip}")

        if self._state not in (IDLE, CONNECT):
            log.info(
                f"Passive: dropping duplicate connection from {peer_ip} "
                f"while state={self._state}"
            )
            self._emit(
                "session",
                f"Passive: duplicate connection from {peer_ip} dropped "
                f"while {self._state}",
                level="warning",
            )
            await self._close_writer(writer)
            return

        try:
            # Drop if queue is full (session already in progress)
            self._incoming.put_nowait(
                _Connection(reader, writer, "passive", peer_ip)
            )
            self._incoming_event.set()
        except asyncio.QueueFull:
            log.debug("Passive: session already in progress, dropping incoming connection")
            await self._close_writer(writer)

    # ── Session loop ──────────────────────────────────────────────────────────

    async def _session_loop(self):
        """Outer loop: race active vs passive, run session, repeat."""
        while self._running:
            reader, writer = await self._race_connect()
            if reader is None:
                break

            try:
                await self._run_session(reader, writer)
            except asyncio.CancelledError:
                raise
            except Exception as e:
                log.error(f"Session error: {e}")
                self._emit("error", f"Session error: {e}", level="error", error=str(e))
            finally:
                await self._teardown()
                try:
                    writer.close()
                    await writer.wait_closed()
                except Exception:
                    pass

            if self._running:
                delay = self.config.reconnect_delay
                await self._wait_reconnect_delay(delay)

    async def _race_connect(self) -> tuple:
        """Race active-connect vs passive-accept. Return (reader, writer) of winner."""
        self._set_state(CONNECT)

        try:
            conn = self._incoming.get_nowait()
        except asyncio.QueueEmpty:
            conn = None

        if conn:
            if self._incoming.empty():
                self._incoming_event.clear()
            self._emit(
                "session",
                f"Passive: using queued connection from {conn.peer_ip}",
            )
            return conn.reader, conn.writer

        self._emit(
            "session",
            f"Racing: active→{self.config.peer_ip}:179 vs passive←:{self.config.listen_port}",
        )

        active_task   = asyncio.create_task(self._active_attempts())
        incoming_task = asyncio.create_task(self._incoming.get())

        try:
            done, pending = await asyncio.wait(
                [active_task, incoming_task],
                return_when=asyncio.FIRST_COMPLETED,
            )
        except asyncio.CancelledError:
            active_task.cancel()
            incoming_task.cancel()
            for task in (active_task, incoming_task):
                try:
                    await task
                except (asyncio.CancelledError, Exception):
                    pass
            return None, None

        await asyncio.sleep(0)
        ready_pending = {t for t in pending if t.done()}
        done.update(ready_pending)
        pending.difference_update(ready_pending)

        for task in pending:
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass

        candidates: list[_Connection] = []
        for task in done:
            if task.cancelled():
                continue
            if task.exception():
                continue
            candidates.append(task.result())

        if not candidates:
            return None, None

        selected = self._select_connection(candidates)
        for conn in candidates:
            if conn is not selected:
                await self._close_connection(conn)

        return selected.reader, selected.writer

    async def _active_attempts(self) -> tuple:
        """Keep trying to connect to peer:179 until success."""
        attempt = 0
        while True:
            attempt += 1
            try:
                log.debug(f"Active attempt #{attempt} → {self.config.peer_ip}:179")
                reader, writer = await asyncio.wait_for(
                    asyncio.open_connection(self.config.peer_ip, 179),
                    timeout=self.config.connect_timeout,
                )
                peer = writer.get_extra_info("peername")
                log.info(f"Active: TCP connected to {peer}")
                self._emit("session", f"Active: TCP connected to {peer}")
                peer_ip = peer[0] if peer else self.config.peer_ip
                return _Connection(reader, writer, "active", peer_ip)
            except asyncio.CancelledError:
                raise
            except asyncio.TimeoutError:
                log.debug(
                    f"Active connect timed out after {self.config.connect_timeout}s "
                    f"— retry in {self.config.active_retry_delay}s"
                )
                await asyncio.sleep(self.config.active_retry_delay)
            except Exception as e:
                log.debug(
                    f"Active connect failed: {e} "
                    f"— retry in {self.config.active_retry_delay}s"
                )
                await asyncio.sleep(self.config.active_retry_delay)

    # ── BGP session ───────────────────────────────────────────────────────────

    async def _run_session(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        cfg = self.config
        self._set_state(OPEN_SENT)
        await self._send(writer, build_open(cfg.local_as, cfg.hold_time, cfg.router_id))
        log.info("Sent OPEN")
        self._emit("session", "Sent OPEN")
        await self._rx_loop(reader, writer)

    async def _rx_loop(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter):
        while True:
            timeout = self._hold_time if self._hold_time > 0 else None
            try:
                hdr = await asyncio.wait_for(
                    reader.readexactly(BGP_HEADER_LEN), timeout=timeout
                )
            except asyncio.TimeoutError:
                raise ConnectionError(f"Hold timer expired ({self._hold_time}s) — no message from peer")
            msg_type, body_len = parse_header(hdr)
            body = await reader.readexactly(body_len) if body_len > 0 else b''
            await self._dispatch(msg_type, body, writer)

    async def _dispatch(self, msg_type: int, body: bytes, writer: asyncio.StreamWriter):
        if msg_type == MSG_OPEN:
            info = parse_open(body)
            if info["peer_as"] != self.config.peer_as:
                raise ConnectionError(
                    f"Peer ASN mismatch: expected {self.config.peer_as}, got {info['peer_as']}"
                )
            self._peer_info = info
            self._hold_time = min(self.config.hold_time, info["hold_time"])
            log.info(
                f"Received OPEN  peer_as={info['peer_as']}"
                f"  router_id={info['router_id']}"
                f"  hold_time={self._hold_time}s"
            )
            self._emit(
                "session",
                f"Received OPEN from AS {info['peer_as']} ({info['router_id']})"
                f"  hold_time={self._hold_time}s",
                peer_info=info,
            )
            await self._send(writer, build_keepalive())
            self._set_state(OPEN_CONFIRMED)
            if self._hold_time > 0:
                self._ka_task = asyncio.create_task(self._ka_loop(writer))

        elif msg_type == MSG_KEEPALIVE:
            if self._state == OPEN_CONFIRMED:
                self._set_state(ESTABLISHED)
                log.info("Session ESTABLISHED ✓")
            else:
                log.debug("Received KEEPALIVE")

        elif msg_type == MSG_UPDATE:
            if self._state != ESTABLISHED:
                return
            update = parse_update_details(
                body,
                asn_len=4 if self._peer_info.get("supports_4byte_asn") else 2,
            )
            announce = update["announce"]
            withdraw = update["withdraw"]
            actions = update["actions"]
            path_attributes = update["path_attributes"]
            for afi, routes in announce.items():
                for route in routes:
                    if afi.endswith("-unicast"):
                        as_path, communities = _unicast_attributes(path_attributes)
                        route_id = self.rib.add_unicast(
                            afi=afi,
                            prefix=route["prefix"],
                            peer=self.config.peer_ip,
                            next_hop=route.get("next_hop", ""),
                            as_path=as_path,
                            communities=communities,
                            path_attributes=path_attributes,
                        )
                        extra = {
                            "family": "unicast",
                            "prefix": route["prefix"],
                            "next_hop": route.get("next_hop", ""),
                            "as_path": as_path,
                            "communities": communities,
                        }
                    else:
                        route_id = self.rib.add_flowspec(
                            afi, route, actions, self.config.peer_ip,
                            path_attributes=path_attributes,
                        )
                        extra = {
                            "family": "flowspec",
                            "match": route,
                            "actions": actions,
                        }
                    self._emit(
                        "announce", f"ANNOUNCE {afi}",
                        level="update",
                        afi=afi, route_id=route_id,
                        path_attributes=path_attributes,
                        peer=self.config.peer_ip,
                        **extra,
                    )
            for afi, routes in withdraw.items():
                for route in routes:
                    if afi.endswith("-unicast"):
                        route_id = self.rib.remove_unicast(
                            afi, route["prefix"], self.config.peer_ip
                        )
                        extra = {
                            "family": "unicast",
                            "prefix": route["prefix"],
                        }
                    else:
                        route_id = self.rib.remove_flowspec(
                            afi, route, self.config.peer_ip
                        )
                        extra = {
                            "family": "flowspec",
                            "match": route,
                        }
                    self._emit(
                        "withdraw", f"WITHDRAW {afi}",
                        level="update",
                        afi=afi, route_id=route_id,
                        path_attributes=path_attributes,
                        peer=self.config.peer_ip,
                        **extra,
                    )

        elif msg_type == MSG_NOTIFICATION:
            error_code = body[0] if body else 0
            subcode    = body[1] if len(body) > 1 else 0
            log.error(f"NOTIFICATION error={error_code} subcode={subcode}")
            self._emit(
                "error", f"NOTIFICATION error={error_code} subcode={subcode}",
                level="error", error_code=error_code, subcode=subcode,
            )
            raise ConnectionError(f"BGP NOTIFICATION {error_code}/{subcode}")

        else:
            log.warning(f"Unknown message type {msg_type}")

    # ── Helpers ───────────────────────────────────────────────────────────────

    def _emit(self, event_type: str, message: str, level: str = "info", **kw):
        if self._events:
            self._events.emit(event_type, level, message, **kw)

    def _set_state(self, state: str):
        self._state = state
        self._emit("session", f"State → {state}", state=state)

    async def _teardown(self):
        prev            = self._state
        self._state     = IDLE
        self._peer_info = {}
        if self._ka_task:
            self._ka_task.cancel()
            try:
                await self._ka_task
            except (asyncio.CancelledError, Exception):
                pass
            self._ka_task = None
        self.rib.clear_peer(self.config.peer_ip)
        await self._discard_queued_incoming()
        if prev not in (IDLE, LISTENING, CONNECT):
            self._emit("session", "Disconnected", level="warning", state=IDLE)

    async def _send(self, writer: asyncio.StreamWriter, data: bytes):
        writer.write(data)
        await writer.drain()

    async def _ka_loop(self, writer: asyncio.StreamWriter):
        interval = max(1, self._hold_time // 3)
        while True:
            await asyncio.sleep(interval)
            await self._send(writer, build_keepalive())
            log.debug("Sent KEEPALIVE")

    def _select_connection(self, candidates: list[_Connection]) -> _Connection:
        for conn in candidates:
            if conn.role == "passive":
                return conn
        return candidates[0]

    async def _discard_queued_incoming(self):
        while True:
            try:
                conn = self._incoming.get_nowait()
            except asyncio.QueueEmpty:
                self._incoming_event.clear()
                return
            await self._close_connection(conn)

    async def _wait_reconnect_delay(self, delay: int | float):
        if delay <= 0:
            return
        if not self._incoming.empty():
            self._emit(
                "session",
                "Passive connection already queued; reconnecting immediately",
            )
            return

        self._emit(
            "session",
            f"Reconnecting in {delay:g}s …",
            level="warning",
        )
        try:
            await asyncio.wait_for(self._incoming_event.wait(), timeout=delay)
        except asyncio.TimeoutError:
            return

        if self._incoming.empty():
            self._incoming_event.clear()
            return
        self._emit(
            "session",
            "Passive connection arrived; reconnecting immediately",
        )

    async def _close_connection(self, conn: _Connection):
        await self._close_writer(conn.writer)

    async def _close_writer(self, writer: asyncio.StreamWriter):
        writer.close()
        try:
            await writer.wait_closed()
        except Exception:
            pass


def _unicast_attributes(path_attributes: list[dict]) -> tuple[list[int], list[str]]:
    as_path: list[int] = []
    as4_path: list[int] = []
    communities: list[str] = []

    for attribute in path_attributes:
        value = attribute.get("value")
        if attribute["name"] in ("AS_PATH", "AS4_PATH") and isinstance(value, list):
            flattened = [
                asn
                for segment in value
                for asn in segment.get("asns", [])
            ]
            if attribute["name"] == "AS4_PATH":
                as4_path = flattened
            else:
                as_path = flattened
        elif attribute["name"] in ("COMMUNITIES", "LARGE_COMMUNITIES"):
            if isinstance(value, list):
                communities.extend(value)

    if as4_path:
        as_path = as_path[:-len(as4_path)] + as4_path
    return as_path, communities
