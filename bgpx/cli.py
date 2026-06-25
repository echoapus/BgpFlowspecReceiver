"""CLI entry point for bgpx."""

import argparse
import asyncio
import logging

from bgpx.capture import PacketCapture
from bgpx.events import EventBus
from bgpx.manager import SessionManager
from bgpx.rib import FlowspecRIB
from bgpx.session import SessionConfig


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="bgpx",
        description=(
            "BGP Unicast and FlowSpec Receiver — open http://localhost:8080 to configure\n"
            "and monitor, or pass BGP flags to auto-start the session."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  # Web-only: configure via browser\n"
            "  bgpx\n\n"
            "  # Auto-start session + web UI\n"
            "  bgpx --local-as 65001 --router-id 192.0.2.2 \\\n"
            "       --peer-ip 192.0.2.1 --peer-as 65000\n\n"
            "  # With JSON output and debug logging\n"
            "  bgpx --local-as 65001 --router-id 10.0.0.1 \\\n"
            "       --peer-ip 10.0.0.2 --peer-as 65000 \\\n"
            "       --json-output /tmp/routes.json --log-level DEBUG\n"
        ),
    )

    bgp = p.add_argument_group("BGP session (optional — can also configure via web UI)")
    bgp.add_argument("--local-as",    type=int, metavar="ASN",
                     help="Local AS Number")
    bgp.add_argument("--router-id",   metavar="IP",
                     help="Local BGP router-id (IPv4 address)")
    bgp.add_argument("--peer-ip",     metavar="IP",
                     help="BGP peer IP to connect to")
    bgp.add_argument("--peer-as",     type=int, metavar="ASN",
                     help="BGP peer AS Number")
    bgp.add_argument("--hold-time",   type=int, default=90, metavar="SECS")
    bgp.add_argument("--reconnect-delay", type=int, default=5, metavar="SECS")
    bgp.add_argument("--connect-timeout", type=float, default=5.0, metavar="SECS")
    bgp.add_argument("--active-retry-delay", type=float, default=1.0, metavar="SECS")
    bgp.add_argument("--json-output", metavar="FILE",
                     help="Write RIB to this JSON file on every change")

    web = p.add_argument_group("Web UI / REST API")
    web.add_argument("--host", default="0.0.0.0", metavar="ADDR",
                     help="Listen address (default: 0.0.0.0)")
    web.add_argument("--port", type=int, default=8080, metavar="PORT",
                     help="Listen port (default: 8080)")
    web.add_argument("--log-level", default="INFO",
                     choices=["DEBUG", "INFO", "WARNING", "ERROR"])

    return p


async def _run(args: argparse.Namespace):
    from aiohttp import web
    from bgpx.api import create_app

    events  = EventBus()
    rib     = FlowspecRIB(json_output=args.json_output)
    manager = SessionManager(events, rib)
    capture = PacketCapture(events)

    # Auto-start session if BGP flags were provided
    if args.local_as and args.router_id and args.peer_ip and args.peer_as:
        cfg = SessionConfig(
            local_as        = args.local_as,
            router_id       = args.router_id,
            peer_ip         = args.peer_ip,
            peer_as         = args.peer_as,
            hold_time       = args.hold_time,
            reconnect_delay = args.reconnect_delay,
            connect_timeout = args.connect_timeout,
            active_retry_delay = args.active_retry_delay,
            json_output     = args.json_output,
        )
        await manager.start(cfg)

    app    = create_app(manager, rib, events, capture)
    runner = web.AppRunner(app, access_log=None)
    await runner.setup()
    site = web.TCPSite(runner, args.host, args.port)
    await site.start()

    log = logging.getLogger(__name__)
    log.info(f"Web UI  →  http://{args.host}:{args.port}")
    log.info("Open your browser to configure and monitor BGP flowspec routes.")
    log.info("Press Ctrl+C to stop.")

    # Keep running forever (session tasks are managed by SessionManager)
    try:
        await asyncio.Event().wait()
    except asyncio.CancelledError:
        pass
    finally:
        await manager.stop()
        await runner.cleanup()


def main():
    parser = _build_parser()
    args   = parser.parse_args()

    logging.basicConfig(
        level   = getattr(logging, args.log_level),
        format  = "%(asctime)s  %(levelname)-8s  %(name)s: %(message)s",
        datefmt = "%H:%M:%S",
    )

    try:
        asyncio.run(_run(args))
    except KeyboardInterrupt:
        logging.getLogger(__name__).info("Stopped")
