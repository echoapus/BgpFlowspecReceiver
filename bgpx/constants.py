# BGP Message Types (RFC 4271 §4)
MSG_OPEN         = 1
MSG_UPDATE       = 2
MSG_NOTIFICATION = 3
MSG_KEEPALIVE    = 4

# Capability Codes
CAP_MPBGP        = 1   # RFC 4760
CAP_ROUTE_REFRESH= 2
CAP_4BYTE_ASN    = 65  # RFC 6793

# AS_TRANS used in the 2-byte OPEN field when the real ASN is 4 bytes.
AS_TRANS         = 23456

# Address Family Identifiers
AFI_IPV4 = 1
AFI_IPV6 = 2

# Subsequent Address Family Identifiers
SAFI_UNICAST  = 1
SAFI_FLOWSPEC = 133   # RFC 8955 / RFC 8956

# BGP Path Attribute Types
ATTR_ORIGIN          = 1
ATTR_AS_PATH         = 2
ATTR_NEXT_HOP        = 3
ATTR_MED             = 4
ATTR_LOCAL_PREF      = 5
ATTR_ATOMIC_AGGREGATE = 6
ATTR_AGGREGATOR      = 7
ATTR_COMMUNITIES     = 8
ATTR_ORIGINATOR_ID   = 9
ATTR_CLUSTER_LIST    = 10
ATTR_MP_REACH_NLRI   = 14  # RFC 4760
ATTR_MP_UNREACH_NLRI = 15
ATTR_EXT_COMMUNITIES = 16  # RFC 4360
ATTR_AS4_PATH        = 17
ATTR_AS4_AGGREGATOR  = 18
ATTR_IPV6_EXT_COMMUNITIES = 25
ATTR_LARGE_COMMUNITIES = 32

# BGP Header constants
BGP_MARKER     = b'\xff' * 16
BGP_HEADER_LEN = 19   # marker(16) + length(2) + type(1)

# Flowspec NLRI component types (RFC 8955 §4.2)
FLOWSPEC_TYPE_NAMES: dict[int, str] = {
    1:  "dst-prefix",
    2:  "src-prefix",
    3:  "ip-proto",
    4:  "port",
    5:  "dst-port",
    6:  "src-port",
    7:  "icmp-type",
    8:  "icmp-code",
    9:  "tcp-flags",
    10: "pkt-len",
    11: "dscp",
    12: "fragment",
}

# Extended community (type, subtype) pairs for flowspec actions (RFC 8955 §7)
EC_TRAFFIC_RATE_BYTES   = (0x80, 0x06)
EC_TRAFFIC_ACTION       = (0x80, 0x07)
EC_RT_REDIRECT_AS2      = (0x80, 0x08)
EC_RT_REDIRECT_IPV4     = (0x81, 0x08)
EC_RT_REDIRECT_AS4      = (0x82, 0x08)
EC_TRAFFIC_MARK         = (0x80, 0x09)
EC_TRAFFIC_RATE_PACKETS = (0x80, 0x0C)

# Draft redirect-to-IP action for IPv4-address-specific extended communities.
EC_REDIRECT_TO_IPV4     = (0x01, 0x0C)

# IPv6-address-specific extended-community action types.
EC_REDIRECT_TO_IPV6     = 0x000C
EC_RT_REDIRECT_IPV6     = 0x000D
