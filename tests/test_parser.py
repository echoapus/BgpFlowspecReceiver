"""Tests for BGP UPDATE path-attribute parsing."""

import ipaddress
import struct

from bgpx.constants import (
    AFI_IPV6, SAFI_FLOWSPEC,
    ATTR_MP_REACH_NLRI, ATTR_IPV6_EXT_COMMUNITIES,
)
from bgpx.message.parser import parse_update, parse_update_details


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
