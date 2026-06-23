"""Tests for Flowspec RIB route normalization."""

from bgpx.rib import FlowspecRIB


def test_rib_normalizes_legacy_match_values_on_output():
    rib = FlowspecRIB()
    route_id = rib.add(
        "ipv4-flowspec",
        {"dst-prefix": "11.1.1.2/32", "tcp-flags": ["=63"]},
        ["discard"],
        "192.0.2.1",
    )

    route = rib.get(route_id)

    assert route["match"] == {
        "dst-prefix": "11.1.1.2/32",
        "tcp-flags": ["all(fin,syn,rst,psh,ack,urg)"],
    }
    assert rib.to_dict()["routes"][0]["match"] == route["match"]


def test_rib_remove_uses_normalized_match_values():
    rib = FlowspecRIB()
    rib.add(
        "ipv4-flowspec",
        {"dst-prefix": "11.1.1.2/32", "tcp-flags": ["=63"]},
        ["discard"],
        "192.0.2.1",
    )

    removed = rib.remove({
        "dst-prefix": "11.1.1.2/32",
        "tcp-flags": ["all(fin,syn,rst,psh,ack,urg)"],
    })

    assert removed
    assert rib.all() == []
