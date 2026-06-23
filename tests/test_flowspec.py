"""Tests for flowspec NLRI and extended-community parsing."""

import ipaddress
import struct
import pytest
from bgpx.message.flowspec import (
    parse_nlri_components, parse_nlri_list, parse_ext_communities,
    parse_ipv6_ext_communities, normalize_nlri_components,
)
from bgpx.constants import AFI_IPV4, AFI_IPV6


# ── Prefix components ────────────────────────────────────────────────────────

def test_dst_prefix_24():
    # type=1 (dst-prefix), len=24, addr=10.0.0
    data = bytes([1, 24, 10, 0, 0])
    assert parse_nlri_components(data) == {"dst-prefix": "10.0.0.0/24"}


def test_src_prefix_32():
    data = bytes([2, 32, 1, 2, 3, 4])
    assert parse_nlri_components(data) == {"src-prefix": "1.2.3.4/32"}


def test_dst_prefix_ipv6():
    # type=1, len=48, first 6 bytes of IPv6 address 2001:db8::/48
    raw = bytes([0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00])
    data = bytes([1, 48]) + raw
    result = parse_nlri_components(data, afi=AFI_IPV6)
    assert result["dst-prefix"] == "2001:db8::/48"


def test_flow_label_ipv6():
    # type=13, op=0xA1: E=1, len=4bytes, EQ=1
    data = bytes([13, 0xA1, 0x00, 0x0A, 0xBC, 0xDE])
    result = parse_nlri_components(data, afi=AFI_IPV6)
    assert result["flow-label"] == ["=703710"]


# ── Numeric/operator components ───────────────────────────────────────────────

def test_ip_proto_tcp():
    # op=0x81: E=1, len=00→1byte, EQ=1
    data = bytes([3, 0x81, 6])
    assert parse_nlri_components(data) == {"ip-proto": ["=tcp(6)"]}


def test_dst_port_eq_80():
    # op=0x81: E=1, len=00→1byte, EQ=1
    data = bytes([5, 0x81, 80])
    assert parse_nlri_components(data) == {"dst-port": ["=80"]}


def test_port_range():
    # port >=1024 (not end-of-list) AND <=65535 (end-of-list)
    # op for >=: E=0, len=2bytes(shift=1→0x10), GT=1, EQ=1 → 0x13  value=1024
    # op for <=: E=1, len=2bytes(shift=1→0x10), LT=1, EQ=1 → 0x95  value=65535
    data = bytes([4, 0x13, 0x04, 0x00, 0x95, 0xFF, 0xFF])
    result = parse_nlri_components(data)
    assert "port" in result
    assert ">=1024" in result["port"]
    assert "<=65535" in result["port"]


def test_combined_rule():
    # dst-prefix 10.0.0.0/24 + ip-proto=TCP(6) + dst-port=80
    # op=0x81: E=1, len=00→1byte, EQ=1
    data = (
        bytes([1, 24, 10, 0, 0]) +   # dst-prefix 10.0.0.0/24
        bytes([3, 0x81, 6])       +   # ip-proto =6
        bytes([5, 0x81, 80])          # dst-port =80
    )
    result = parse_nlri_components(data)
    assert result["dst-prefix"] == "10.0.0.0/24"
    assert result["ip-proto"]   == ["=tcp(6)"]
    assert result["dst-port"]   == ["=80"]


def test_tcp_flags_all_bits():
    # type=9, bitmask_op=0x81: E=1, len=1byte, all bits must match
    data = bytes([9, 0x81, 0x3F])
    assert parse_nlri_components(data) == {
        "tcp-flags": ["all(fin,syn,rst,psh,ack,urg)"]
    }


def test_fragment_flags_any_bits():
    # type=12, bitmask_op=0x80: E=1, len=1byte, any bit can match
    data = bytes([12, 0x80, 0x05])
    assert parse_nlri_components(data) == {
        "fragment": ["any(df,first-fragment)"]
    }


def test_normalize_legacy_component_values():
    match = {
        "ip-proto": ["=6"],
        "tcp-flags": ["=63"],
        "fragment": ["?5"],
    }

    assert normalize_nlri_components(match) == {
        "ip-proto": ["=tcp(6)"],
        "tcp-flags": ["all(fin,syn,rst,psh,ack,urg)"],
        "fragment": ["any(df,first-fragment)"],
    }


# ── NLRI list ─────────────────────────────────────────────────────────────────

def test_nlri_list_two_routes():
    nlri1 = bytes([1, 24, 10, 0, 1])   # dst-prefix 10.0.1.0/24
    nlri2 = bytes([1, 24, 10, 0, 2])   # dst-prefix 10.0.2.0/24
    payload = bytes([len(nlri1)]) + nlri1 + bytes([len(nlri2)]) + nlri2
    routes = parse_nlri_list(payload)
    assert len(routes) == 2
    assert routes[0] == {"dst-prefix": "10.0.1.0/24"}
    assert routes[1] == {"dst-prefix": "10.0.2.0/24"}


def test_nlri_list_empty():
    assert parse_nlri_list(b'') == []


# ── Extended communities (actions) ────────────────────────────────────────────

def test_discard_action():
    ec = bytes([0x80, 0x06, 0x00, 0x00]) + struct.pack("!f", 0.0)
    assert parse_ext_communities(ec) == ["discard"]


def test_rate_limit_action():
    ec = bytes([0x80, 0x06, 0x00, 0x00]) + struct.pack("!f", 9600.0)
    actions = parse_ext_communities(ec)
    assert len(actions) == 1
    assert actions[0].startswith("rate-limit=")
    assert "9600" in actions[0]


def test_traffic_action():
    ec = bytes([0x80, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03])
    actions = parse_ext_communities(ec)
    assert actions == ["traffic-action(sample=True,terminal=True)"]


def test_rt_redirect_ipv4_action():
    ec = bytes([0x81, 0x08, 192, 0, 2, 1, 0x12, 0x34])
    actions = parse_ext_communities(ec)
    assert actions == ["rt-redirect=192.0.2.1:4660"]


def test_redirect_to_ipv4_action():
    ec = bytes.fromhex("010c010101010000")
    actions = parse_ext_communities(ec)
    assert actions == ["redirect-to-ipv4=1.1.1.1"]


def test_copy_to_ipv4_action():
    ec = bytes.fromhex("010c010101010001")
    actions = parse_ext_communities(ec)
    assert actions == ["copy-to-ipv4=1.1.1.1"]


def test_redirect_to_ipv6_action():
    ec = struct.pack("!H", 0x000C) + ipaddress.IPv6Address("2001:db8::1").packed + b"\x00\x00"
    actions = parse_ipv6_ext_communities(ec)
    assert actions == ["redirect-to-ipv6=2001:db8::1"]


def test_rt_redirect_ipv6_action():
    ec = struct.pack("!H", 0x000D) + ipaddress.IPv6Address("2001:db8::1").packed + b"\x12\x34"
    actions = parse_ipv6_ext_communities(ec)
    assert actions == ["rt-redirect=[2001:db8::1]:4660"]


def test_mark_dscp_action():
    ec = bytes([0x80, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2E])
    actions = parse_ext_communities(ec)
    assert actions == ["mark-dscp=46"]


def test_multiple_actions():
    ec = (
        bytes([0x80, 0x06, 0x00, 0x00]) + struct.pack("!f", 0.0) +   # discard
        bytes([0x80, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2E])       # mark-dscp=46
    )
    actions = parse_ext_communities(ec)
    assert "discard" in actions
    assert "mark-dscp=46" in actions
