# bgpx

BGP Unicast and FlowSpec receiver with a live web UI — native Rust edition, version 26.9.21.
Connects to a peer router, maintains an in-memory RIB, and streams everything to the browser via Server-Sent Events.

```bash
cargo install --path . --locked
bgpx --local-as 65001 --router-id 192.0.2.2 --peer-ip 192.0.2.1 --peer-as 65000
# open http://localhost:8080
```

The `bgpx` binary embeds the existing web UI and runs BGP, RIB, HTTP and SSE
directly in Rust. Python and PyO3 are not required at runtime. `tcpdump` is
optional and required only for packet capture. The legacy Python/PyO3 backend
has been removed. Its parser outputs are preserved as Rust regression fixtures.

> **BGP session is IPv4-only.** `--peer-ip` and `--router-id` must be IPv4 addresses.  
> Routes received over that session can be IPv4 or IPv6, unicast or FlowSpec.

---

## Features

- **Dual-mode connection** — races active-connect and passive-accept; first to succeed wins
- **IPv4 + IPv6 FlowSpec** (RFC 8955/8956) — all NLRI types: prefix, port, protocol, TCP flags, DSCP, fragment, flow-label
- **IPv4 + IPv6 unicast** — prefix, next-hop, AS path, standard / well-known / large communities
- **All standard FlowSpec actions** — rate-limit (bps/pps), discard, redirect-to-VRF, redirect-to-IP, DSCP mark, traffic-action
- **4-byte ASN** — `AS_TRANS`, `CAP_4BYTE_ASN`, `AS4_PATH` (RFC 6793)
- **Hold-timer enforcement** — session resets on expiry
- **JSON RIB persistence** — debounced atomic writes to a file (`--json-output`)
- **Web UI** — sortable route table, live log with filter chips, analytics, packet capture viewer, `show ip bgp <ip>` longest-prefix-match search

---

## Quick start

```bash
# Web UI only — configure the session in the browser
bgpx

# Auto-start a session
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000

# With JSON output and debug logging
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000 \
     --json-output /tmp/routes.json --log-level DEBUG
```

### Docker

```bash
docker build -t bgpx .
# To enable packet capture, add: --cap-add=NET_RAW --cap-add=NET_ADMIN
# Note: Binding port 179 may require root on standard Linux hosts. 
# Alternatively, use `-p 9179:9179` and pass `--listen-port 9179` to bgpx.
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

---

## CLI flags

| Flag | Default | Description |
|---|---|---|
| `--local-as` | — | Local AS number |
| `--router-id` | — | Local BGP router-id (IPv4) |
| `--peer-ip` | — | BGP peer IP (IPv4) |
| `--peer-as` | — | BGP peer AS number |
| `--hold-time` | `90` | Hold time in seconds (`0` = disabled, otherwise at least `3`) |
| `--reconnect-delay` | `5` | Seconds before reconnecting after a drop |
| `--connect-timeout` | `5.0` | TCP connect timeout |
| `--active-retry-delay` | `1.0` | Delay between active connect attempts |
| `--listen-port` | `179` | Passive BGP listener port |
| `--json-output` | — | Write RIB to this file after each change burst |
| `--host` | `0.0.0.0` | Web UI listen address |
| `--port` | `8080` | Web UI listen port |
| `--log-level` | `INFO` | `DEBUG` / `INFO` / `WARNING` / `ERROR` |

> **Port 179** requires root or:
> ```bash
> sudo setcap cap_net_bind_service+ep "$(readlink -f "$(command -v bgpx)")"
> ```

---

## Web UI

Open `http://localhost:8080`. Session config is saved to `localStorage`.

| Panel | What it shows |
|---|---|
| Sidebar | Configure and start/stop the BGP session |
| **Total / Unicast / FlowSpec** tabs | Paginated, sortable route table |
| **Analytics** tab | Family/AFI counts, top communities, origin AS, next-hops, prefix lengths, FlowSpec actions/protocols/ports |
| **Live Log** tab | SSE event stream — filter by SESSION / ANNOUNCE / WITHDRAW / ERROR / PCAP; click to expand JSON |
| **◉ Capture** | Start/stop `tcpdump` on BGP traffic (requires `tcpdump` on `$PATH`) |
| **⬇ Export** | Download the current table view as JSON |
| **🔍 Search** box | `show ip bgp <ip>` style longest-prefix-match lookup against the unicast RIB (`GET /routes/search?ip=`) |

Memory limits: event replay retains at most 2,000 events or 8 MiB of serialized
JSON, whichever comes first; larger individual events are live-only. Explicitly
stopping a session clears server-side replay history. Live delivery buffers 64
events; lagging clients receive a fresh snapshot. Exports allow two concurrent
snapshots and expire after 60 seconds (additional requests return HTTP 429).
Unused interned route attributes and spare index capacity are reclaimed on the
maintenance worker's 30-second cadence; session teardown releases the containers.

Header indicators:
- **SSE dot** — green = live, pulsing yellow = reconnecting
- **State badge** — `IDLE` → `CONNECT` → `OPEN_SENT` → `OPEN_CONFIRMED` → `ESTABLISHED`

> `announce`/`withdraw` SSE events are batched per BGP UPDATE message — one event
> carries `{"count", "routes": [...], "path_attributes"}` for every route in that
> message, not one event per route. This keeps a full-table initial sync from
> flooding the broadcast channel and the Live Log.

---

## HTTP API & Monitoring

In addition to the Web UI, `bgpx` exposes HTTP endpoints:

- `GET /health` — Returns `200 OK` when the BGP session is `ESTABLISHED`, and `503 Service Unavailable` otherwise. Ideal for load balancer or Kubernetes readiness probes.
- `GET /routes/search?ip=<ip>` — Performs a longest-prefix-match lookup against the unicast RIB.

---

## JSON RIB format

```json
{
  "count": 2,
  "routes": [
    {
      "id": "98d8d25ae319",
      "family": "unicast",
      "afi": "ipv4-unicast",
      "peer": "10.0.0.2",
      "received_at": "2026-06-25T12:00:00+00:00",
      "prefix": "192.0.2.0/24",
      "next_hop": "10.0.0.2",
      "as_path": [65000, 64496],
      "communities": ["65000:100", "NO_EXPORT"],
      "path_attributes": []
    },
    {
      "id": "a3f1b2c4d5e6",
      "family": "flowspec",
      "afi": "ipv4-flowspec",
      "peer": "10.0.0.2",
      "received_at": "2026-06-04T12:00:00+00:00",
      "match": {
        "dst-prefix": "203.0.113.0/24",
        "ip-proto": ["=tcp(6)"],
        "dst-port": ["=80", "=443"]
      },
      "actions": ["discard"],
      "path_attributes": []
    }
  ]
}
```

`traffic-rate-bytes` is decoded per RFC 8955 as bytes/second and rendered as network bits/second — e.g. `0.1 Mbps` from the router appears as `rate-limit=100000bps`.

---

## RFC coverage

### Supported & Partially Supported RFCs

| RFC | Scope | Status | Notes / Limitations |
|---|---|---|---|
| RFC 4271 | BGP-4 / IPv4 Unicast | 🟡 Partial | Receiver-only; FSM, timers, OPEN/UPDATE/KEEPALIVE, IPv4 unicast NLRI. **Unimplemented**: Outbound UPDATE origination, standard outbound NOTIFICATION generation on parse error, BGP over IPv6 transport, best-path selection algorithm across multiple peers. |
| RFC 1997 | BGP Communities | ✅ Full | Standard communities (`ASN:val`) and well-known communities (`NO_EXPORT`, `NO_ADVERTISE`, `NO_EXPORT_SUBCONFED`, `NOPEER`). |
| RFC 4360 | Extended Communities | ✅ Full | 2-octet/4-octet/IPv4-specific extended communities and FlowSpec actions (rate-limit, redirect to VRF, etc.). |
| RFC 4760 | MP-BGP | 🟡 Partial | Capability 1, `MP_REACH_NLRI` / `MP_UNREACH_NLRI` for AFI 1/2 and SAFI 1/133. **Unimplemented**: Other SAFIs (e.g. SAFI 128 L3VPN, SAFI 4 MPLS, SAFI 2 Multicast, SAFI 134 FlowSpec VPN). |
| RFC 5492 | Capabilities Advertisement | ✅ Full | Optional Parameter Type 2 in OPEN message. |
| RFC 5701 | IPv6 Specific Ext. Communities | ✅ Full | Type 25 extended communities (e.g. IPv6 FlowSpec redirect actions). |
| RFC 6793 | 4-Byte ASN | ✅ Full | Capability 65, `AS_TRANS`, 4-octet `AS_PATH`, `AS4_PATH` merging, `AS4_AGGREGATOR`. |
| RFC 7606 | Revised UPDATE Error Handling | 🟡 Partial | Attribute discard / preservation of raw hex on malformed attributes without session drop. **Unimplemented**: Full formal treat-as-withdraw state machine. |
| RFC 8092 | BGP Large Communities | ✅ Full | 12-octet large communities (`admin:data1:data2`). |
| RFC 8955 | IPv4 FlowSpec | 🟡 Partial | All component types (1–12) and actions (rate-limit bps/pps, discard, redirect to VRF/IP, DSCP mark, sample/terminal). **Unimplemented**: Strict ascending component ordering check (Section 5.1), FlowSpec route validation against unicast RIB (Section 6), explicit AND/OR operator grouping in UI. |
| RFC 8956 | IPv6 FlowSpec | 🟡 Partial | Component types 1–13 (including flow-label) and IPv6 redirect actions. **Unimplemented**: Prefix offset octet (Section 3.2; currently decodes `[length][prefix]`, offset $\ne 0$ is unsupported). |
| RFC 9184 | FlowSpec Redirect to IP | ✅ Full | Redirect-to-IP / copy-to-IP extended communities (`0x010C` / `0x000C`). |

### Out-of-Scope / Not Implemented RFCs

| RFC | Name | Status | Reason / Operational Impact |
|---|---|---|---|
| RFC 2918 | Route Refresh Capability | ❌ Not implemented | ROUTE-REFRESH message (Type 5) not supported; requires session re-establishment for RIB refresh. |
| RFC 4724 | Graceful Restart Mechanism | ❌ Not implemented | Capability 64 not advertised; routes are purged immediately upon session termination. |
| RFC 7911 | Advertisement of Multiple Paths (ADD-PATH) | ❌ Not implemented | Path Identifier prefix in NLRI not supported; expects single path per prefix. |
| RFC 8654 | Extended Message Support (64K BGP messages) | ❌ Not implemented | Max message size strictly capped at 4,096 bytes per RFC 4271. |
| RFC 5065 | Autonomous System Confederations | ❌ Not implemented | Confederation segment types parsed if present, but confederation peering/loop detection logic is omitted. |
| RFC 2385 / 5925 | TCP MD5 Signature / TCP-AO | ❌ Not implemented | TCP authentication options not supported at the socket layer. |
| RFC 7999 | BLACKHOLE Community | ❌ Not implemented | Community `65535:666` parsed as generic community rather than named action. |
| RFC 8212 | EBGP Route Propagation without Policies | ❌ Not implemented | No inbound route policy / filtering engine implemented. |

---

## Architecture

*BGP messages are parsed natively (`wire.rs`), stored in an in-memory RIB (`app.rs`), and immediately broadcast to the frontend via Server-Sent Events (SSE).*

```
src/
├── main.rs         CLI, HTTP listener, shutdown and persistence worker
├── session.rs      Tokio BGP connections, FSM, timers and UPDATE dispatch
├── app.rs          RIB, analytics, HTTP routes and SSE history
├── capture.rs      tcpdump lifecycle and packet events
├── wire.rs         native BGP parsing and message building
└── wire_helpers.rs existing Rust FlowSpec decoding helpers
web/ui.html         embedded single-file vanilla JS web UI
```

---

## Development

```bash
cargo build --locked --release
cargo test --locked
./test.sh               # fmt, Clippy, native tests, shell/JS checks
# cargo test includes 324 parser compatibility fixtures.
# Python 3 is optional for the standalone HTTP/SSE smoke test.
```

---

## Installation

You can easily install `bgpx` as a systemd service using the included script:

```bash
sudo ./deploy.sh --service --cap-net-bind-service
```

For full host deployment instructions, custom paths, or to cleanly remove the application via `uninstall.sh`, see [INSTALL.md](INSTALL.md).

**Upgrading from Python:** The legacy Python backend and PyO3 dependency have been completely removed. If you are upgrading an older deployment, simply re-run `./deploy.sh --service` to replace it with the native Rust binary. Existing configurations will be preserved.

---

## License

PolyForm Noncommercial License 1.0.0 — personal, research, educational, government, and public-benefit use permitted. Commercial use requires written permission.
