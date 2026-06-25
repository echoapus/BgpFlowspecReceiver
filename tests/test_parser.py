"""Tests for BGP UPDATE path-attribute parsing."""

import ipaddress
import socket
import struct
import pytest

from bgpx.constants import (
    AFI_IPV6, SAFI_FLOWSPEC,
    ATTR_MP_REACH_NLRI, ATTR_MP_UNREACH_NLRI,
    ATTR_IPV6_EXT_COMMUNITIES, ATTR_NEXT_HOP,
)
from bgpx.message.parser import parse_open, parse_update, parse_update_details


def _attr(flags: int, code: int, value: bytes) -> bytes:
    if len(value) > 255:
        return bytes([flags | 0x10, code]) + struct.pack("!H", len(value)) + value
    return bytes([flags, code, len(value)]) + value


def _update(*attrs: bytes) -> bytes:
    path_attrs = b"".join(attrs)
    return b"\x00\x00" + struct.pack("!H", len(path_attrs)) + path_attrs


def test_parse_update_details_decodes_ipv6_flowspec_and_ipv6_redirect_action():
    # flow-label = 0xABCDE
    nlri_value = bytes([13, 0xA1, 0x00, 0x0A, 0xBC, 0xDE])
    nlri = bytes([len(nlri_value)]) + nlri_value
    mp_reach = struct.pack("!HBB", AFI_IPV6, SAFI_FLOWSPEC, 0) + b"\x00" + nlri
    redirect_to_ipv6 = (
        struct.pack("!H", 0x000C) +
        ipaddress.IPv6Address("2001:db8::1").packed +
        b"\x00\x00"
    )
    body = _update(
        _attr(0x80, ATTR_MP_REACH_NLRI, mp_reach),
        _attr(0xC0, ATTR_IPV6_EXT_COMMUNITIES, redirect_to_ipv6),
    )

    details = parse_update_details(body)

    assert details["announce"] == {
        "ipv6-flowspec": [{"flow-label": ["=703710"]}]
    }
    assert details["actions"] == ["redirect-to-ipv6=2001:db8::1"]
    assert details["path_attributes"][1]["value"] == ["redirect-to-ipv6=2001:db8::1"]


def test_parse_update_preserves_original_tuple_api():
    body = _update(_attr(0xC0, ATTR_IPV6_EXT_COMMUNITIES, b""))
    assert parse_update(body) == ({}, {}, [])


def test_parse_ipv4_unicast_announce_and_withdraw():
    withdrawn = bytes([24, 198, 51, 100])
    announced = bytes([25, 203, 0, 113, 128])
    next_hop = _attr(0x40, ATTR_NEXT_HOP, socket.inet_aton("192.0.2.1"))
    body = (
        struct.pack("!H", len(withdrawn)) + withdrawn +
        struct.pack("!H", len(next_hop)) + next_hop +
        announced
    )

    details = parse_update_details(body)

    assert details["withdraw"]["ipv4-unicast"] == [
        {"prefix": "198.51.100.0/24"}
    ]
    assert details["announce"]["ipv4-unicast"] == [{
        "prefix": "203.0.113.128/25",
        "next_hop": "192.0.2.1",
    }]


def test_parse_ipv6_unicast_mp_reach_and_unreach():
    announced = bytes([32]) + ipaddress.IPv6Address("2001:db8::").packed[:4]
    withdrawn = bytes([48]) + ipaddress.IPv6Address("2001:db8:1::").packed[:6]
    next_hop = ipaddress.IPv6Address("2001:db8::1").packed
    mp_reach = (
        struct.pack("!HBB", AFI_IPV6, 1, len(next_hop)) +
        next_hop + b"\x00" + announced
    )
    mp_unreach = struct.pack("!HB", AFI_IPV6, 1) + withdrawn
    body = _update(
        _attr(0x80, ATTR_MP_REACH_NLRI, mp_reach),
        _attr(0x80, ATTR_MP_UNREACH_NLRI, mp_unreach),
    )

    details = parse_update_details(body)

    assert details["announce"]["ipv6-unicast"] == [{
        "prefix": "2001:db8::/32",
        "next_hop": "2001:db8::1",
    }]
    assert details["withdraw"]["ipv6-unicast"] == [{
        "prefix": "2001:db8:1::/48",
    }]


def test_parse_unicast_rejects_truncated_prefix():
    body = b"\x00\x00\x00\x00" + bytes([24, 203, 0])

    with pytest.raises(ValueError, match="Truncated unicast NLRI"):
        parse_update_details(body)


def test_parse_update_details_retains_unknown_path_attribute_raw():
    body = _update(_attr(0x80, 42, b"abc"))

    details = parse_update_details(body)

    assert details["path_attributes"] == [{
        "code": 42,
        "name": "ATTR_42",
        "flags": {
            "optional": True,
            "transitive": False,
            "partial": False,
            "extended_length": False,
        },
        "length": 3,
        "raw": "616263",
    }]


def test_parse_open_decodes_4byte_asn_capability():
    version = 4
    my_as = 23456  # AS_TRANS
    hold_time = 90
    bgp_id = socket.inet_aton("192.0.2.1")

    # Capability 65 (4-byte ASN) with value 123456
    cap_code = 65
    cap_len = 4
    cap_value = struct.pack("!I", 123456)
    cap = bytes([cap_code, cap_len]) + cap_value

    # Parameter 2 (Capabilities) containing the capability
    param_type = 2
    param_len = len(cap)
    param = bytes([param_type, param_len]) + cap

    body = (
        bytes([version]) +
        struct.pack("!H", my_as) +
        struct.pack("!H", hold_time) +
        bgp_id +
        bytes([len(param)]) +
        param
    )

    res = parse_open(body)
    assert res["version"] == 4
    assert res["peer_as"] == 123456
    assert res["hold_time"] == 90
    assert res["router_id"] == "192.0.2.1"
    assert res["supports_4byte_asn"] is True


def test_parse_open_no_capabilities():
    body = (
        bytes([4]) +
        struct.pack("!H", 65000) +
        struct.pack("!H", 90) +
        socket.inet_aton("192.0.2.1") +
        bytes([0])  # opt_len = 0
    )
    res = parse_open(body)
    assert res["peer_as"] == 65000
    assert res["supports_4byte_asn"] is False


def test_parse_update_decodes_negotiated_4byte_as_path():
    as_path = bytes([2, 2]) + struct.pack("!II", 65000, 123456)
    body = _update(_attr(0x40, 2, as_path))

    details = parse_update_details(body, asn_len=4)

    assert details["path_attributes"][0]["value"] == [{
        "type": "AS_SEQUENCE",
        "asns": [65000, 123456],
    }]


def test_parse_open_other_capabilities_only():
    # Capability 1 (Multiprotocol BGP) with some data
    cap = bytes([1, 4, 0, 1, 0, 1])
    param = bytes([2, len(cap)]) + cap
    body = (
        bytes([4]) +
        struct.pack("!H", 65000) +
        struct.pack("!H", 90) +
        socket.inet_aton("192.0.2.1") +
        bytes([len(param)]) +
        param
    )
    res = parse_open(body)
    assert res["peer_as"] == 65000


def test_parse_open_truncated_4byte_asn_capability():
    # Capability 65 (4-byte ASN) but length is 2 instead of 4 (malformed)
    cap = bytes([65, 2, 0, 1])
    param = bytes([2, len(cap)]) + cap
    body = (
        bytes([4]) +
        struct.pack("!H", 23456) +
        struct.pack("!H", 90) +
        socket.inet_aton("192.0.2.1") +
        bytes([len(param)]) +
        param
    )
    res = parse_open(body)
    assert res["peer_as"] == 23456  # Fallback to 2-byte AS
