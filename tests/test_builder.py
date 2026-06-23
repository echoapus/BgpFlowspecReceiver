"""Tests for BGP message builders."""

import struct
import pytest
from bgpx.message.builder import build_open, build_keepalive, build_notification
from bgpx.constants import (
    BGP_MARKER, BGP_HEADER_LEN,
    MSG_OPEN, MSG_KEEPALIVE, MSG_NOTIFICATION,
    CAP_MPBGP, CAP_4BYTE_ASN, AS_TRANS,
)


def _split_header(data: bytes):
    assert data[:16] == BGP_MARKER, "Marker mismatch"
    length, msg_type = struct.unpack("!HB", data[16:19])
    body = data[19:]
    assert length == len(data), f"Length field {length} != actual {len(data)}"
    return msg_type, body


# ── KEEPALIVE ─────────────────────────────────────────────────────────────────

def test_keepalive_length():
    msg = build_keepalive()
    assert len(msg) == BGP_HEADER_LEN


def test_keepalive_type():
    msg_type, body = _split_header(build_keepalive())
    assert msg_type == MSG_KEEPALIVE
    assert body == b''


# ── OPEN ──────────────────────────────────────────────────────────────────────

def test_open_type():
    msg_type, _ = _split_header(build_open(65001, 90, "1.2.3.4"))
    assert msg_type == MSG_OPEN


def test_open_version():
    _, body = _split_header(build_open(65001, 90, "1.2.3.4"))
    assert body[0] == 4   # BGP version 4


def test_open_asn():
    _, body = _split_header(build_open(65001, 90, "1.2.3.4"))
    assert struct.unpack("!H", body[1:3])[0] == 65001


def test_open_hold_time():
    _, body = _split_header(build_open(65001, 120, "1.2.3.4"))
    assert struct.unpack("!H", body[3:5])[0] == 120


def test_open_router_id():
    import socket
    _, body = _split_header(build_open(65001, 90, "10.20.30.40"))
    assert socket.inet_ntoa(body[5:9]) == "10.20.30.40"


def test_open_contains_flowspec_caps():
    msg = build_open(65001, 90, "1.2.3.4")
    # SAFI=133 (0x85) must appear in the capabilities
    assert b'\x85' in msg


def test_open_contains_4byte_asn_cap():
    msg = build_open(65001, 90, "1.2.3.4")
    # CAP_4BYTE_ASN = 65 (0x41)
    assert bytes([CAP_4BYTE_ASN]) in msg


def test_open_uses_as_trans_for_4byte_asn():
    _, body = _split_header(build_open(70000, 90, "1.2.3.4"))
    assert struct.unpack("!H", body[1:3])[0] == AS_TRANS
    assert bytes([CAP_4BYTE_ASN, 4]) + struct.pack("!I", 70000) in body


# ── NOTIFICATION ──────────────────────────────────────────────────────────────

def test_notification_type():
    msg_type, _ = _split_header(build_notification(6, 3))
    assert msg_type == MSG_NOTIFICATION


def test_notification_codes():
    _, body = _split_header(build_notification(6, 3))
    assert body[0] == 6   # Cease
    assert body[1] == 3   # Peer De-configured


def test_notification_with_data():
    _, body = _split_header(build_notification(2, 2, b'\xde\xad'))
    assert body[2:] == b'\xde\xad'
