"""Parse BGP Flowspec NLRI and extended communities (RFC 8955, RFC 8956)."""

import ipaddress
import struct

from bgpx.constants import (
    AFI_IPV4, AFI_IPV6,
    FLOWSPEC_TYPE_NAMES,
    EC_TRAFFIC_RATE_BYTES, EC_TRAFFIC_RATE_PACKETS, EC_TRAFFIC_ACTION,
    EC_RT_REDIRECT_AS2, EC_RT_REDIRECT_IPV4, EC_RT_REDIRECT_AS4,
    EC_TRAFFIC_MARK, EC_REDIRECT_TO_IPV4,
    EC_REDIRECT_TO_IPV6, EC_RT_REDIRECT_IPV6,
)


def _parse_prefix(data: bytes, offset: int, afi: int) -> tuple[str, int]:
    prefix_len = data[offset]
    offset += 1
    num_bytes = (prefix_len + 7) // 8
    raw = data[offset:offset + num_bytes]
    offset += num_bytes
    if afi == AFI_IPV6:
        addr = ipaddress.IPv6Address(raw.ljust(16, b'\x00'))
    else:
        addr = ipaddress.IPv4Address(raw.ljust(4, b'\x00'))
    return f"{addr}/{prefix_len}", offset


def _decode_op(op: int) -> tuple[bool, int, str]:
    """Decode a numeric operator byte.

    Bit layout: E|A|len1|len0|reserved|LT|GT|EQ
    Returns (end_of_list, value_length_bytes, operator_symbol).
    """
    end_of_list = bool(op & 0x80)
    length = 1 << ((op >> 4) & 0x03)   # 1, 2, 4, or 8 bytes
    lt = bool(op & 0x04)
    gt = bool(op & 0x02)
    eq = bool(op & 0x01)
    sym = ("" + ("<" if lt else "") + (">" if gt else "") + ("=" if eq else "")) or "?"
    return end_of_list, length, sym


def _decode_bitmask_op(op: int) -> tuple[bool, int, str]:
    """Decode a bitmask operator byte used by TCP flags and fragment."""
    end_of_list = bool(op & 0x80)
    length = 1 << ((op >> 4) & 0x03)
    negated = bool(op & 0x02)
    match_all = bool(op & 0x01)

    if match_all:
        opname = "not-all" if negated else "all"
    else:
        opname = "none" if negated else "any"
    return end_of_list, length, opname


def _component_name(ftype: int, afi: int) -> str:
    if afi == AFI_IPV6 and ftype == 13:
        return "flow-label"
    return FLOWSPEC_TYPE_NAMES.get(ftype, f"type{ftype}")


def parse_nlri_components(data: bytes, afi: int = AFI_IPV4) -> dict:
    """Parse the type/value pairs of one flowspec NLRI into a human-readable dict."""
    offset = 0
    components: dict = {}

    while offset < len(data):
        ftype = data[offset]
        offset += 1
        name = _component_name(ftype, afi)

        if ftype in (1, 2):                 # prefix components (dst / src)
            prefix, offset = _parse_prefix(data, offset, afi)
            components[name] = prefix

        elif ftype in (9, 12):              # bitmask operator components
            values = []
            while True:
                op = data[offset]
                offset += 1
                end, length, opname = _decode_bitmask_op(op)
                value = int.from_bytes(data[offset:offset + length], "big")
                offset += length
                values.append(_format_bitmask_value(ftype, opname, value))
                if end:
                    break
            components[name] = values

        else:                               # numeric operator components
            values: list[str] = []
            while True:
                op = data[offset]
                offset += 1
                end, length, sym = _decode_op(op)
                value = int.from_bytes(data[offset:offset + length], "big")
                offset += length
                values.append(f"{sym}{_format_numeric_value(ftype, value)}")
                if end:
                    break
            components[name] = values

    return components


def _format_numeric_value(ftype: int, value: int) -> str:
    if ftype == 3:
        protocols = {
            1: "icmp",
            2: "igmp",
            6: "tcp",
            17: "udp",
            41: "ipv6",
            47: "gre",
            50: "esp",
            51: "ah",
            58: "icmpv6",
            89: "ospf",
            132: "sctp",
        }
        if value in protocols:
            return f"{protocols[value]}({value})"
    if ftype == 11:
        return str(value & 0x3F)
    return str(value)


def normalize_nlri_components(components: dict) -> dict:
    """Normalize legacy human-readable component values from older parsers."""
    normalized = dict(components)

    if isinstance(normalized.get("ip-proto"), list):
        normalized["ip-proto"] = [
            _normalize_legacy_numeric_value(3, value)
            for value in normalized["ip-proto"]
        ]

    if isinstance(normalized.get("dscp"), list):
        normalized["dscp"] = [
            _normalize_legacy_numeric_value(11, value)
            for value in normalized["dscp"]
        ]

    if isinstance(normalized.get("tcp-flags"), list):
        normalized["tcp-flags"] = [
            _normalize_legacy_bitmask_value(9, value)
            for value in normalized["tcp-flags"]
        ]

    if isinstance(normalized.get("fragment"), list):
        normalized["fragment"] = [
            _normalize_legacy_bitmask_value(12, value)
            for value in normalized["fragment"]
        ]

    return normalized


def _normalize_legacy_numeric_value(ftype: int, value) -> str:
    if not isinstance(value, str):
        return str(value)
    parsed = _split_operator_value(value)
    if parsed is None:
        return value
    op, number = parsed
    return f"{op}{_format_numeric_value(ftype, number)}"


def _normalize_legacy_bitmask_value(ftype: int, value) -> str:
    if not isinstance(value, str):
        return str(value)
    if "(" in value:
        return value
    parsed = _split_operator_value(value, {">=", ">", "=", "?"})
    if parsed is None:
        return value

    op, number = parsed
    opname = {
        "?": "any",
        "=": "all",
        ">": "none",
        ">=": "not-all",
    }.get(op)
    if opname is None:
        return value
    return _format_bitmask_value(ftype, opname, number)


def _split_operator_value(value: str, operators=None) -> tuple[str, int] | None:
    operators = operators or {">=", "<=", ">", "<", "=", "?"}
    for op in sorted(operators, key=len, reverse=True):
        if value.startswith(op):
            raw_number = value[len(op):]
            if raw_number.isdigit():
                return op, int(raw_number)
    return None


def _format_bitmask_value(ftype: int, opname: str, value: int) -> str:
    names = _tcp_flag_names(value) if ftype == 9 else _fragment_flag_names(value)
    rendered = ",".join(names) if names else f"0x{value:x}"
    return f"{opname}({rendered})"


def _tcp_flag_names(value: int) -> list[str]:
    flags = [
        (0x001, "fin"),
        (0x002, "syn"),
        (0x004, "rst"),
        (0x008, "psh"),
        (0x010, "ack"),
        (0x020, "urg"),
        (0x040, "ece"),
        (0x080, "cwr"),
        (0x100, "ns"),
    ]
    return [name for bit, name in flags if value & bit]


def _fragment_flag_names(value: int) -> list[str]:
    flags = [
        (0x01, "df"),
        (0x02, "is-fragment"),
        (0x04, "first-fragment"),
        (0x08, "last-fragment"),
    ]
    return [name for bit, name in flags if value & bit]


def parse_nlri_list(payload: bytes, afi: int = AFI_IPV4) -> list[dict]:
    """Parse a sequence of length-prefixed flowspec NLRIs from an MP_REACH/UNREACH payload."""
    routes: list[dict] = []
    offset = 0

    while offset < len(payload):
        # RFC 8955 §4.3: length < 0xF0 → 1 byte; otherwise 2 bytes (high nibble = 0xF)
        first = payload[offset]
        if first < 0xF0:
            nlri_len = first
            offset += 1
        else:
            nlri_len = ((first & 0x0F) << 8) | payload[offset + 1]
            offset += 2

        raw = payload[offset:offset + nlri_len]
        offset += nlri_len
        routes.append(parse_nlri_components(raw, afi))

    return routes


def parse_ext_communities(data: bytes) -> list[str]:
    """Parse flowspec action extended communities into human-readable strings."""
    actions: list[str] = []

    for i in range(0, len(data), 8):
        ec = data[i:i + 8]
        if len(ec) < 8:
            break
        t, s = ec[0], ec[1]

        if (t, s) == EC_TRAFFIC_RATE_BYTES:
            rate = struct.unpack("!f", ec[4:8])[0]
            actions.append("discard" if rate == 0.0 else f"rate-limit={rate:.0f}bps")

        elif (t, s) == EC_TRAFFIC_RATE_PACKETS:
            rate = struct.unpack("!f", ec[4:8])[0]
            actions.append(
                "discard-packets" if rate == 0.0 else f"rate-limit={rate:.0f}pps"
            )

        elif (t, s) == EC_TRAFFIC_ACTION:
            sample = bool(ec[7] & 0x02)
            terminal = bool(ec[7] & 0x01)
            actions.append(f"traffic-action(sample={sample},terminal={terminal})")

        elif (t, s) == EC_RT_REDIRECT_AS2:
            asn, value = struct.unpack("!HI", ec[2:8])
            actions.append(f"rt-redirect={asn}:{value}")

        elif (t, s) == EC_RT_REDIRECT_IPV4:
            addr = ipaddress.IPv4Address(ec[2:6])
            value = struct.unpack("!H", ec[6:8])[0]
            actions.append(f"rt-redirect={addr}:{value}")

        elif (t, s) == EC_RT_REDIRECT_AS4:
            asn, value = struct.unpack("!IH", ec[2:8])
            actions.append(f"rt-redirect={asn}:{value}")

        elif (t, s) == EC_TRAFFIC_MARK:
            actions.append(f"mark-dscp={ec[7] & 0x3F}")

        elif (t, s) in (EC_REDIRECT_TO_IPV4, (0x80, 0x0b), (0x08, 0x00)):
            addr = ipaddress.IPv4Address(ec[2:6])
            flags = struct.unpack("!H", ec[6:8])[0]
            actions.append(_redirect_to_ip_action("ipv4", str(addr), flags))

        else:
            # Types 0x80/0x81/0x82 are the flowspec opaque EC range per IANA;
            # an unrecognised subtype here is likely a vendor extension or a
            # future IANA assignment rather than a generic BGP community.
            if t in (0x80, 0x81, 0x82):
                actions.append(f"unknown-flowspec-ec={ec.hex()}")
            else:
                actions.append(f"ec={ec.hex()}")

    return actions


def parse_ipv6_ext_communities(data: bytes) -> list[str]:
    """Parse IPv6-address-specific extended communities relevant to flowspec."""
    actions: list[str] = []

    for i in range(0, len(data), 20):
        ec = data[i:i + 20]
        if len(ec) < 20:
            break
        etype = struct.unpack("!H", ec[0:2])[0]
        addr = ipaddress.IPv6Address(ec[2:18])
        value = struct.unpack("!H", ec[18:20])[0]

        if etype == EC_REDIRECT_TO_IPV6:
            actions.append(_redirect_to_ip_action("ipv6", str(addr), value))

        elif etype == EC_RT_REDIRECT_IPV6:
            actions.append(f"rt-redirect=[{addr}]:{value}")

        else:
            # 0x000C/0x000D are the only IANA-registered flowspec IPv6 EC types;
            # flag anything else in the same sub-range as potentially flowspec-related.
            if etype in range(0x000C, 0x0010):
                actions.append(f"unknown-flowspec-ipv6-ec={ec.hex()}")
            else:
                actions.append(f"ipv6-ec={ec.hex()}")

    return actions


def _redirect_to_ip_action(family: str, addr: str, flags: int) -> str:
    verb = "copy-to" if flags & 0x0001 else "redirect-to"
    extra = "" if flags in (0, 1) else f"(flags=0x{flags:04x})"
    if addr in ("0.0.0.0", "::"):
        return f"{verb}-next-hop{extra}"
    return f"{verb}-{family}={addr}{extra}"
