"""In-memory Unicast and FlowSpec RIB with optional JSON persistence."""

import asyncio
import hashlib
import json
import logging
import os
from collections import Counter
from datetime import datetime, timezone
from itertools import islice

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
    def __init__(self, json_output: str | None = None):
        self._routes: dict[str, dict] = {}
        self._json_output = json_output
        self._persist_handle: asyncio.TimerHandle | None = None
        self._counts = Counter()
        self._analytics = {
            name: Counter()
            for name in (
                "communities", "origin_as", "next_hops", "prefix_lengths",
                "actions", "protocols", "ports",
            )
        }
        self._changes = 0

    # ── Public API ────────────────────────────────────────────────────────────

    def add_flowspec(
        self,
        afi: str,
        components: dict,
        actions: list,
        peer: str,
        path_attributes: list[dict] | None = None,
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
        old = self._routes.pop(route_id, None)
        if old:
            self._remove_stats(old)
        self._routes[route_id] = entry
        self._add_stats(entry)

        log.debug(
            "RIB %s [%s] id=%s peer=%s match=%s actions=%s",
            "UPDATE" if old else "ADD", afi, route_id, peer, components, actions,
        )
        self._changed()
        return route_id

    def remove_flowspec(self, afi: str, components: dict, peer: str) -> str | None:
        components = normalize_nlri_components(components)
        route_id = _route_id("flowspec", afi, peer, components)
        return self._remove_id(route_id)

    def add_unicast(
        self,
        afi: str,
        prefix: str,
        peer: str,
        next_hop: str = "",
        as_path: list[int] | None = None,
        communities: list[str] | None = None,
        path_attributes: list[dict] | None = None,
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
        old = self._routes.pop(route_id, None)
        if old:
            self._remove_stats(old)
        self._routes[route_id] = entry
        self._add_stats(entry)
        log.debug("RIB %s [%s] id=%s peer=%s prefix=%s",
                  "UPDATE" if old else "ADD", afi, route_id, peer, prefix)
        self._changed()
        return route_id

    def remove_unicast(self, afi: str, prefix: str, peer: str) -> str | None:
        return self._remove_id(
            _route_id("unicast", afi, peer, {"prefix": prefix})
        )

    def clear_peer(self, peer: str) -> int:
        keys = [k for k, v in self._routes.items() if v["peer"] == peer]
        for k in keys:
            self._remove_stats(self._routes.pop(k))
        if keys:
            log.info(f"RIB cleared {len(keys)} route(s) from peer {peer}")
            self._changed()
        return len(keys)

    def all(self) -> list[dict]:
        return [self._normalized_route(r) for r in self._routes.values()]

    def by_afi(self, afi: str) -> list[dict]:
        return [
            self._normalized_route(r)
            for r in self._routes.values()
            if r["afi"] == afi
        ]

    def get(self, route_id: str) -> dict | None:
        route = self._routes.get(route_id)
        return self._normalized_route(route) if route else None

    def clear_all(self) -> int:
        count = len(self._routes)
        self._routes.clear()
        self._counts.clear()
        for values in self._analytics.values():
            values.clear()
        if count:
            log.info(f"RIB cleared all {count} route(s)")
            self._changed()
        return count

    def set_json_output(self, path: str | None) -> None:
        self._json_output = path

    def to_dict(self) -> dict:
        routes = [self._normalized_route(r) for r in self._routes.values()]
        return {"count": len(routes), "routes": routes}

    def stats(self) -> dict:
        return {
            "total": len(self._routes),
            "unicast": self._counts["unicast"],
            "flowspec": self._counts["flowspec"],
            "ipv4": self._counts["ipv4"],
            "ipv6": self._counts["ipv6"],
            "analytics": {
                name: values.most_common(5)
                for name, values in self._analytics.items()
            },
        }

    def page(
        self,
        family: str = "total",
        page: int = 1,
        page_size: int = 50,
        sort: str = "received_at",
        ascending: bool = False,
    ) -> dict:
        allowed_sort = {"id", "family", "afi", "prefix", "next_hop", "peer", "received_at"}
        if sort not in allowed_sort:
            sort = "received_at"

        values = self._routes.values()
        if sort == "received_at":
            ordered = values if ascending else reversed(values)
            routes = (
                route for route in ordered
                if family == "total" or route["family"] == family
            )
            total = len(self._routes) if family == "total" else self._counts[family]
        else:
            routes = sorted(
                (
                    route for route in values
                    if family == "total" or route["family"] == family
                ),
                key=lambda route: str(route.get(sort, "")).lower(),
                reverse=not ascending,
            )
            total = len(routes)
        page = max(1, page)
        page_size = min(500, max(1, page_size))
        start = (page - 1) * page_size
        return {
            "page": page,
            "page_size": page_size,
            "count": total,
            "routes": [
                self._normalized_route(route)
                for route in islice(routes, start, start + page_size)
            ],
            "stats": self.stats(),
        }

    def iter_routes(self, family: str = "total"):
        for route in self._routes.values():
            if family == "total" or route["family"] == family:
                yield self._normalized_route(route)

    def flush(self) -> None:
        if self._persist_handle:
            self._persist_handle.cancel()
            self._persist_handle = None
        self._persist()

    # ── Internal ──────────────────────────────────────────────────────────────

    def _normalized_route(self, route: dict) -> dict:
        normalized = dict(route)
        if route.get("family") == "flowspec" or "match" in route:
            normalized["match"] = normalize_nlri_components(route.get("match", {}))
        return normalized

    def _remove_id(self, route_id: str) -> str | None:
        removed = self._routes.pop(route_id, None)
        if not removed:
            return None
        self._remove_stats(removed)
        log.debug("RIB DEL id=%s", route_id)
        self._changed()
        return route_id

    def _changed(self) -> None:
        self._changes += 1
        if self._changes % 10_000 == 0:
            log.info("RIB contains %s routes", f"{len(self._routes):,}")
        if not self._json_output:
            return
        if self._persist_handle:
            self._persist_handle.cancel()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            self._persist()
            return
        # ponytail: one full JSON write after a burst, not one per route
        self._persist_handle = loop.call_later(5, self.flush)

    def _add_stats(self, route: dict) -> None:
        family = route["family"]
        self._counts[family] += 1
        self._counts["ipv6" if str(route["afi"]).startswith("ipv6") else "ipv4"] += 1
        for name, values in self._route_metrics(route).items():
            self._analytics[name].update(values)

    def _remove_stats(self, route: dict) -> None:
        family = route["family"]
        self._counts.subtract([family])
        self._counts.subtract([
            "ipv6" if str(route["afi"]).startswith("ipv6") else "ipv4"
        ])
        self._counts += Counter()
        for name, values in self._route_metrics(route).items():
            self._analytics[name].subtract(values)
            self._analytics[name] += Counter()

    def _route_metrics(self, route: dict) -> dict[str, list]:
        metrics = {name: [] for name in self._analytics}
        if route["family"] == "unicast":
            metrics["communities"] = route.get("communities", [])
            path = route.get("as_path", [])
            if path:
                metrics["origin_as"] = [str(path[-1])]
            if route.get("next_hop"):
                metrics["next_hops"] = [route["next_hop"]]
            prefix_length = str(route.get("prefix", "")).partition("/")[2]
            if prefix_length:
                metrics["prefix_lengths"] = [f"/{prefix_length}"]
        else:
            match = route.get("match", {})
            metrics["actions"] = route.get("actions", [])
            metrics["protocols"] = match.get("ip-proto", [])
            metrics["ports"] = (
                match.get("port", [])
                + match.get("src-port", [])
                + match.get("dst-port", [])
            )
        return metrics

    def _persist(self):
        if not self._json_output:
            return
        tmp = self._json_output + ".tmp"
        try:
            with open(tmp, "w") as f:
                f.write(f'{{"count":{len(self._routes)},"routes":[')
                for index, route in enumerate(self.iter_routes()):
                    if index:
                        f.write(",")
                    json.dump(route, f, separators=(",", ":"))
                f.write("]}")
            os.replace(tmp, self._json_output)  # atomic rename
        except OSError as e:
            log.error(f"Failed to write {self._json_output}: {e}")
