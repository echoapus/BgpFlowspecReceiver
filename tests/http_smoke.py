"""Exercise the standalone binary over HTTP/SSE and check graceful shutdown."""
import json
from pathlib import Path
import signal
import socket
import subprocess
import sys
import urllib.error
import urllib.request


def main():
    binary = str(Path(sys.argv[1]).resolve())
    process = subprocess.Popen([binary, "--host", "127.0.0.1", "--port", "0", "--log-level", "ERROR"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    events = None
    try:
        line = process.stderr.readline()
        assert line.startswith("Web UI: "), line
        base = line.removeprefix("Web UI: ").strip()

        def request(path, data=None, method=None):
            req = urllib.request.Request(base + path, data=data, method=method,
                                         headers={"Content-Type": "application/json"})
            try:
                response = urllib.request.urlopen(req, timeout=5)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                return response.status, response.read()

        status, page = request("/")
        assert status == 200 and b"routes-tbody" in page
        assert request("/health")[0] == 503
        assert request("/session/start", b"not json")[0] == 400
        assert request("/session/start", b"[]")[0] == 400
        assert request("/capture/start", b"{}")[0] == 400
        assert json.loads(request("/routes")[1])["count"] == 0
        assert json.loads(request("/routes/export")[1]) == {"routes": []}
        events = urllib.request.urlopen(base + "/events", timeout=5)
        assert events.headers["Content-Type"] == "text/event-stream"

        def event():
            while True:
                line = events.readline()
                assert line, "SSE ended unexpectedly"
                if line.startswith(b"data: "):
                    return json.loads(line[6:])

        assert event()["type"] == "snapshot"
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        config = {"local_as": "65001", "peer_as": "65000", "router_id": "192.0.2.1",
                  "peer_ip": "127.0.0.2", "listen_port": str(port), "hold_time": "0"}
        assert request("/session/start", json.dumps(config).encode())[0] == 200
        assert event()["running"] is True
        assert request("/session/stop", b"{}")[0] == 200
        assert request("/log", method="DELETE")[0] == 200
        # Leave SSE open: SIGTERM must close it and let the process exit.
        process.send_signal(signal.SIGTERM)
        assert process.wait(timeout=5) == 0
        print("Native binary HTTP/SSE/start-stop/shutdown smoke test passed")
    finally:
        if events:
            events.close()
        if process.poll() is None:
            process.kill()
            process.wait()


if __name__ == "__main__":
    main()
