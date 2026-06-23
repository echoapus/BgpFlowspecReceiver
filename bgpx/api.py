"""Web UI + SSE event stream (aiohttp). All state is pushed via SSE; no REST API."""

import asyncio
import json
import logging
import pathlib
from datetime import datetime, timezone

from aiohttp import web

from bgpx.session import SessionConfig, ESTABLISHED

log = logging.getLogger(__name__)

_UI_PATH = pathlib.Path(__file__).parent / "web" / "ui.html"


def create_app(manager, rib, events, capture) -> web.Application:
    app = web.Application()
    app["manager"] = manager
    app["rib"]     = rib
    app["events"]  = events
    app["capture"] = capture

    app.router.add_get("/",                  _ui)
    app.router.add_post("/session/start",    _session_start)
    app.router.add_post("/session/stop",     _session_stop)
    app.router.add_post("/capture/start",    _capture_start)
    app.router.add_post("/capture/stop",     _capture_stop)
    app.router.add_delete("/log",            _log_clear)
    app.router.add_get("/events",            _sse)
    app.router.add_get("/health",            _health)

    return app


# ── Web UI ────────────────────────────────────────────────────────────────────

async def _ui(req: web.Request) -> web.Response:
    html = _UI_PATH.read_text()
    return web.Response(text=html, content_type="text/html")


# ── Commands ──────────────────────────────────────────────────────────────────

async def _session_start(req: web.Request) -> web.Response:
    try:
        body = await req.json()
    except Exception:
        return _err("Invalid JSON body", 400)

    required = ("local_as", "router_id", "peer_ip", "peer_as")
    missing  = [f for f in required if not body.get(f)]
    if missing:
        return _err(f"Missing fields: {missing}", 400)

    try:
        config = SessionConfig(
            local_as           = int(body["local_as"]),
            router_id          = str(body["router_id"]),
            peer_ip            = str(body["peer_ip"]),
            peer_as            = int(body["peer_as"]),
            hold_time          = int(body.get("hold_time", 90)),
            reconnect_delay    = int(body.get("reconnect_delay", 5)),
            connect_timeout    = float(body.get("connect_timeout", 5.0)),
            active_retry_delay = float(body.get("active_retry_delay", 1.0)),
            listen_port        = int(body.get("listen_port", 179)),
            json_output        = body.get("json_output") or None,
        )
    except (ValueError, TypeError) as e:
        return _err(f"Invalid config: {e}", 400)

    req.app["rib"].set_json_output(config.json_output)
    await req.app["manager"].start(config)
    return web.json_response({"ok": True})


async def _session_stop(req: web.Request) -> web.Response:
    await req.app["manager"].stop()
    return web.json_response({"ok": True})


async def _capture_start(req: web.Request) -> web.Response:
    manager = req.app["manager"]
    capture = req.app["capture"]
    if not manager.is_running or not manager.config:
        return _err("No active session — start a BGP session first", 400)
    ok = await capture.start(manager.config.peer_ip)
    return web.json_response({"ok": ok})


async def _capture_stop(req: web.Request) -> web.Response:
    await req.app["capture"].stop()
    return web.json_response({"ok": True})


async def _log_clear(req: web.Request) -> web.Response:
    req.app["events"].clear()
    return web.json_response({"ok": True})


# ── SSE stream ────────────────────────────────────────────────────────────────

async def _sse(req: web.Request) -> web.StreamResponse:
    resp = web.StreamResponse()
    resp.headers["Content-Type"]      = "text/event-stream"
    resp.headers["Cache-Control"]     = "no-cache"
    resp.headers["X-Accel-Buffering"] = "no"
    await resp.prepare(req)

    manager = req.app["manager"]
    rib     = req.app["rib"]
    bus     = req.app["events"]
    q       = bus.subscribe()

    # Push a snapshot first so the client can initialise without any polling.
    snapshot = {
        "ts":      datetime.now(timezone.utc).isoformat(),
        "type":    "snapshot",
        "level":   "info",
        "message": "snapshot",
        "status":  manager.status(),
        "routes":  rib.to_dict()["routes"],
    }
    await _write_sse(resp, snapshot)

    # Replay past events for the log panel; mark them so the client skips
    # state updates (snapshot is already authoritative).
    for event in bus.history():
        await _write_sse(resp, {**event, "replayed": True})

    try:
        while True:
            try:
                event = await asyncio.wait_for(q.get(), timeout=20.0)
                await _write_sse(resp, event)
            except asyncio.TimeoutError:
                await resp.write(b": heartbeat\n\n")
    except (ConnectionResetError, asyncio.CancelledError):
        pass
    finally:
        bus.unsubscribe(q)

    return resp


async def _write_sse(resp: web.StreamResponse, event: dict):
    data = json.dumps(event, separators=(",", ":"))
    await resp.write(f"data: {data}\n\n".encode())


# ── Health ────────────────────────────────────────────────────────────────────

async def _health(req: web.Request) -> web.Response:
    manager = req.app["manager"]
    ok = manager.is_running and (
        manager.session is not None and manager.session.state == ESTABLISHED
    )
    return web.json_response(
        {"status": "ok" if ok else "degraded",
         "bgp_state": manager.session.state if manager.session else "IDLE"},
        status=200 if ok else 503,
    )


# ── Helpers ───────────────────────────────────────────────────────────────────

def _err(msg: str, status: int = 400) -> web.Response:
    return web.json_response({"error": msg}, status=status)
