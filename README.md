# bgpx

BGP Unicast and FlowSpec Receiver — version 26.180

Receives IPv4/IPv6 unicast routes and BGP Flowspec rules from a peer router,
then maintains an in-memory RIB. A live web UI shows routes, analytics, events,
and packet captures in real time via Server-Sent Events.

---

## Features

- **Dual-mode connection** — races active-connect and passive-accept simultaneously; whichever succeeds first wins
- **IPv4 and IPv6 FlowSpec** — all NLRI component types (prefix, port, protocol, TCP flags, DSCP, fragment, flow-label)
- **IPv4 and IPv6 unicast** — prefix, next hop, AS path, standard/well-known/large communities
- **All standard FlowSpec actions** — rate-limit in network units (bps/pps), discard, redirect-to-VRF (AS2/IPv4/AS4), redirect-to-IP (IPv4/IPv6), DSCP mark, traffic-action
- **Unknown-FlowSpec-EC detection** — vendor or future-IANA extended communities in the FlowSpec EC range are flagged distinctly, with Juniper-style redirect next-hop hints annotated when visible
- **4-byte ASN support** — `AS_TRANS`, `CAP_4BYTE_ASN`, `AS4_PATH` (RFC 6793)
- **Hold timer enforcement** — session resets if no message arrives within the negotiated hold time
- **JSON RIB persistence** — debounced atomic file writes after RIB changes (optional)
- **Web UI** — SSE-driven status/events with server-side route pages; sortable routes table, live log with filter chips, packet capture viewer
- **Docker-ready**

> **⚠️ BGP Session: IPv4 only**
> 
> The BGP TCP session itself is IPv4-only. Both `--peer-ip` and `--router-id`
> must be IPv4 addresses. IPv4/IPv6 Unicast and FlowSpec routes can all be
> received over that session.

---

## Requirements

- Python 3.11+
- `aiohttp >= 3.9`
- `tcpdump` on `$PATH` (optional, for packet capture)
- Rust/Cargo and `maturin` (optional, for the Rust PyO3 parser)

---

## Installation

Quick start — see [INSTALL.md](INSTALL.md) for detailed setup, Docker, systemd, and troubleshooting:

```bash
pip install bgpx
```

Install from this source tree to `/opt/bgpx`:

```bash
sudo ./deploy.sh
```

The deploy script installs from a copied source tree into a virtualenv and
verifies both the packaged Web UI and the selected Python/Rust parser engine.
With `--service`, it enables and starts `bgpx.service` and prints the service
status before exiting.

The deploy script asks which port the Web UI should bind to. Press Enter to use `8080`, or pass the port noninteractively:

```bash
sudo ./deploy.sh --web-port 9090
```

Remove a `/opt/bgpx` deployment:

```bash
sudo ./uninstall.sh
```

Or with Docker:

```bash
docker build -t bgpx .
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

> **⚠️ Port 179:** Requires root or `setcap cap_net_bind_service+ep $(readlink -f $(which python3))`. See [INSTALL.md](INSTALL.md#port-179-configuration).

---

## Usage

### Web UI only (configure via browser)

```bash
bgpx
# Open http://localhost:8080
```

After deploying with `deploy.sh`, open the port selected during deployment:

```bash
sudo ./deploy.sh --web-port 9090
# Open http://localhost:9090
```

Use the compiled Rust parser:

```bash
sudo ./deploy.sh --rust
```

### Auto-start session

```bash
bgpx --local-as 65001 --router-id 192.0.2.2 \
     --peer-ip 192.0.2.1 --peer-as 65000
```

### Full example

```bash
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000 \
     --hold-time 90 \
     --json-output /tmp/routes.json \
     --log-level DEBUG
```

### All flags

| Flag | Default | Description |
|---|---|---|
| `--local-as` | — | Local AS number |
| `--router-id` | — | Local BGP router-id (IPv4 address required) |
| `--peer-ip` | — | BGP peer IP address (IPv4 address required) |
| `--peer-as` | — | BGP peer AS number |
| `--hold-time` | `90` | BGP hold time in seconds (0 = disabled) |
| `--reconnect-delay` | `5` | Seconds to wait before reconnecting |
| `--connect-timeout` | `5.0` | TCP connect timeout in seconds |
| `--active-retry-delay` | `1.0` | Delay between active connect attempts |
| `--json-output` | — | Write RIB to this JSON file after route changes |
| `--host` | `0.0.0.0` | Web UI listen address |
| `--port` | `8080` | Web UI listen port |
| `--log-level` | `INFO` | `DEBUG` / `INFO` / `WARNING` / `ERROR` |

> **Port 179** requires root or:
> ```bash
> sudo setcap cap_net_bind_service+ep $(readlink -f $(which python3))
> ```

---

## Web UI

Open `http://localhost:8080` in a browser.

If installed with `deploy.sh`, use the Web UI port selected during deployment. The default is `8080`.

Session state, counters, and events are pushed via **Server-Sent Events**.
Route table pages are fetched on demand and refreshed after BGP updates, so the
UI can handle larger RIBs without sending every route in the SSE snapshot.

| Panel | Description |
|---|---|
| Sidebar | Configure and start/stop the BGP session. Config is saved to `localStorage`. |
| **Total** tab | Combined unicast and FlowSpec RIB. |
| **Unicast** tab | IPv4/IPv6 prefixes with community, AS path, next hop, and peer. |
| **FlowSpec** tab | Live FlowSpec matches and actions. |
| **Analytics** tab | Family/AFI counts, updates, communities, origin AS, next hops, prefix lengths, FlowSpec actions, protocols, and ports. |
| **Live Log** tab | Real-time event stream. Filter by SESSION / ANNOUNCE / WITHDRAW / ERROR / PCAP. Click an entry to expand JSON detail; `⎘ copy` copies it to clipboard. |
| **◉ Capture** | Start/stop `tcpdump` on BGP traffic. Output appears in the Live Log. |
| **⬇ Export** | Download routes visible in the selected Total/Unicast/FlowSpec view. |

The header shows:
- **SSE dot** — green = stream live, pulsing yellow = reconnecting
- **State badge** — current BGP FSM state (`IDLE` / `CONNECT` / `OPEN_SENT` / `OPEN_CONFIRMED` / `ESTABLISHED`)
- **Negotiated Session** panel — appears after OPEN handshake with peer router-id and negotiated hold time

---

## JSON RIB format

When `--json-output` is set, bgpx writes the RIB atomically after route-change
bursts and flushes pending output on session stop:

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
      "received_at": "2026-06-04T12:00:00.000000+00:00",
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

FlowSpec `traffic-rate-bytes` communities are decoded per RFC 8955 as
bytes/second and rendered as network bits/second. For example, a router rate of
`0.1Mbps` appears as `rate-limit=100000bps` in JSON and `100Kbps` in the UI.
`traffic-rate-packets` remains packets/second (`pps`).

Standard redirect actions are decoded from the IANA FlowSpec extended-community
types. The Juniper-style `0x80:0x0b` extended community is kept as a raw
community because it overlaps with the registered E-Tree Info subtype; when the
same UPDATE carries an MP_REACH next-hop, bgpx annotates it as a hint, for
example `ec=800b000000000000(juniper-redirect-to-ipv4=192.168.1.1)`.

---

## Architecture

```
.
├── bgpx/
│   ├── cli.py          Entry point and component wiring
│   ├── manager.py      Session start/stop/restart
│   ├── session.py      BGP FSM and UPDATE dispatch
│   ├── rib.py          Unicast/FlowSpec RIB, stats, pagination, JSON persistence
│   ├── events.py       Event history and SSE subscribers
│   ├── capture.py      tcpdump subprocess wrapper
│   ├── api.py          aiohttp UI, commands, route pages/export, SSE, health
│   ├── message/
│   │   ├── parser.py   Python parser and optional Rust PyO3 loading
│   │   ├── builder.py  OPEN, KEEPALIVE, and NOTIFICATION builders
│   │   └── flowspec.py FlowSpec NLRI and action parsing
│   └── web/ui.html     Single-file vanilla JavaScript UI
├── bgpx_rust/          Optional Rust PyO3 parser
├── deploy.sh           Host deployment
├── uninstall.sh        Host uninstall
└── test.sh             Full Python/Rust test matrix
```

---

## RFC coverage

| RFC | Title | Status |
|---|---|---|
| RFC 4271 | BGP-4 / IPv4 Unicast | Core FSM, timers, UPDATE and IPv4 Unicast ✅; outbound error notifications ⚠️ |
| RFC 4360 | BGP Extended Communities | ✅ |
| RFC 4760 | MP-BGP / IPv6 Unicast | ✅ |
| RFC 6793 | 4-Byte ASN | OPEN capability, AS_PATH and AS4_PATH ✅ |
| RFC 8955 | IPv4 FlowSpec | ✅ All component types and actions; traffic-rate bytes converted to bps for display |
| RFC 8956 | IPv6 FlowSpec | ✅ All component types and actions |

---

## Address-family support

**BGP session establishment**: IPv4 only. Both the local `--router-id` and `--peer-ip` must be IPv4 addresses.

The OPEN message advertises:

- IPv4 Unicast (`AFI 1 / SAFI 1`)
- IPv6 Unicast (`AFI 2 / SAFI 1`)
- IPv4 FlowSpec (`AFI 1 / SAFI 133`)
- IPv6 FlowSpec (`AFI 2 / SAFI 133`)

Example: connect via IPv4 (`10.0.0.1` ↔ `10.0.0.2`) while receiving an IPv6
Unicast prefix such as `2001:db8::/32` and IPv6 FlowSpec rules.

---

## Development

```bash
pip install -e ".[dev]"
pytest
```

Tests cover message parsing, IPv4/IPv6 Unicast, FlowSpec NLRI/actions, RIB,
session dispatch, package data, and the Python/Rust PyO3 paths.

Run the full Python fallback + Rust PyO3 test matrix:

```bash
./test.sh
```

`test.sh` requires Cargo and `maturin` with cached/downloadable Rust
dependencies. The Rust wheel is installed into a temporary target directory for
the test run, not into the active Python environment. Node.js is optional; when
present, the script also checks the embedded Web UI JavaScript.

---

## License

PolyForm Noncommercial License 1.0.0.

Personal study, research, testing, educational, charitable, public research,
public safety/health, environmental, and government use are permitted. Commercial
or company use requires written permission from the copyright holder.
