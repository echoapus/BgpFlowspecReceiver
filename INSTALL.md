# bgpx 26.176 Installation Guide

## Prerequisites

### System Requirements
- **Python** 3.11 or later
- **pip** (Python package manager)
- **Linux/Unix** environment (macOS or Linux)
- **Root access** or capability to bind port 179 (for passive BGP listening)

### Network Requirements
- **BGP peer connectivity** — can reach your peer router on port 179
- **Port 179** — available for active BGP and the default passive listener

### Optional
- **tcpdump** — for packet capture feature in web UI
  ```bash
  # Ubuntu/Debian
  sudo apt-get install tcpdump
  
  # macOS
  brew install tcpdump
  ```
- **Rust and Cargo** — required only for `deploy.sh --rust` and the full
  `test.sh` matrix

---

## Installation Methods

### Method 1: Install from PyPI (recommended)

```bash
pip install bgpx
```

Verify installation:
```bash
bgpx --help
```

### Method 2: Install from Source

Clone the repository:
```bash
git clone <repo-url>
cd BgpFlowspecReceiver
```

Install in editable mode:
```bash
pip install -e .
```

Verify installation:
```bash
bgpx --help
```

### Method 3: Deploy to /opt

The source tree includes `deploy.sh` for host installs. By default it installs the application under `/opt/bgpx`, creates a virtual environment at `/opt/bgpx/venv`, copies the source to `/opt/bgpx/app`, and links `bgpx` into `/usr/local/bin` when run as root.

Interactive install:
```bash
sudo ./deploy.sh
```

The script asks which Web UI port to bind. Press Enter to use `8080`.

After `pip install`, the script verifies that the installed package can load `bgpx/web/ui.html`. If package data is missing, deployment stops immediately instead of leaving a service that returns HTTP 500 for the Web UI.

Install the Rust parser and verify that Python loads the compiled FFI library:

```bash
sudo ./deploy.sh --rust
```

Re-running deployment without `--rust` removes any previously installed Rust
library and verifies that the Python parser is active.

Noninteractive install:
```bash
sudo ./deploy.sh --web-port 9090
```

Equivalent environment variable:
```bash
sudo WEB_PORT=9090 ./deploy.sh
```

Install and enable a systemd service:
```bash
sudo ./deploy.sh --service --web-port 8080
```

Allow the installed virtualenv Python to bind privileged ports such as BGP port 179:
```bash
sudo ./deploy.sh --cap-net-bind-service
```

Common combined production install:
```bash
sudo ./deploy.sh --rust --service --cap-net-bind-service --web-port 8080
```

After deployment:
```bash
/opt/bgpx/venv/bin/bgpx --host 0.0.0.0 --port 8080
```

Or, when run as root and linked into `/usr/local/bin`:
```bash
bgpx --host 0.0.0.0 --port 8080
```

Uninstall a deployment:
```bash
sudo ./uninstall.sh
```

Uninstall without an interactive prompt:
```bash
sudo ./uninstall.sh --force
```

### Method 4: Docker

Build the image:
```bash
docker build -t bgpx .
```

Run the container:
```bash
docker run --rm -p 8080:8080 bgpx
```

For BGP (port 179), add port mapping:
```bash
docker run --rm -p 179:179 -p 8080:8080 bgpx
```

The image runs as root by default. If you change it to a non-root user, add
`--cap-add=NET_BIND_SERVICE` or use a non-privileged passive listen port.
The provided Dockerfile installs the Python parser. Use host deployment with
`deploy.sh --rust` when the Rust FFI parser is required.

---

## Port 179 Configuration

BGP uses port 179 for connections. You have three options:

### Option A: Run with root (simplest, development only)

```bash
sudo bgpx --local-as 65001 --router-id 10.0.0.1 \
           --peer-ip 10.0.0.2 --peer-as 65000
```

### Option B: Use setcap (recommended for production)

Grant Python the capability to bind privileged ports:

```bash
sudo setcap cap_net_bind_service+ep $(readlink -f $(which python3))
```

Then run normally without `sudo`:
```bash
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000
```

Verify the capability:
```bash
getcap $(readlink -f $(which python3))
# Output: /usr/bin/python3 = cap_net_bind_service+ep
```

### Option C: Use a custom port

Start `bgpx`, set **Listen Port** in the Web UI to a non-privileged port such
as `9179`, then configure the peer router to connect to that port. Active
outbound connections still use TCP port 179.

---

## Development Setup

Install with development dependencies:

```bash
pip install -e ".[dev]"
```

Run tests:
```bash
pytest
```

Run Python fallback, shell/JavaScript checks, Rust fmt/Clippy/unit tests, and
the release Rust library through Python FFI:

```bash
./test.sh
```

Run with debug logging:
```bash
bgpx --log-level DEBUG
```

---

## Verification

### 1. Check Installation

```bash
bgpx --help
```

The installed package version is also available as:

```bash
python -c 'import bgpx; print(bgpx.__version__)'
```

### 2. Start the Web UI

```bash
bgpx
```

Open `http://localhost:8080` in your browser. You should see:
- Configuration panel on the left
- Total, Unicast, FlowSpec, Analytics, and Live Log tabs
- "IDLE" state in the top-right badge

If you installed with `deploy.sh`, open the Web UI port selected during deployment. For example, `--web-port 9090` means `http://localhost:9090`.

### 3. Configure and Start a Session

In the web UI:
1. Fill in **Local AS**, **Router ID**, **Peer IP**, **Peer AS**
2. Click **Start Session**
3. Watch the state change: `IDLE` → `CONNECT` → `OPEN_SENT` → `OPEN_CONFIRMED` → `ESTABLISHED`

Or via command line:

```bash
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000
```

---

## Common Issues

### Issue: "Cannot bind port 179"

**Symptom:** Error message in web UI: "Cannot bind port 179 — passive disabled"

**Solutions:**
1. Run with `sudo` (not recommended for production)
2. Use `setcap` (Option B above)
3. Set a non-privileged **Listen Port** in the Web UI

### Issue: Active connection fails, passive only works

**Symptom:** State shows `CONNECT` → `IDLE` loop; only accepts incoming

**Cause:** Firewall blocking outbound port 179 to peer

**Solution:** 
- Check firewall rules: `sudo ufw allow out 179`
- Ensure peer IP is reachable: `ping <peer-ip>`
- Verify BGP is enabled on peer router

### Issue: tcpdump errors in packet capture

**Symptom:** Capture button doesn't work or shows permission errors

**Solutions:**
```bash
# Install tcpdump
sudo apt-get install tcpdump

# Grant capability (preferred)
sudo setcap cap_net_raw+ep $(which tcpdump)

# Or run bgpx with sudo
sudo bgpx ...
```

### Issue: "Address already in use" for port 179

**Symptom:** Error when starting bgpx

**Solutions:**
```bash
# Find what's using port 179
sudo lsof -i :179

# Kill the process (if it's an old bgpx instance)
sudo kill -9 <PID>

# Or set a different passive Listen Port in the Web UI
```

### Issue: installed parser engine does not match deployment selection

Re-run the current `deploy.sh`. It locates the installed `bgpx.message`
directory directly, removes stale Rust libraries from both `purelib` and
`platlib`, and verifies imports in Python isolated mode:

```bash
sudo ./deploy.sh --rust
```

To switch back to Python and remove the installed `.so`:

```bash
sudo ./deploy.sh
```

---

## Docker Advanced Usage

### Mount a volume for JSON output

```bash
docker run --rm -p 179:179 -p 8080:8080 \
  -v /tmp/routes:/data \
  bgpx \
  --host 0.0.0.0 --port 8080 \
  --local-as 65001 --router-id 10.0.0.1 \
  --peer-ip 10.0.0.2 --peer-as 65000 \
  --json-output /data/routes.json
```

### Custom network

```bash
# Create a custom network for testing
docker network create bgpnet

# Run two containers (one as peer, one as receiver)
docker run --rm --network bgpnet --name receiver \
  -p 8080:8080 \
  bgpx \
  --host 0.0.0.0 --port 8080 \
  --local-as 65001 \
  --router-id 10.0.0.1 --peer-ip 10.0.0.2 \
  --peer-as 65000
```

---

## Running as a System Service

### Generated systemd Service

The recommended systemd path is to let `deploy.sh` generate the unit:

```bash
sudo ./deploy.sh --service --cap-net-bind-service --web-port 8080
sudo systemctl start bgpx
sudo systemctl status bgpx
```

The generated service uses `/opt/bgpx/app` as the working directory and starts:

```bash
/opt/bgpx/venv/bin/bgpx --host 0.0.0.0 --port <selected-web-port>
```

### Manual systemd Service

Create `/etc/systemd/system/bgpx.service`:

```ini
[Unit]
Description=BGP Unicast and FlowSpec Receiver
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/bgpx/app
ExecStart=/opt/bgpx/venv/bin/bgpx --host 0.0.0.0 --port 8080
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

To bind BGP port 179 without running the process as root, grant the installed Python binary the capability:

```bash
sudo setcap cap_net_bind_service+ep $(readlink -f /opt/bgpx/venv/bin/python)
```

Enable and start:
```bash
sudo systemctl daemon-reload
sudo systemctl enable bgpx
sudo systemctl start bgpx
sudo systemctl status bgpx
```

View logs:
```bash
sudo journalctl -u bgpx -f
```

---

## Uninstall

Use `uninstall.sh` to remove a deployment created by `deploy.sh`:

```bash
sudo ./uninstall.sh
```

The script shows a removal plan and asks for confirmation. It stops and disables `bgpx.service` when present, removes `/etc/systemd/system/bgpx.service`, removes `/usr/local/bin/bgpx` only when it points into the selected install directory, and removes `/opt/bgpx`. It does not modify the source checkout or local Cargo build cache.

Noninteractive uninstall:

```bash
sudo ./uninstall.sh --force
```

Remove service and command link but keep the install directory:

```bash
sudo ./uninstall.sh --keep-data
```

Remove a custom install path:

```bash
sudo ./uninstall.sh --install-dir /opt/custom-bgpx
```

---

## Next Steps

1. **Review Configuration** — see [README.md](README.md) for all flags and options
2. **Test with a Peer** — connect to your BGP peer router
3. **Monitor Routes** — use Total, Unicast, and FlowSpec tabs
4. **Export RIB** — use `--json-output` to persist routes to a file
5. **Set Up Monitoring** — integrate with your monitoring stack

---

## Support

- Check logs: `bgpx --log-level DEBUG`
- Review RFC compliance in [README.md](README.md#rfc-coverage)
- File issues on GitHub

### Issue: Web UI returns HTTP 500 after pip install or deploy

**Symptom:** The API starts, but opening `/` fails because `bgpx/web/ui.html` is missing from the installed package.

**Solutions:**
1. Use a version whose `pyproject.toml` includes `bgpx = ["web/*.html"]` under `[tool.setuptools.package-data]`.
2. Re-run `sudo ./deploy.sh`; deployment now verifies the installed package data before completing.
3. For manual installs, run:
   ```bash
   python - <<'PY'
   from importlib import resources
   ui = resources.files("bgpx").joinpath("web", "ui.html")
   print(ui.is_file(), ui)
   PY
   ```
