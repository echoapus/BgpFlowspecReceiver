"""Parse incoming BGP messages (RFC 4271)."""

import ctypes
import ipaddress
import json
import os
import socket
import struct

from bgpx.constants import (
    BGP_MARKER, BGP_HEADER_LEN,
    ATTR_ORIGIN, ATTR_AS_PATH, ATTR_NEXT_HOP, ATTR_MED, ATTR_LOCAL_PREF,
    ATTR_ATOMIC_AGGREGATE, ATTR_AGGREGATOR, ATTR_COMMUNITIES,
    ATTR_ORIGINATOR_ID, ATTR_CLUSTER_LIST,
    ATTR_MP_REACH_NLRI, ATTR_MP_UNREACH_NLRI, ATTR_EXT_COMMUNITIES,
    ATTR_AS4_PATH, ATTR_AS4_AGGREGATOR, ATTR_IPV6_EXT_COMMUNITIES,
    ATTR_LARGE_COMMUNITIES,
    AFI_IPV4, AFI_IPV6, SAFI_FLOWSPEC,
)
from bgpx.message.flowspec import (
    parse_nlri_list, parse_ext_communities, parse_ipv6_ext_communities,
)

# ponytail: load Rust parser library if available, with transparent fallback
_lib = None
_lib_path = os.path.join(os.path.dirname(__file__), "libbgpx_rust.so")
if os.path.exists(_lib_path):
    try:
        _lib = ctypes.CDLL(_lib_path)
        _lib.parse_header_rust.argtypes = [
            ctypes.c_void_p, ctypes.c_size_t,
            ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(ctypes.c_uint32)
        ]
        _lib.parse_header_rust.restype = ctypes.c_int32

        _lib.parse_open_rust.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        _lib.parse_open_rust.restype = ctypes.c_void_p

        _lib.parse_update_details_rust.argtypes = [
            ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t,
        ]
        _lib.parse_update_details_rust.restype = ctypes.c_void_p

        _lib.free_string.argtypes = [ctypes.c_void_p]
        _lib.free_string.restype = None
    except Exception:
        _lib = None


PATH_ATTR_NAMES = {
    ATTR_ORIGIN: "ORIGIN",
    ATTR_AS_PATH: "AS_PATH",
    ATTR_NEXT_HOP: "NEXT_HOP",
    ATTR_MED: "MULTI_EXIT_DISC",
    ATTR_LOCAL_PREF: "LOCAL_PREF",
    ATTR_ATOMIC_AGGREGATE: "ATOMIC_AGGREGATE",
    ATTR_AGGREGATOR: "AGGREGATOR",
    ATTR_COMMUNITIES: "COMMUNITIES",
    ATTR_ORIGINATOR_ID: "ORIGINATOR_ID",
    ATTR_CLUSTER_LIST: "CLUSTER_LIST",
    ATTR_MP_REACH_NLRI: "MP_REACH_NLRI",
    ATTR_MP_UNREACH_NLRI: "MP_UNREACH_NLRI",
    ATTR_EXT_COMMUNITIES: "EXTENDED_COMMUNITIES",
    ATTR_AS4_PATH: "AS4_PATH",
    ATTR_AS4_AGGREGATOR: "AS4_AGGREGATOR",
    ATTR_IPV6_EXT_COMMUNITIES: "IPV6_ADDRESS_SPECIFIC_EXTENDED_COMMUNITIES",
    ATTR_LARGE_COMMUNITIES: "LARGE_COMMUNITIES",
}

ORIGIN_NAMES = {
    0: "igp",
    1: "egp",
    2: "incomplete",
}

AS_PATH_SEGMENT_NAMES = {
    1: "AS_SET",
    2: "AS_SEQUENCE",
    3: "AS_CONFED_SEQUENCE",
    4: "AS_CONFED_SET",
}


def parse_header(data: bytes) -> tuple[int, int]:
    """Return (msg_type, body_length). Raise ValueError on invalid marker."""
    if _lib is not None:
        msg_type = ctypes.c_uint8(0)
        body_length = ctypes.c_uint32(0)
        res = _lib.parse_header_rust(data, len(data), ctypes.byref(msg_type), ctypes.byref(body_length))
        if res == 0:
            return int(msg_type.value), int(body_length.value)
        elif res == -1:
            raise ValueError("Header too short")
        elif res == -2:
            raise ValueError("Invalid BGP marker")
        elif res == -3:
            raise ValueError(f"BGP message length below minimum 19")

    if len(data) < BGP_HEADER_LEN:
        raise ValueError("Header too short")
    if data[:16] != BGP_MARKER:
        raise ValueError("Invalid BGP marker")
    length, msg_type = struct.unpack("!HB", data[16:19])
    if length < BGP_HEADER_LEN:
        raise ValueError(f"BGP message length {length} below minimum {BGP_HEADER_LEN}")
    return msg_type, length - BGP_HEADER_LEN


def parse_open(body: bytes) -> dict:
    """Parse a BGP OPEN message body and return a dict with peer info."""
    if _lib is not None:
        parsed = _rust_json(_lib.parse_open_rust(body, len(body)))
        if parsed is not None:
            return parsed

    version = body[0]
    peer_as = struct.unpack("!H", body[1:3])[0]
    hold_time = struct.unpack("!H", body[3:5])[0]
    router_id = socket.inet_ntoa(body[5:9])
    supports_4byte_asn = False

    # Parse optional parameters for 4-byte ASN capability (RFC 6793)
    # ponytail: parse 4-octet ASN capability if present in OPEN optional parameters
    if len(body) > 10:
        opt_len = body[9]
        offset = 10
        end = 10 + opt_len
        while offset + 2 <= end and offset + 2 <= len(body):
            param_type = body[offset]
            param_len = body[offset + 1]
            offset += 2
            if param_type == 2:  # Capabilities (RFC 5492)
                cap_offset = offset
                cap_end = offset + param_len
                while cap_offset + 2 <= cap_end and cap_offset + 2 <= len(body):
                    cap_code = body[cap_offset]
                    cap_len = body[cap_offset + 1]
                    cap_offset += 2
                    if cap_code == 65 and cap_len == 4 and cap_offset + 4 <= len(body):
                        supports_4byte_asn = True
                        peer_as = struct.unpack("!I", body[cap_offset:cap_offset + 4])[0]
                    cap_offset += cap_len
            offset += param_len

    return {
        "version":   version,
        "peer_as":   peer_as,
        "hold_time": hold_time,
        "router_id": router_id,
        "supports_4byte_asn": supports_4byte_asn,
    }


def parse_update(body: bytes) -> tuple[dict, dict, list]:
    """Parse a BGP UPDATE message body.

    Returns:
        announce  – {afi_label: [route_dicts]}  routes being advertised
        withdraw  – {afi_label: [route_dicts]}  routes being withdrawn
        actions   – [str]  flowspec extended-community actions from this update
    """
    details = parse_update_details(body)
    return details["announce"], details["withdraw"], details["actions"]


def parse_update_details(body: bytes, asn_len: int = 2) -> dict:
    """Parse a BGP UPDATE and retain decoded/raw path-attribute metadata."""
    if _lib is not None:
        parsed = _rust_json(
            _lib.parse_update_details_rust(body, len(body), asn_len)
        )
        if parsed is not None:
            return parsed

    offset = 0

    if offset + 2 > len(body):
        raise ValueError("UPDATE too short for withdrawn-routes length field")
    withdrawn_len = struct.unpack("!H", body[offset:offset + 2])[0]
    offset += 2
    if offset + withdrawn_len > len(body):
        raise ValueError(f"UPDATE withdrawn_len {withdrawn_len} exceeds message body")
    withdrawn_payload = body[offset:offset + withdrawn_len]
    offset += withdrawn_len

    # Walk path attributes
    if offset + 2 > len(body):
        raise ValueError("UPDATE too short for path-attributes length field")
    attr_len = struct.unpack("!H", body[offset:offset + 2])[0]
    offset += 2
    attr_end = offset + attr_len
    if attr_end > len(body):
        raise ValueError(f"UPDATE attr_len {attr_len} exceeds message body")

    announce: dict = {}
    withdraw: dict = {}
    actions:  list = []
    path_attributes: list[dict] = []

    while offset < attr_end:
        if offset + 2 > attr_end:
            raise ValueError("Truncated path-attribute header")
        flags = body[offset]
        atype = body[offset + 1]
        offset += 2

        # Extended-length flag means the length field is 2 bytes instead of 1
        if flags & 0x10:
            if offset + 2 > attr_end:
                raise ValueError("Truncated extended path-attribute length")
            alen = struct.unpack("!H", body[offset:offset + 2])[0]
            offset += 2
        else:
            if offset >= attr_end:
                raise ValueError("Truncated path-attribute length")
            alen = body[offset]
            offset += 1

        if offset + alen > attr_end:
            raise ValueError("Path attribute exceeds declared attribute length")
        abody  = body[offset:offset + alen]
        offset += alen

        attr_info = _decode_path_attribute(flags, atype, abody, asn_len)
        path_attributes.append(attr_info)

        if atype == ATTR_MP_REACH_NLRI and len(abody) > 3:
            afi  = struct.unpack("!H", abody[0:2])[0]
            safi = abody[2]
            if safi == SAFI_FLOWSPEC and afi in (AFI_IPV4, AFI_IPV6):
                nh_len     = abody[3]
                # Skip next-hop bytes + 1 reserved SNPA byte
                nlri_start = 4 + nh_len + 1
                if nlri_start > len(abody):
                    raise ValueError("MP_REACH next-hop exceeds attribute length")
                label      = "ipv6-flowspec" if afi == AFI_IPV6 else "ipv4-flowspec"
                announce[label] = parse_nlri_list(abody[nlri_start:], afi)
            elif safi == 1 and afi in (AFI_IPV4, AFI_IPV6):
                nh_len = abody[3]
                nlri_start = 4 + nh_len + 1
                if nlri_start > len(abody):
                    raise ValueError("MP_REACH next-hop exceeds attribute length")
                next_hop = _format_next_hop(abody[4:4 + nh_len])
                label = "ipv6-unicast" if afi == AFI_IPV6 else "ipv4-unicast"
                announce[label] = [
                    {"prefix": prefix, "next_hop": next_hop}
                    for prefix in _parse_unicast_nlri(abody[nlri_start:], afi)
                ]

        elif atype == ATTR_MP_UNREACH_NLRI and len(abody) > 2:
            afi  = struct.unpack("!H", abody[0:2])[0]
            safi = abody[2]
            if safi == SAFI_FLOWSPEC and afi in (AFI_IPV4, AFI_IPV6):
                label      = "ipv6-flowspec" if afi == AFI_IPV6 else "ipv4-flowspec"
                withdraw[label] = parse_nlri_list(abody[3:], afi)
            elif safi == 1 and afi in (AFI_IPV4, AFI_IPV6):
                label = "ipv6-unicast" if afi == AFI_IPV6 else "ipv4-unicast"
                withdraw[label] = [
                    {"prefix": prefix}
                    for prefix in _parse_unicast_nlri(abody[3:], afi)
                ]

        elif atype == ATTR_EXT_COMMUNITIES:
            actions.extend(parse_ext_communities(abody))

        elif atype == ATTR_IPV6_EXT_COMMUNITIES:
            actions.extend(parse_ipv6_ext_communities(abody))

    if withdrawn_payload:
        withdraw["ipv4-unicast"] = [
            {"prefix": prefix}
            for prefix in _parse_unicast_nlri(withdrawn_payload, AFI_IPV4)
        ]

    trailing_nlri = body[attr_end:]
    if trailing_nlri:
        next_hop = next(
            (
                attribute.get("value")
                for attribute in path_attributes
                if attribute["code"] == ATTR_NEXT_HOP
            ),
            "",
        )
        announce["ipv4-unicast"] = [
            {"prefix": prefix, "next_hop": next_hop}
            for prefix in _parse_unicast_nlri(trailing_nlri, AFI_IPV4)
        ]

    return {
        "announce": announce,
        "withdraw": withdraw,
        "actions": actions,
        "path_attributes": path_attributes,
    }


def _rust_json(ptr) -> dict | None:
    if not ptr:
        return None
    try:
        return json.loads(ctypes.string_at(ptr).decode("utf-8"))
    finally:
        _lib.free_string(ptr)


def _parse_unicast_nlri(payload: bytes, afi: int) -> list[str]:
    max_bits = 128 if afi == AFI_IPV6 else 32
    addr_size = 16 if afi == AFI_IPV6 else 4
    address_type = ipaddress.IPv6Address if afi == AFI_IPV6 else ipaddress.IPv4Address
    prefixes: list[str] = []
    offset = 0

    while offset < len(payload):
        prefix_len = payload[offset]
        offset += 1
        if prefix_len > max_bits:
            raise ValueError(f"Invalid prefix length {prefix_len} for AFI {afi}")
        byte_len = (prefix_len + 7) // 8
        if offset + byte_len > len(payload):
            raise ValueError("Truncated unicast NLRI")
        raw = payload[offset:offset + byte_len]
        offset += byte_len
        address = address_type(raw.ljust(addr_size, b"\x00"))
        prefixes.append(str(ipaddress.ip_network(f"{address}/{prefix_len}", strict=False)))

    return prefixes


def _decode_path_attribute(
    flags: int, atype: int, data: bytes, asn_len: int = 2
) -> dict:
    info = {
        "code": atype,
        "name": PATH_ATTR_NAMES.get(atype, f"ATTR_{atype}"),
        "flags": {
            "optional": bool(flags & 0x80),
            "transitive": bool(flags & 0x40),
            "partial": bool(flags & 0x20),
            "extended_length": bool(flags & 0x10),
        },
        "length": len(data),
    }

    try:
        decoded = _decode_path_attribute_value(atype, data, asn_len)
    except (ValueError, struct.error, OSError):
        decoded = None

    if decoded is None:
        info["raw"] = data.hex()
    else:
        info["value"] = decoded
    return info


def _decode_path_attribute_value(atype: int, data: bytes, asn_len: int = 2):
    if atype == ATTR_ORIGIN and len(data) >= 1:
        return ORIGIN_NAMES.get(data[0], f"unknown-{data[0]}")

    if atype == ATTR_AS_PATH:
        return _parse_as_path(data, asn_len)

    if atype == ATTR_NEXT_HOP and len(data) == 4:
        return socket.inet_ntoa(data)

    if atype in (ATTR_MED, ATTR_LOCAL_PREF) and len(data) == 4:
        return struct.unpack("!I", data)[0]

    if atype == ATTR_ATOMIC_AGGREGATE and len(data) == 0:
        return True

    if atype == ATTR_AGGREGATOR and len(data) == 6:
        return {
            "asn": struct.unpack("!H", data[0:2])[0],
            "router_id": socket.inet_ntoa(data[2:6]),
        }

    if atype == ATTR_COMMUNITIES:
        return _parse_communities(data)

    if atype == ATTR_ORIGINATOR_ID and len(data) == 4:
        return socket.inet_ntoa(data)

    if atype == ATTR_CLUSTER_LIST and len(data) % 4 == 0:
        return [socket.inet_ntoa(data[i:i + 4]) for i in range(0, len(data), 4)]

    if atype in (ATTR_MP_REACH_NLRI, ATTR_MP_UNREACH_NLRI) and len(data) >= 3:
        return _parse_mp_attribute(data, atype)

    if atype == ATTR_EXT_COMMUNITIES:
        return parse_ext_communities(data)

    if atype == ATTR_AS4_PATH:
        return _parse_as_path(data, 4)

    if atype == ATTR_AS4_AGGREGATOR and len(data) == 8:
        return {
            "asn": struct.unpack("!I", data[0:4])[0],
            "router_id": socket.inet_ntoa(data[4:8]),
        }

    if atype == ATTR_IPV6_EXT_COMMUNITIES:
        return parse_ipv6_ext_communities(data)

    if atype == ATTR_LARGE_COMMUNITIES:
        return _parse_large_communities(data)

    return None


def _parse_as_path(data: bytes, asn_len: int) -> list[dict]:
    path: list[dict] = []
    offset = 0
    while offset + 2 <= len(data):
        seg_type = data[offset]
        seg_len = data[offset + 1]
        offset += 2
        byte_len = seg_len * asn_len
        if offset + byte_len > len(data):
            raise ValueError("AS_PATH segment exceeds attribute length")
        asns = [
            int.from_bytes(data[i:i + asn_len], "big")
            for i in range(offset, offset + byte_len, asn_len)
        ]
        offset += byte_len
        path.append({
            "type": AS_PATH_SEGMENT_NAMES.get(seg_type, f"SEGMENT_{seg_type}"),
            "asns": asns,
        })
    if offset != len(data):
        raise ValueError("Trailing AS_PATH data")
    return path


def _parse_communities(data: bytes) -> list[str]:
    if len(data) % 4:
        raise ValueError("COMMUNITIES length is not a multiple of 4")
    communities: list[str] = []
    well_known = {
        0xFFFFFF01: "NO_EXPORT",
        0xFFFFFF02: "NO_ADVERTISE",
        0xFFFFFF03: "NO_EXPORT_SUBCONFED",
        0xFFFFFF04: "NOPEER",
    }
    for i in range(0, len(data), 4):
        value = struct.unpack("!I", data[i:i + 4])[0]
        if value in well_known:
            communities.append(well_known[value])
        else:
            communities.append(
                f"{struct.unpack('!H', data[i:i + 2])[0]}:"
                f"{struct.unpack('!H', data[i + 2:i + 4])[0]}"
            )
    return communities


def _parse_large_communities(data: bytes) -> list[str]:
    if len(data) % 12:
        raise ValueError("LARGE_COMMUNITIES length is not a multiple of 12")
    communities: list[str] = []
    for i in range(0, len(data), 12):
        ga, ld1, ld2 = struct.unpack("!III", data[i:i + 12])
        communities.append(f"{ga}:{ld1}:{ld2}")
    return communities


def _parse_mp_attribute(data: bytes, atype: int) -> dict:
    afi = struct.unpack("!H", data[0:2])[0]
    safi = data[2]
    info = {"afi": afi, "safi": safi}

    if atype == ATTR_MP_REACH_NLRI:
        nh_len = data[3] if len(data) > 3 else 0
        next_hop = data[4:4 + nh_len]
        info["next_hop"] = _format_next_hop(next_hop)
        info["nlri_length"] = max(0, len(data) - (4 + nh_len + 1))
    else:
        info["nlri_length"] = max(0, len(data) - 3)
    return info


def _format_next_hop(data: bytes) -> str:
    if len(data) == 0:
        return ""
    if len(data) == 4:
        return socket.inet_ntoa(data)
    if len(data) == 16:
        return socket.inet_ntop(socket.AF_INET6, data)
    if len(data) == 32:
        global_addr = socket.inet_ntop(socket.AF_INET6, data[:16])
        link_addr = socket.inet_ntop(socket.AF_INET6, data[16:])
        return f"{global_addr},{link_addr}"
    return data.hex()
