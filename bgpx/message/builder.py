"""Build outgoing BGP messages (RFC 4271)."""

import socket
import struct

from bgpx.constants import (
    BGP_MARKER, BGP_HEADER_LEN,
    MSG_OPEN, MSG_KEEPALIVE, MSG_NOTIFICATION,
    CAP_MPBGP, CAP_4BYTE_ASN, AS_TRANS,
    AFI_IPV4, AFI_IPV6, SAFI_FLOWSPEC,
)


def _header(msg_type: int, body: bytes) -> bytes:
    length = BGP_HEADER_LEN + len(body)
    return BGP_MARKER + struct.pack("!HB", length, msg_type) + body


def _capability(code: int, data: bytes) -> bytes:
    return bytes([code, len(data)]) + data


def build_open(local_as: int, hold_time: int, router_id: str) -> bytes:
    """Build a BGP OPEN message advertising IPv4 + IPv6 flowspec capabilities."""
    my_as = local_as if local_as <= 0xFFFF else AS_TRANS
    caps = (
        _capability(CAP_MPBGP, struct.pack("!HBB", AFI_IPV4, 0, SAFI_FLOWSPEC)) +
        _capability(CAP_MPBGP, struct.pack("!HBB", AFI_IPV6, 0, SAFI_FLOWSPEC)) +
        _capability(CAP_4BYTE_ASN, struct.pack("!I", local_as))
    )
    # Wrap all capabilities in a single Optional Parameter (type=2)
    opt = b'\x02' + bytes([len(caps)]) + caps
    body = (
        struct.pack("!BHH", 4, my_as, hold_time) +
        socket.inet_aton(router_id) +
        bytes([len(opt)]) + opt
    )
    return _header(MSG_OPEN, body)


def build_keepalive() -> bytes:
    return _header(MSG_KEEPALIVE, b'')


def build_notification(error_code: int, subcode: int, data: bytes = b'') -> bytes:
    return _header(MSG_NOTIFICATION, bytes([error_code, subcode]) + data)
