"""Manages the BGP session lifecycle — start, stop, restart."""

import asyncio
import logging

from bgpx.events import EventBus
from bgpx.rib import FlowspecRIB
from bgpx.session import BGPSession, SessionConfig

log = logging.getLogger(__name__)


class SessionManager:
    def __init__(self, events: EventBus, rib: FlowspecRIB):
        self._events  = events
        self._rib     = rib
        self._session: BGPSession | None = None
        self._task:    asyncio.Task | None  = None
        self._config:  SessionConfig | None = None

    # ── Public ────────────────────────────────────────────────────────────────

    async def start(self, config: SessionConfig) -> None:
        if self.is_running:
            await self.stop()

        self._rib.clear_all()
        self._config  = config
        self._session = BGPSession(config, self._rib, self._events)
        self._task    = asyncio.create_task(self._session.run())
        self._events.emit(
            "session", "info",
            f"Starting BGP session → {config.peer_ip} (AS {config.peer_as})"
            f"  auto-mode: active + passive :{config.listen_port}",
            running=True,
            peer_ip=config.peer_ip,
            peer_as=config.peer_as,
            local_as=config.local_as,
            router_id=config.router_id,
            hold_time=config.hold_time,
            reconnect_delay=config.reconnect_delay,
            connect_timeout=config.connect_timeout,
            active_retry_delay=config.active_retry_delay,
            listen_port=config.listen_port,
            json_output=config.json_output,
        )
        log.info(f"Session started → {config.peer_ip} (auto-mode)")

    async def stop(self) -> None:
        if self._session:
            self._session.stop()
        if self._task and not self._task.done():
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):
                pass
        self._session = None
        self._task    = None
        self._rib.flush()
        self._events.emit("session", "info", "BGP session stopped", running=False)
        log.info("Session stopped")

    @property
    def session(self) -> BGPSession | None:
        return self._session

    @property
    def is_running(self) -> bool:
        return self._task is not None and not self._task.done()

    @property
    def config(self) -> SessionConfig | None:
        return self._config

    def status(self) -> dict:
        cfg = self._config
        return {
            "running":     self.is_running,
            "state":       self._session.state if self._session else "IDLE",
            "peer_ip":     cfg.peer_ip     if cfg else None,
            "peer_as":     cfg.peer_as     if cfg else None,
            "local_as":    cfg.local_as    if cfg else None,
            "router_id":   cfg.router_id   if cfg else None,
            "reconnect_delay": cfg.reconnect_delay if cfg else 5,
            "connect_timeout": cfg.connect_timeout if cfg else 5.0,
            "active_retry_delay": cfg.active_retry_delay if cfg else 1.0,
            "listen_port": cfg.listen_port if cfg else 179,
            "json_output": cfg.json_output if cfg else None,
            "peer_info":   self._session.peer_info if self._session else {},
        }
