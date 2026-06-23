# Installation Guide

## Prerequisites

### System Requirements
- **Python** 3.11 or later
- **pip** (Python package manager)
- **Linux/Unix** environment (macOS or Linux)
- **Root access** or capability to bind port 179 (for passive BGP listening)

### Network Requirements
- **BGP peer connectivity** — can reach your peer router on port 179
- **Port 179** — available for BGP (default, can be changed with `--listen-port`)

### Optional
- **tcpdump** — for packet capture feature in web UI
  ```bash
  # Ubuntu/Debian
  sudo apt-get install tcpdump
  
  # macOS
  brew install tcpdump
  ```

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

### Method 3: Docker

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

> **Note:** Requires `--cap-add=NET_BIND_SERVICE` or running as root.

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

Run on a non-privileged port:
```bash
bgpx --local-as 65001 --router-id 10.0.0.1 \
     --peer-ip 10.0.0.2 --peer-as 65000 \
     --listen-port 9179
```

Then configure your peer router to connect to port 9179.

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

Run with debug logging:
```bash
bgpx --log-level DEBUG
```

---

## Verification

### 1. Check Installation

```bash
bgpx --version
# or
bgpx --help
```

### 2. Start the Web UI

```bash
bgpx
```

Open `http://localhost:8080` in your browser. You should see:
- Configuration panel on the left
- Routes, Live Log, and Capture tabs
- "IDLE" state in the top-right badge

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
3. Use a custom port with `--listen-port`

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

# Or use a different port
bgpx --listen-port 9179
```

---

## Docker Advanced Usage

### Mount a volume for JSON output

```bash
docker run --rm -p 179:179 -p 8080:8080 \
  -v /tmp/routes:/data \
  -e "ARGS=--local-as 65001 --router-id 10.0.0.1 \
            --peer-ip 10.0.0.2 --peer-as 65000 \
            --json-output /data/routes.json" \
  bgpx
```

### Custom network

```bash
# Create a custom network for testing
docker network create bgpnet

# Run two containers (one as peer, one as receiver)
docker run --rm --network bgpnet --name receiver \
  -p 8080:8080 \
  -e "ARGS=--listen-port 179 --local-as 65001 \
            --router-id 10.0.0.1 --peer-ip 10.0.0.2 \
            --peer-as 65000" \
  bgpx
```

---

## Running as a System Service

### Using systemd (Linux)

Create `/etc/systemd/system/bgpx.service`:

```ini
[Unit]
Description=BGP Flowspec Receiver
After=network.target

[Service]
Type=simple
User=bgpx
WorkingDirectory=/var/lib/bgpx
ExecStart=/usr/local/bin/bgpx \
  --local-as 65001 \
  --router-id 10.0.0.1 \
  --peer-ip 10.0.0.2 \
  --peer-as 65000 \
  --json-output /var/lib/bgpx/routes.json \
  --log-level INFO
Restart=always
RestartSec=5

# Capabilities for port 179
AmbientCapabilities=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
```

Create bgpx user:
```bash
sudo useradd -r -s /bin/false bgpx
sudo mkdir -p /var/lib/bgpx
sudo chown bgpx:bgpx /var/lib/bgpx
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

## Next Steps

1. **Review Configuration** — see [README.md](README.md) for all flags and options
2. **Test with a Peer** — connect to your BGP peer router
3. **Monitor Routes** — check the web UI's Routes tab for received flowspec rules
4. **Export RIB** — use `--json-output` to persist routes to a file
5. **Set Up Monitoring** — integrate with your monitoring stack

---

## Support

- Check logs: `bgpx --log-level DEBUG`
- Review RFC compliance in [README.md](README.md#rfc-coverage)
- File issues on GitHub
