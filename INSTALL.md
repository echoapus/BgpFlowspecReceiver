# Native Rust Installation

The default application is now the standalone Rust `bgpx` binary. It embeds
the web UI and needs neither Python nor PyO3. The legacy backend has been
removed; its parser outputs are retained as fixed regression fixtures.

## Build and Run

Install a current stable Rust toolchain, then:

```bash
cargo build --locked --release
./target/release/bgpx --host 127.0.0.1 --port 8080
```

Open http://127.0.0.1:8080. Configure the BGP peer in the sidebar, or start
with CLI options:

```bash
./target/release/bgpx --local-as 65001 --router-id 192.0.2.2 \
  --peer-ip 192.0.2.1 --peer-as 65000 --json-output /tmp/routes.json
```

Use `bgpx --help` for all options. The four peer options must be supplied
together. BGP transport remains IPv4; the received routes can be IPv4 or IPv6.

## Host Deployment

```bash
sudo ./deploy.sh --service --cap-net-bind-service --web-port 8080
```

The script builds the release binary, installs it at `/opt/bgpx/bin/bgpx`,
links `/usr/local/bin/bgpx`, and optionally creates and starts
`bgpx.service`. Cargo must be accessible to the user running the script.

To build as your own user and deploy without Cargo under sudo:

```bash
cargo build --locked --release
sudo ./deploy.sh --binary ./target/release/bgpx \
  --service --cap-net-bind-service --web-port 8080
```

Available options: `--install-dir`, `--binary`, `--web-port`, `--service`,
`--no-service`, `--no-link`, and `--cap-net-bind-service`. `INSTALL_DIR` and `WEB_PORT` environment
variables are also supported. The old `--rust` flag is accepted as a no-op.
The old `--python` option is no longer applicable.

For a local installation without changing systemd or the global command link:

```bash
./deploy.sh --binary ./target/release/bgpx --install-dir "$HOME/.local/bgpx" \
  --no-service --no-link --web-port 8080
```

Installation records ownership in `.bgpx-install`. Existing services and global
command links belonging to another installation are not overwritten. Reapply
`--service` when upgrading a running system service so it restarts with the new binary.

The port capability is applied to the bgpx executable, not a Python
interpreter. It must be reapplied when replacing the binary.

## Upgrading a Python Deployment

Re-run deployment with `--service` to replace the service command with the
native binary. Existing legacy `app/` and `venv/` directories are left in
place, but are no longer used by the service. Existing browser settings,
HTTP endpoints and route JSON fields are retained.

The native API validates configuration before starting a session. Invalid
ASNs, IP addresses, ports, hold times and retry delays are rejected.

JSON output retains the existing behavior: it exports the current in-memory
RIB, uses debounced atomic replacement, and flushes when stopping. It does
not restore routes from disk on startup.

## Docker

```bash
docker build -t bgpx .
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

The multi-stage image contains the release executable and tcpdump, with no
Python runtime. To export routes, mount a writable directory and pass
`--json-output /data/routes.json`.

## Port 179 and Packet Capture

Binding TCP port 179 requires root or `CAP_NET_BIND_SERVICE`. Alternatively,
use `--listen-port 9179` and configure the peer to connect to that port.
Active outbound connections still target port 179.

Packet capture requires `tcpdump` on PATH and permission to capture packets.
On Debian/Ubuntu:

```bash
sudo apt-get install tcpdump
```

Container packet capture may also need `--cap-add=NET_RAW --cap-add=NET_ADMIN`.
Capture events and errors appear in the existing Live Log.

## Verification

```bash
./test.sh
./target/release/bgpx --version
curl -i http://127.0.0.1:8080/health
```

The health endpoint returns 200 only for an established BGP session and
503 otherwise. `test.sh` runs Rust formatting, Clippy and tests, shell syntax
checks and 324 parser compatibility fixtures. It also checks JavaScript syntax
when Node is present and runs the standalone HTTP/SSE smoke test when Python 3
is present.

## Service and Uninstall

```bash
sudo systemctl status bgpx
sudo journalctl -u bgpx -f
sudo /opt/bgpx/uninstall.sh
```

The installed uninstaller automatically selects its own installation directory.
When running the source checkout's script, use `--install-dir DIR` for custom paths.
Only the matching service and command link are removed. Unrecognized directories
are refused even with `--force`; rerun the current installer for older deployments
to record ownership first. `--keep-data`
removes the service and command link while retaining the installation.
`--force` skips the interactive confirmation.
