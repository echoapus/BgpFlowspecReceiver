# Native Rust Installation

The default application is now the standalone Rust `bgpx` binary. It embeds
the web UI and needs neither Python nor PyO3. The legacy backend has been
removed; its parser outputs are retained as fixed regression fixtures.

[English overview](README.md) · [繁體中文說明](README-zhtw.md)

## Build and Run

Install a current stable Rust toolchain and a C compiler (`gcc` or `clang`,
needed to build the bundled mimalloc allocator), then:

```bash
cargo build --locked --release
./target/release/bgpx --host 127.0.0.1 --port 8080
```

Open http://127.0.0.1:8080/ on a trusted private LAN. No login is required.
The WebUI has Total, Unicast, FlowSpec and Live Log tabs; the Analytics tab
has been removed, while route counts and clickable route filters remain.
Configure the BGP peer in the sidebar, or start
with CLI options:

```bash
./target/release/bgpx --local-as 65001 --router-id 192.0.2.2 \
  --peer-ip 192.0.2.1 --peer-as 65000 --json-output /tmp/routes.json
```

Use `bgpx --help` for all options. The four peer options must be supplied
together. BGP transport remains IPv4; the received routes can be IPv4 or IPv6.

## Host Deployment

`deploy.sh` supports Debian/Ubuntu and Red Hat-family hosts through systemd,
and Alpine through OpenRC. Alpine's base image uses BusyBox, so install Bash
before running the script (the script installs GNU coreutils automatically when
run as root):

```bash
apk add bash
```

```bash
sudo ./deploy.sh --service --cap-net-bind-service --web-port 8080
```

The script builds the release binary, installs it at `/opt/bgpx/bin/bgpx`,
links `/usr/local/bin/bgpx`, and optionally creates and starts
`bgpx.service`. Cargo must be accessible to the user running the script.
At completion it prints reverse-proxy checks and Nginx/Caddy examples using
the selected WebUI port. It does not install, configure or reload a proxy.

To build as your own user and deploy without Cargo under sudo:

```bash
cargo build --locked --release
sudo ./deploy.sh --binary ./target/release/bgpx \
  --service --cap-net-bind-service --web-port 8080
```

Available options: `--install-dir`, `--binary`, `--web-port`, `--service`,
`--no-service`, `--no-link`, and `--cap-net-bind-service`. `INSTALL_DIR` and `WEB_PORT` environment
variables are also supported. `--rust` refreshes packages and installs the C
compiler, curl, and CA certificates needed to build from source: it uses `apk`
on Alpine, `apt-get` on Debian/Ubuntu, and `dnf` or `yum` on Red Hat-family
hosts. Run it with `sudo` from the non-root user who owns the source checkout;
it is ignored when `--binary` is supplied. The old `--python` option is no
longer applicable.

`--service` creates `bgpx.service` with systemd, or `/etc/init.d/bgpx` and an
OpenRC default runlevel entry on Alpine. `--cap-net-bind-service` requires the
platform libcap tools (`libcap2-bin` on Debian/Ubuntu; `libcap` on Alpine and
Red Hat-family distributions).

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

### UI Languages (Locales)

The HTML, CSS and JavaScript are compiled into the binary; only translations
live on disk. `deploy.sh` copies `web/locales/*.json` to `<install-dir>/locales/`
and records the shipped files in `.bgpx-locales`. On upgrade, shipped files are
refreshed and ones no longer shipped are removed; files you added yourself are kept.

To add a language, copy `en.json` to `<code>.json` (for example `ja.json`),
translate the values, and set `"_name"` to the name shown in the language menu.
Codes may contain only letters, digits, `-` and `_`. Reload the page; no
restart is required. bgpx searches `./web/locales`, `./locales`,
`<binary-dir>/locales`, `<binary-dir>/../locales` and `<binary-dir>/../web/locales`,
with later directories taking precedence. `en`, `ko` and `zh-TW` are also
embedded in the binary as fallbacks.

## Upgrading a Python Deployment

Re-run deployment with `--service` to replace the service command with the
native binary. Existing legacy `app/` and `venv/` directories are left in
place, but are no longer used by the service. Existing browser settings,
endpoint URLs remain, but API requests now require the WebUI headers described
below, including `/health`. Stored routes and exports contain decoded fields;
raw `path_attributes` are carried on announce/withdraw events rather than
stored on each route. The removed Analytics tab does not remove backend
aggregate-statistics computation.

The native API validates configuration before starting a session. Invalid
ASNs, IP addresses, ports, hold times and retry delays are rejected.

JSON output retains the existing behavior: it exports the current in-memory
RIB, uses periodic atomic replacement, and flushes when stopping. It does
not restore routes from disk on startup.

## Docker

```bash
docker build -t bgpx .
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

The multi-stage image contains the release executable and tcpdump, with no
Python runtime. To export routes, mount a writable directory and pass
`--json-output /data/routes.json`. Locale files are installed at
`/usr/local/locales`; mount a directory there to add or override translations.

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

## Reverse Proxy Examples

`deploy.sh` prints these examples using the selected WebUI port. The examples
below use port 8080 and plain HTTP on a private LAN. Point `bgpx.lan` to the
proxy's LAN IP using your DNS or hosts file, then open `http://bgpx.lan/`.
Use one proxy. These examples assume bgpx and the proxy run on the same host;
otherwise replace `127.0.0.1` with the bgpx host IP.

Serve the WebUI at `/`, not a subpath such as `/bgpx/`. Preserve the browser's
Host header including its port, and its original Referer. Do not synthesize
Referer or Sec-Fetch-Site headers: that bypasses the WebUI request check.

### Nginx

Add this server block inside the Nginx `http` block (often through
`/etc/nginx/conf.d/bgpx.conf`):

```nginx
server {
    listen 80;
    server_name bgpx.lan;
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $http_host;
        proxy_set_header Referer $http_referer;
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 1h;
    }
}
```

Validate before reloading: `sudo nginx -t`. Using `$http_host` preserves the
browser's port; disabling buffering keeps live events streaming. See the
[Nginx proxy documentation](https://nginx.org/en/docs/http/ngx_http_proxy_module.html).

### Caddy

Add this site block to your Caddyfile (usually `/etc/caddy/Caddyfile`):

```caddyfile
http://bgpx.lan {
    # Host (including port) and Referer are preserved; SSE flushes immediately.
    reverse_proxy 127.0.0.1:8080
}
```

Validate before reloading:
`sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile`.
Caddy preserves these headers for this HTTP upstream and streams SSE responses
immediately without extra buffering settings. See the
[Caddy reverse proxy documentation](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy).
The explicit `http://` keeps this LAN example on HTTP; use your normal TLS site
configuration if HTTPS is needed. HTTPS termination at the proxy is supported
when the original Host and Referer are preserved.

If the UI loads but data or controls return HTTP 403, check Host and Referer
forwarding. If live events arrive late, check buffering and proxy timeouts.
The header check remains spoofable and is not authentication. If a browser or
privacy extension strips Referer, UI API requests will also return 403. When
present, Sec-Fetch-Site must be same-origin; it may be absent on plain HTTP LANs.
Static files and locale dictionaries do not require these API headers.

## Verification

```bash
./test.sh
./target/release/bgpx --version
# Open http://127.0.0.1:8080/ in your browser
```

API endpoints require a WebUI-root Referer matching the request Host. Direct
requests (including `/health` probes) return 403. This spoofable header check
is a convenience restriction for trusted private LANs, not authentication.
Proxies must preserve Host and Referer. With accepted headers, `/health` returns
200 for an established BGP session and 503 otherwise.

`test.sh` runs Rust formatting, Clippy and tests, shell syntax checks and 324
parser compatibility fixtures. It also checks JavaScript syntax
when Node is present and runs both HTTP/SSE and installation smoke tests when
Python 3 is present. The request-check tests cover denied direct requests,
accepted WebUI requests, wrong-host referrers and cross-site metadata.

## Service and Uninstall

```bash
# Debian/Ubuntu and Red Hat-family hosts
sudo systemctl status bgpx
sudo journalctl -u bgpx -f
# Alpine instead
sudo rc-service bgpx status
sudo /opt/bgpx/uninstall.sh
```

The installed uninstaller automatically selects its own installation directory.
When running the source checkout's script, use `--install-dir DIR` for custom paths.
Only the matching service and command link are removed. Unrecognized directories
are refused even with `--force`; rerun the current installer for older deployments
to record ownership first. `--keep-data`
removes the service and command link while retaining the installation.
`--force` skips the interactive confirmation.
