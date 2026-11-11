"""Exercise installation and removal in a temporary directory, without system changes."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(script, *args, ok=True):
    env = {k: v for k, v in os.environ.items() if k not in ("INSTALL_DIR", "WEB_PORT")}
    result = subprocess.run(["bash", str(script), *map(str, args)], stdin=subprocess.DEVNULL,
                            capture_output=True, text=True, env=env)
    assert (result.returncode == 0) == ok, result.stdout + result.stderr
    return result


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="bgpx-install-") as tmp:
        home = Path(tmp) / "custom install"
        deploy = ["--binary", binary, "--install-dir", home,
                  "--no-service", "--no-link", "--web-port", "09090"]
        run(ROOT / "deploy.sh", *deploy)
        assert (home / "bin/bgpx").is_file()
        assert (home / ".bgpx-install").read_text().strip() == "bgpx-native-v1"
        source_locales = sorted(p.name for p in (ROOT / "web/locales").glob("*.json"))
        assert source_locales and sorted(p.name for p in (home / "locales").glob("*.json")) == source_locales
        data = home / "routes.json"
        data.write_text('{"routes":[]}')
        # Upgrade drops locales no longer shipped but keeps operator-added ones.
        (home / "locales/old.json").write_text("{}")
        with (home / ".bgpx-locales").open("a") as manifest:
            manifest.write("old.json\n")
        (home / "locales/custom.json").write_text('{"_name":"Custom"}')
        run(ROOT / "deploy.sh", *deploy)
        assert data.read_text() == '{"routes":[]}'
        assert not (home / "locales/old.json").exists()
        assert (home / "locales/custom.json").exists()
        assert all((home / "locales" / name).is_file() for name in source_locales)
        uninstall = home / "uninstall.sh"
        run(uninstall, ok=False)  # Noninteractive removal requires --force.
        run(uninstall, "--force", "--keep-data")
        assert data.exists() and (home / "bin/bgpx").exists()
        run(uninstall, "--force")  # Installed script infers the custom path.
        assert not home.exists()
        run(ROOT / "uninstall.sh", "--install-dir", home, "--force")

        other = Path(tmp) / "unrelated"
        other.mkdir()
        (other / "keep").write_text("keep")
        run(ROOT / "uninstall.sh", "--install-dir", other, "--force", ok=False)
        assert (other / "keep").exists()
        alias = Path(tmp) / "unsafe"
        alias.symlink_to("/usr/bin", target_is_directory=True)
        for unsafe in ("/tmp/..", alias, "/usr/local"):
            run(ROOT / "deploy.sh", "--install-dir", unsafe, "--no-service", "--no-link", ok=False)
            run(ROOT / "uninstall.sh", "--install-dir", unsafe, "--force", ok=False)
        run(ROOT / "deploy.sh", *deploy, "--web-port", "65536", ok=False)
        assert not home.exists()
    print("Install, upgrade, keep-data, uninstall and path-safety checks passed")


if __name__ == "__main__":
    main()
