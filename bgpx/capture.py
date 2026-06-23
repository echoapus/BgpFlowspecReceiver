"""BGP packet capture via tcpdump subprocess — results go to EventBus."""

import asyncio
import ipaddress
import logging
import re
import shutil
from typing import Optional

from bgpx.events import EventBus

log = logging.getLogger(__name__)

# BGP message types (first byte after the 16-byte marker + 2-byte length)
_BGP_MSG_TYPES = {1: "OPEN", 2: "UPDATE", 3: "NOTIFICATION", 4: "KEEPALIVE"}

# TCP flag abbreviations
_FLAG_MAP = {
    "S":  "SYN", "R": "RST", "F": "FIN",
    "P":  "PSH", ".": "ACK",
    "S.": "SYN-ACK", "F.": "FIN-ACK", "R.": "RST-ACK", "P.": "PSH-ACK",
}

# Parse a tcpdump text line (with -X hex output lines ignored here)
_PKT_RE = re.compile(
    r"(\d{2}:\d{2}:\d{2}\.\d+)"        # timestamp
    r".+?\s(In|Out)\s+IP\s"            # direction
    r"([\d.]+)\.(\d+)\s+>\s+"          # src ip.port
    r"([\d.]+)\.(\d+):\s+"             # dst ip.port
    r"Flags\s+\[([^\]]+)\]"            # TCP flags
    r".*?length\s+(\d+)"               # TCP payload length
)

# Hex line from tcpdump -X:  \t0x0000:  ffff ffff ...
_HEX_RE = re.compile(r"^\s+0x([0-9a-f]+):\s+(.+)$")


class PacketCapture:
    def __init__(self, events: EventBus):
        self._events = events
        self._proc:  Optional[asyncio.subprocess.Process] = None
        self._task:  Optional[asyncio.Task] = None
        self.running = False

    # ── Public ────────────────────────────────────────────────────────────────

    async def start(self, peer_ip: str) -> bool:
        if self.running:
            await self.stop()
        try:
            ipaddress.ip_address(peer_ip)
        except ValueError:
            self._emit("error", f"Invalid peer IP address: {peer_ip!r}")
            return False
        if not shutil.which("tcpdump"):
            self._emit("error", "tcpdump not found — install: apt install tcpdump")
            return False
        self.running = True
        self._task = asyncio.create_task(self._run(peer_ip))
        return True

    async def stop(self) -> None:
        self.running = False
        if self._proc:
            try:
                self._proc.terminate()
            except ProcessLookupError:
                pass
        if self._task:
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):
                pass
        self._proc = None
        self._task = None

    # ── Internal ──────────────────────────────────────────────────────────────

    async def _run(self, peer_ip: str) -> None:
        # -l  line-buffered
        # -nn no name resolution
        # -X  print hex payload (for BGP type detection)
        # -i any  all interfaces
        cmd = ["tcpdump", "-l", "-nn", "-X", "-i", "any",
               f"host {peer_ip} and port 179"]

        self._emit("info", "Capture started  →  " + " ".join(cmd))
        log.info("Capture: " + " ".join(cmd))

        try:
            self._proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )

            # We accumulate hex lines belonging to the current packet
            current_pkt: Optional[dict] = None
            hex_bytes: list[int] = []

            async for raw in self._proc.stdout:
                if not self.running:
                    break
                line = raw.decode(errors="replace").rstrip()
                if not line:
                    continue

                m_pkt = _PKT_RE.search(line)
                if m_pkt:
                    # Flush previous packet first
                    if current_pkt is not None:
                        self._flush(current_pkt, hex_bytes, peer_ip)

                    ts, direction, src_ip, src_port, dst_ip, dst_port, flags, length = m_pkt.groups()
                    current_pkt = dict(
                        ts=ts, direction=direction,
                        src=f"{src_ip}:{src_port}",
                        dst=f"{dst_ip}:{dst_port}",
                        src_ip=src_ip, dst_ip=dst_ip,
                        flags=flags, length=int(length),
                    )
                    hex_bytes = []
                    continue

                m_hex = _HEX_RE.match(line)
                if m_hex and current_pkt is not None:
                    raw_hex = m_hex.group(2).replace(" ", "")
                    try:
                        hex_bytes += [int(raw_hex[i:i+2], 16)
                                      for i in range(0, len(raw_hex), 2)]
                    except ValueError:
                        pass

            # Flush last packet
            if current_pkt is not None:
                self._flush(current_pkt, hex_bytes, peer_ip)

        except FileNotFoundError:
            self._emit("error", "tcpdump not found")
        except PermissionError:
            self._emit("error", "tcpdump needs root — run bgpx with sudo")
        except asyncio.CancelledError:
            raise
        except Exception as e:
            self._emit("error", f"Capture error: {e}", error=str(e))
        finally:
            if self._proc:
                try:
                    self._proc.terminate()
                    await self._proc.wait()
                except Exception:
                    pass
            self.running = False
            self._emit("info", "Capture stopped")
            log.info("Capture stopped")

    def _flush(self, pkt: dict, hex_bytes: list[int], peer_ip: str) -> None:
        """Emit one packet event, enriched with BGP type if detectable."""
        flags_label = _FLAG_MAP.get(pkt["flags"], pkt["flags"])
        direction   = "→" if pkt["dst_ip"] == peer_ip else "←"
        bgp_type    = _detect_bgp_type(hex_bytes) if hex_bytes else None
        tcp_event   = _tcp_event(pkt["flags"])

        # Build human-readable summary
        summary = (
            f"[{flags_label}]  "
            f"{pkt['src']}  {direction}  {pkt['dst']}"
            f"  len={pkt['length']}"
        )
        if bgp_type:
            summary += f"  ▶ BGP {bgp_type}"
        elif tcp_event:
            summary += f"  ({tcp_event})"

        self._events.emit(
            "capture", "packet", summary,
            src=pkt["src"], dst=pkt["dst"],
            direction=direction,
            flags=flags_label,
            length=pkt["length"],
            bgp_type=bgp_type,
            tcp_event=tcp_event,
        )

    def _emit(self, level: str, message: str, **kw) -> None:
        self._events.emit("capture", level, message, **kw)


# ── Helpers ───────────────────────────────────────────────────────────────────

def _detect_bgp_type(raw: list[int]) -> Optional[str]:
    """
    BGP header: 16 bytes marker (0xFF) + 2 bytes length + 1 byte type.
    In a TCP stream the BGP header starts at byte 0 of the payload.
    tcpdump's hex includes the IP+TCP headers before the payload, so
    we scan for the 0xFF marker sequence.
    """
    marker = [0xFF] * 16
    for i in range(len(raw) - 18):
        if raw[i:i + 16] == marker:
            msg_type = raw[i + 18] if i + 18 < len(raw) else None
            if msg_type in _BGP_MSG_TYPES:
                return _BGP_MSG_TYPES[msg_type]
    return None


def _tcp_event(flags: str) -> Optional[str]:
    if flags in ("S",):     return "TCP SYN — connection attempt"
    if flags in ("S.",):    return "TCP SYN-ACK — connection accepted"
    if flags in ("R", "R."): return "TCP RST — connection refused/reset"
    if flags in ("F", "F."): return "TCP FIN — connection closing"
    return None
