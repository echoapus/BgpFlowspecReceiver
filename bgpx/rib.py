"""In-memory Flowspec RIB with optional JSON file persistence."""

import hashlib
import json
import logging
import os
import threading
from datetime import datetime, timezone
from typing import Optional

from bgpx.message.flowspec import normalize_nlri_components

log = logging.getLogger(__name__)


def _route_id(components: dict) -> str:
    """Deterministic 12-char ID derived from a route's match components."""
    canonical = json.dumps(components, sort_keys=True, separators=(",", ":"))
    return hashlib.sha1(canonical.encode()).hexdigest()[:12]


class FlowspecRIB:
    def __init__(self, json_output: Optional[str] = None):
        self._routes: dict[str, dict] = {}
        self._lock = threading.Lock()
        self._json_output = json_output

    # ── Public API ────────────────────────────────────────────────────────────

    def add(
        self,
        afi: str,
        components: dict,
        actions: list,
        peer: str,
        path_attributes: Optional[list[dict]] = None,
    ) -> str:
        components = normalize_nlri_components(components)
        route_id = _route_id(components)
        entry = {
            "id":          route_id,
            "afi":         afi,
            "peer":        peer,
            "received_at": datetime.now(timezone.utc).isoformat(),
            "match":       components,
            "actions":     actions,
        }
        if path_attributes is not None:
            entry["path_attributes"] = path_attributes
        with self._lock:
            is_new = route_id not in self._routes
            self._routes[route_id] = entry

        verb = "ADD" if is_new else "UPDATE"
        log.info(f"RIB {verb} [{afi}] id={route_id} peer={peer} match={components} actions={actions}")
        self._persist()
        return route_id

    def remove(self, components: dict) -> Optional[str]:
        """Remove a route and return its id, or None if it was not present."""
        components = normalize_nlri_components(components)
        route_id = _route_id(components)
        with self._lock:
            removed = self._routes.pop(route_id, None)
        if removed:
            log.info(f"RIB DEL id={route_id} match={components}")
            self._persist()
            return route_id
        return None

    def clear_peer(self, peer: str) -> int:
        with self._lock:
            keys = [k for k, v in self._routes.items() if v["peer"] == peer]
            for k in keys:
                del self._routes[k]
        if keys:
            log.info(f"RIB cleared {len(keys)} route(s) from peer {peer}")
            self._persist()
        return len(keys)

    def all(self) -> list[dict]:
        with self._lock:
            return [self._normalized_route(r) for r in self._routes.values()]

    def by_afi(self, afi: str) -> list[dict]:
        with self._lock:
            return [
                self._normalized_route(r)
                for r in self._routes.values()
                if r["afi"] == afi
            ]

    def get(self, route_id: str) -> Optional[dict]:
        with self._lock:
            route = self._routes.get(route_id)
            return self._normalized_route(route) if route else None

    def clear_all(self) -> int:
        with self._lock:
            count = len(self._routes)
            self._routes.clear()
        if count:
            log.info(f"RIB cleared all {count} route(s)")
            self._persist()
        return count

    def set_json_output(self, path: Optional[str]) -> None:
        self._json_output = path

    def to_dict(self) -> dict:
        with self._lock:
            routes = [self._normalized_route(r) for r in self._routes.values()]
        return {"count": len(routes), "routes": routes}

    # ── Internal ──────────────────────────────────────────────────────────────

    def _normalized_route(self, route: dict) -> dict:
        normalized = dict(route)
        normalized["match"] = normalize_nlri_components(route.get("match", {}))
        return normalized

    def _persist(self):
        if not self._json_output:
            return
        data = self.to_dict()
        tmp = self._json_output + ".tmp"
        try:
            with open(tmp, "w") as f:
                json.dump(data, f, indent=2)
            os.replace(tmp, self._json_output)  # atomic rename
        except OSError as e:
            log.error(f"Failed to write {self._json_output}: {e}")
