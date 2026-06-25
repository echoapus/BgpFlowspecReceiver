"""In-memory Unicast and FlowSpec RIB with optional JSON persistence."""

import hashlib
import json
import logging
import os
from datetime import datetime, timezone
from typing import Optional

from bgpx.message.flowspec import normalize_nlri_components

log = logging.getLogger(__name__)


def _route_id(family: str, afi: str, peer: str, route: dict) -> str:
    """Deterministic ID scoped by family, AFI and peer."""
    canonical = json.dumps(
        [family, afi, peer, route],
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha1(canonical.encode()).hexdigest()[:12]


# ponytail: threading.Lock removed because bgpx is purely single-threaded asyncio
class FlowspecRIB:
    def __init__(self, json_output: Optional[str] = None):
        self._routes: dict[str, dict] = {}
        self._json_output = json_output

    # ── Public API ────────────────────────────────────────────────────────────

    def add_flowspec(
        self,
        afi: str,
        components: dict,
        actions: list,
        peer: str,
        path_attributes: Optional[list[dict]] = None,
    ) -> str:
        components = normalize_nlri_components(components)
        route_id = _route_id("flowspec", afi, peer, components)
        entry = {
            "id":          route_id,
            "family":      "flowspec",
            "afi":         afi,
            "peer":        peer,
            "received_at": datetime.now(timezone.utc).isoformat(),
            "match":       components,
            "actions":     actions,
        }
        if path_attributes is not None:
            entry["path_attributes"] = path_attributes
        is_new = route_id not in self._routes
        self._routes[route_id] = entry

        verb = "ADD" if is_new else "UPDATE"
        log.info(f"RIB {verb} [{afi}] id={route_id} peer={peer} match={components} actions={actions}")
        self._persist()
        return route_id

    def remove_flowspec(self, afi: str, components: dict, peer: str) -> Optional[str]:
        components = normalize_nlri_components(components)
        route_id = _route_id("flowspec", afi, peer, components)
        return self._remove_id(route_id)

    def add_unicast(
        self,
        afi: str,
        prefix: str,
        peer: str,
        next_hop: str = "",
        as_path: Optional[list[int]] = None,
        communities: Optional[list[str]] = None,
        path_attributes: Optional[list[dict]] = None,
    ) -> str:
        route_id = _route_id("unicast", afi, peer, {"prefix": prefix})
        entry = {
            "id": route_id,
            "family": "unicast",
            "afi": afi,
            "peer": peer,
            "received_at": datetime.now(timezone.utc).isoformat(),
            "prefix": prefix,
            "next_hop": next_hop,
            "as_path": as_path or [],
            "communities": communities or [],
        }
        if path_attributes is not None:
            entry["path_attributes"] = path_attributes
        self._routes[route_id] = entry
        log.info(f"RIB ADD [{afi}] id={route_id} peer={peer} prefix={prefix}")
        self._persist()
        return route_id

    def remove_unicast(self, afi: str, prefix: str, peer: str) -> Optional[str]:
        return self._remove_id(
            _route_id("unicast", afi, peer, {"prefix": prefix})
        )

    def clear_peer(self, peer: str) -> int:
        keys = [k for k, v in self._routes.items() if v["peer"] == peer]
        for k in keys:
            del self._routes[k]
        if keys:
            log.info(f"RIB cleared {len(keys)} route(s) from peer {peer}")
            self._persist()
        return len(keys)

    def all(self) -> list[dict]:
        return [self._normalized_route(r) for r in self._routes.values()]

    def by_afi(self, afi: str) -> list[dict]:
        return [
            self._normalized_route(r)
            for r in self._routes.values()
            if r["afi"] == afi
        ]

    def get(self, route_id: str) -> Optional[dict]:
        route = self._routes.get(route_id)
        return self._normalized_route(route) if route else None

    def clear_all(self) -> int:
        count = len(self._routes)
        self._routes.clear()
        if count:
            log.info(f"RIB cleared all {count} route(s)")
            self._persist()
        return count

    def set_json_output(self, path: Optional[str]) -> None:
        self._json_output = path

    def to_dict(self) -> dict:
        routes = [self._normalized_route(r) for r in self._routes.values()]
        return {"count": len(routes), "routes": routes}

    # ── Internal ──────────────────────────────────────────────────────────────

    def _normalized_route(self, route: dict) -> dict:
        normalized = dict(route)
        if route.get("family") == "flowspec" or "match" in route:
            normalized["match"] = normalize_nlri_components(route.get("match", {}))
        return normalized

    def _remove_id(self, route_id: str) -> Optional[str]:
        removed = self._routes.pop(route_id, None)
        if not removed:
            return None
        log.info(f"RIB DEL id={route_id}")
        self._persist()
        return route_id

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
