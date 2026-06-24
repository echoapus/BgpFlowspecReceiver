use std::collections::HashMap;
use std::ffi::CString;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::raw::c_char;
use serde::Serialize;
use serde_json::Value;

// Constants
const BGP_HEADER_LEN: usize = 19;
const BGP_MARKER: &[u8; 16] = &[0xff; 16];

const AFI_IPV4: u16 = 1;
const AFI_IPV6: u16 = 2;
const SAFI_FLOWSPEC: u8 = 133;

const ATTR_ORIGIN: u8 = 1;
const ATTR_AS_PATH: u8 = 2;
const ATTR_NEXT_HOP: u8 = 3;
const ATTR_MED: u8 = 4;
const ATTR_LOCAL_PREF: u8 = 5;
const ATTR_ATOMIC_AGGREGATE: u8 = 6;
const ATTR_AGGREGATOR: u8 = 7;
const ATTR_COMMUNITIES: u8 = 8;
const ATTR_ORIGINATOR_ID: u8 = 9;
const ATTR_CLUSTER_LIST: u8 = 10;
const ATTR_MP_REACH_NLRI: u8 = 14;
const ATTR_MP_UNREACH_NLRI: u8 = 15;
const ATTR_EXT_COMMUNITIES: u8 = 16;
const ATTR_AS4_PATH: u8 = 17;
const ATTR_AS4_AGGREGATOR: u8 = 18;
const ATTR_IPV6_EXT_COMMUNITIES: u8 = 25;
const ATTR_LARGE_COMMUNITIES: u8 = 32;

// Structs for JSON serialization
#[derive(Serialize)]
struct OpenResult {
    version: u8,
    peer_as: u32,
    hold_time: u16,
    router_id: String,
}

#[derive(Serialize)]
struct UpdateResult {
    announce: HashMap<String, Vec<Value>>,
    withdraw: HashMap<String, Vec<Value>>,
    actions: Vec<String>,
    path_attributes: Vec<PathAttribute>,
}

#[derive(Serialize)]
struct PathAttribute {
    code: u8,
    name: String,
    flags: AttrFlags,
    length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
}

#[derive(Serialize)]
struct AttrFlags {
    optional: bool,
    transitive: bool,
    partial: bool,
    extended_length: bool,
}

// ── Exported C APIs ─────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn parse_header_rust(
    data: *const u8,
    len: usize,
    out_type: *mut u8,
    out_len: *mut u32,
) -> i32 {
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    if slice.len() < BGP_HEADER_LEN {
        return -1;
    }
    if &slice[0..16] != BGP_MARKER {
        return -2;
    }
    let length = u16::from_be_bytes([slice[16], slice[17]]) as u32;
    if length < BGP_HEADER_LEN as u32 {
        return -3;
    }
    unsafe {
        *out_type = slice[18];
        *out_len = length - BGP_HEADER_LEN as u32;
    }
    0
}

#[no_mangle]
pub extern "C" fn parse_open_rust(body: *const u8, len: usize) -> *mut c_char {
    let slice = unsafe { std::slice::from_raw_parts(body, len) };
    if slice.len() < 9 {
        return std::ptr::null_mut();
    }
    let version = slice[0];
    let mut peer_as = u16::from_be_bytes([slice[1], slice[2]]) as u32;
    let hold_time = u16::from_be_bytes([slice[3], slice[4]]);
    let router_id = format!("{}.{}.{}.{}", slice[5], slice[6], slice[7], slice[8]);

    // Optional parameters for 4-byte ASN (Capability 65)
    if slice.len() > 10 {
        let opt_len = slice[9] as usize;
        let mut offset = 10;
        let end = std::cmp::min(10 + opt_len, slice.len());
        while offset + 2 <= end {
            let param_type = slice[offset];
            let param_len = slice[offset + 1] as usize;
            offset += 2;
            if param_type == 2 { // Capabilities
                let mut cap_offset = offset;
                let cap_end = std::cmp::min(offset + param_len, end);
                while cap_offset + 2 <= cap_end {
                    let cap_code = slice[cap_offset];
                    let cap_len = slice[cap_offset + 1] as usize;
                    cap_offset += 2;
                    if cap_code == 65 && cap_len == 4 && cap_offset + 4 <= slice.len() {
                        peer_as = u32::from_be_bytes([
                            slice[cap_offset],
                            slice[cap_offset + 1],
                            slice[cap_offset + 2],
                            slice[cap_offset + 3],
                        ]);
                    }
                    cap_offset += cap_len;
                }
            }
            offset += param_len;
        }
    }

    let res = OpenResult {
        version,
        peer_as,
        hold_time,
        router_id,
    };
    serialize_to_c_char(&res)
}

#[no_mangle]
pub extern "C" fn parse_update_details_rust(body: *const u8, len: usize) -> *mut c_char {
    let slice = unsafe { std::slice::from_raw_parts(body, len) };
    let mut offset = 0;

    // Skip IPv4 unicast withdrawn routes
    if offset + 2 > slice.len() {
        return std::ptr::null_mut();
    }
    let withdrawn_len = u16::from_be_bytes([slice[offset], slice[offset + 1]]) as usize;
    offset += 2;
    if offset + withdrawn_len > slice.len() {
        return std::ptr::null_mut();
    }
    offset += withdrawn_len;

    // Path attributes length
    if offset + 2 > slice.len() {
        return std::ptr::null_mut();
    }
    let attr_len = u16::from_be_bytes([slice[offset], slice[offset + 1]]) as usize;
    offset += 2;
    let attr_end = offset + attr_len;
    if attr_end > slice.len() {
        return std::ptr::null_mut();
    }

    let mut announce = HashMap::new();
    let mut withdraw = HashMap::new();
    let mut actions = Vec::new();
    let mut path_attributes = Vec::new();

    while offset < attr_end {
        if offset + 2 > attr_end {
            break;
        }
        let flags_byte = slice[offset];
        let atype = slice[offset + 1];
        offset += 2;

        let alen: usize;
        if (flags_byte & 0x10) != 0 {
            if offset + 2 > attr_end {
                break;
            }
            alen = u16::from_be_bytes([slice[offset], slice[offset + 1]]) as usize;
            offset += 2;
        } else {
            if offset + 1 > attr_end {
                break;
            }
            alen = slice[offset] as usize;
            offset += 1;
        }

        if offset + alen > attr_end {
            break;
        }
        let abody = &slice[offset..offset + alen];
        offset += alen;

        let attr_info = decode_path_attribute(flags_byte, atype, abody, &mut announce, &mut withdraw, &mut actions);
        path_attributes.push(attr_info);
    }

    let res = UpdateResult {
        announce,
        withdraw,
        actions,
        path_attributes,
    };
    serialize_to_c_char(&res)
}

#[no_mangle]
pub extern "C" fn free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

// ── Private Helpers ─────────────────────────────────────────────────────────

fn serialize_to_c_char<T: Serialize>(val: &T) -> *mut c_char {
    if let Ok(json) = serde_json::to_string(val) {
        if let Ok(c_str) = CString::new(json) {
            return c_str.into_raw();
        }
    }
    std::ptr::null_mut()
}

fn path_attr_name(code: u8) -> String {
    let name = match code {
        ATTR_ORIGIN => "ORIGIN",
        ATTR_AS_PATH => "AS_PATH",
        ATTR_NEXT_HOP => "NEXT_HOP",
        ATTR_MED => "MULTI_EXIT_DISC",
        ATTR_LOCAL_PREF => "LOCAL_PREF",
        ATTR_ATOMIC_AGGREGATE => "ATOMIC_AGGREGATE",
        ATTR_AGGREGATOR => "AGGREGATOR",
        ATTR_COMMUNITIES => "COMMUNITIES",
        ATTR_ORIGINATOR_ID => "ORIGINATOR_ID",
        ATTR_CLUSTER_LIST => "CLUSTER_LIST",
        ATTR_MP_REACH_NLRI => "MP_REACH_NLRI",
        ATTR_MP_UNREACH_NLRI => "MP_UNREACH_NLRI",
        ATTR_EXT_COMMUNITIES => "EXTENDED_COMMUNITIES",
        ATTR_AS4_PATH => "AS4_PATH",
        ATTR_AS4_AGGREGATOR => "AS4_AGGREGATOR",
        ATTR_IPV6_EXT_COMMUNITIES => "IPV6_ADDRESS_SPECIFIC_EXTENDED_COMMUNITIES",
        ATTR_LARGE_COMMUNITIES => "LARGE_COMMUNITIES",
        _ => return format!("ATTR_{}", code),
    };
    name.to_string()
}

fn decode_path_attribute(
    flags: u8,
    code: u8,
    data: &[u8],
    announce: &mut HashMap<String, Vec<Value>>,
    withdraw: &mut HashMap<String, Vec<Value>>,
    actions: &mut Vec<String>,
) -> PathAttribute {
    let mut attr = PathAttribute {
        code,
        name: path_attr_name(code),
        flags: AttrFlags {
            optional: (flags & 0x80) != 0,
            transitive: (flags & 0x40) != 0,
            partial: (flags & 0x20) != 0,
            extended_length: (flags & 0x10) != 0,
        },
        length: data.len(),
        value: None,
        raw: None,
    };

    // Decode and extract MP_REACH / MP_UNREACH / EXT_COMMUNITIES for Flowspec
    if code == ATTR_MP_REACH_NLRI && data.len() > 3 {
        let afi = u16::from_be_bytes([data[0], data[1]]);
        let safi = data[2];
        if safi == SAFI_FLOWSPEC {
            let nh_len = data[3] as usize;
            let nlri_start = 4 + nh_len + 1;
            if nlri_start < data.len() {
                let label = if afi == AFI_IPV6 { "ipv6-flowspec" } else { "ipv4-flowspec" };
                let routes = parse_nlri_list(&data[nlri_start..], afi);
                announce.insert(label.to_string(), routes);
            }
        }
    } else if code == ATTR_MP_UNREACH_NLRI && data.len() > 2 {
        let afi = u16::from_be_bytes([data[0], data[1]]);
        let safi = data[2];
        if safi == SAFI_FLOWSPEC {
            let label = if afi == AFI_IPV6 { "ipv6-flowspec" } else { "ipv4-flowspec" };
            let routes = parse_nlri_list(&data[3..], afi);
            withdraw.insert(label.to_string(), routes);
        }
    } else if code == ATTR_EXT_COMMUNITIES {
        actions.extend(parse_ext_communities(data));
    } else if code == ATTR_IPV6_EXT_COMMUNITIES {
        actions.extend(parse_ipv6_ext_communities(data));
    }

    match decode_path_attribute_value(code, data) {
        Ok(val) => attr.value = Some(val),
        Err(_) => attr.raw = Some(hex_encode(data)),
    }

    attr
}

fn decode_path_attribute_value(code: u8, data: &[u8]) -> Result<Value, ()> {
    match code {
        ATTR_ORIGIN => {
            if data.is_empty() { return Err(()); }
            let name = match data[0] {
                0 => "igp",
                1 => "egp",
                2 => "incomplete",
                _ => "unknown",
            };
            Ok(Value::String(name.to_string()))
        }
        ATTR_AS_PATH => {
            let path = parse_as_path(data, 2)?;
            Ok(serde_json::to_value(&path).unwrap())
        }
        ATTR_NEXT_HOP => {
            if data.len() != 4 { return Err(()); }
            let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
            Ok(Value::String(ip.to_string()))
        }
        ATTR_MED | ATTR_LOCAL_PREF => {
            if data.len() != 4 { return Err(()); }
            let val = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            Ok(Value::Number(val.into()))
        }
        ATTR_ATOMIC_AGGREGATE => {
            if !data.is_empty() { return Err(()); }
            Ok(Value::Bool(true))
        }
        ATTR_AGGREGATOR => {
            if data.len() != 6 { return Err(()); }
            let asn = u16::from_be_bytes([data[0], data[1]]);
            let ip = Ipv4Addr::new(data[2], data[3], data[4], data[5]);
            let mut map = serde_json::Map::new();
            map.insert("asn".to_string(), Value::Number(asn.into()));
            map.insert("router_id".to_string(), Value::String(ip.to_string()));
            Ok(Value::Object(map))
        }
        ATTR_COMMUNITIES => {
            if data.len() % 4 != 0 { return Err(()); }
            let mut comms = Vec::new();
            for chunk in data.chunks_exact(4) {
                let val = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let s = match val {
                    0xFFFFFF01 => "NO_EXPORT".to_string(),
                    0xFFFFFF02 => "NO_ADVERTISE".to_string(),
                    0xFFFFFF03 => "NO_EXPORT_SUBCONFED".to_string(),
                    0xFFFFFF04 => "NOPEER".to_string(),
                    _ => {
                        let high = u16::from_be_bytes([chunk[0], chunk[1]]);
                        let low = u16::from_be_bytes([chunk[2], chunk[3]]);
                        format!("{}:{}", high, low)
                    }
                };
                comms.push(Value::String(s));
            }
            Ok(Value::Array(comms))
        }
        ATTR_ORIGINATOR_ID => {
            if data.len() != 4 { return Err(()); }
            let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
            Ok(Value::String(ip.to_string()))
        }
        ATTR_CLUSTER_LIST => {
            if data.len() % 4 != 0 { return Err(()); }
            let mut list = Vec::new();
            for chunk in data.chunks_exact(4) {
                let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
                list.push(Value::String(ip.to_string()));
            }
            Ok(Value::Array(list))
        }
        ATTR_MP_REACH_NLRI | ATTR_MP_UNREACH_NLRI => {
            if data.len() < 3 { return Err(()); }
            let afi = u16::from_be_bytes([data[0], data[1]]);
            let safi = data[2];
            let mut map = serde_json::Map::new();
            map.insert("afi".to_string(), Value::Number(afi.into()));
            map.insert("safi".to_string(), Value::Number(safi.into()));

            if code == ATTR_MP_REACH_NLRI {
                let nh_len = data[3] as usize;
                if 4 + nh_len <= data.len() {
                    let nh_bytes = &data[4..4 + nh_len];
                    let nh_str = format_next_hop(nh_bytes);
                    map.insert("next_hop".to_string(), Value::String(nh_str));
                    let nlri_len = data.len().saturating_sub(4 + nh_len + 1);
                    map.insert("nlri_length".to_string(), Value::Number(nlri_len.into()));
                }
            } else {
                let nlri_len = data.len().saturating_sub(3);
                map.insert("nlri_length".to_string(), Value::Number(nlri_len.into()));
            }
            Ok(Value::Object(map))
        }
        ATTR_EXT_COMMUNITIES => {
            let list = parse_ext_communities(data);
            Ok(Value::Array(list.into_iter().map(Value::String).collect()))
        }
        ATTR_AS4_PATH => {
            let path = parse_as_path(data, 4)?;
            Ok(serde_json::to_value(&path).unwrap())
        }
        ATTR_AS4_AGGREGATOR => {
            if data.len() != 8 { return Err(()); }
            let asn = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            let ip = Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            let mut map = serde_json::Map::new();
            map.insert("asn".to_string(), Value::Number(asn.into()));
            map.insert("router_id".to_string(), Value::String(ip.to_string()));
            Ok(Value::Object(map))
        }
        ATTR_IPV6_EXT_COMMUNITIES => {
            let list = parse_ipv6_ext_communities(data);
            Ok(Value::Array(list.into_iter().map(Value::String).collect()))
        }
        ATTR_LARGE_COMMUNITIES => {
            if data.len() % 12 != 0 { return Err(()); }
            let mut comms = Vec::new();
            for chunk in data.chunks_exact(12) {
                let ga = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                let ld1 = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                let ld2 = u32::from_be_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
                comms.push(Value::String(format!("{}:{}:{}", ga, ld1, ld2)));
            }
            Ok(Value::Array(comms))
        }
        _ => Err(()),
    }
}

fn parse_as_path(data: &[u8], asn_len: usize) -> Result<Vec<Value>, ()> {
    let mut path = Vec::new();
    let mut offset = 0;
    while offset + 2 <= data.len() {
        let seg_type = data[offset];
        let seg_len = data[offset + 1] as usize;
        offset += 2;
        let byte_len = seg_len * asn_len;
        if offset + byte_len > data.len() {
            return Err(());
        }
        let mut asns = Vec::new();
        for i in (offset..offset + byte_len).step_by(asn_len) {
            let asn = if asn_len == 2 {
                u16::from_be_bytes([data[i], data[i + 1]]) as u32
            } else {
                u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]])
            };
            asns.push(Value::Number(asn.into()));
        }
        offset += byte_len;

        let seg_name = match seg_type {
            1 => "AS_SET",
            2 => "AS_SEQUENCE",
            3 => "AS_CONFED_SEQUENCE",
            4 => "AS_CONFED_SET",
            _ => "SEGMENT_UNKNOWN",
        };

        let mut seg_map = serde_json::Map::new();
        seg_map.insert("type".to_string(), Value::String(seg_name.to_string()));
        seg_map.insert("asns".to_string(), Value::Array(asns));
        path.push(Value::Object(seg_map));
    }
    if offset != data.len() {
        return Err(());
    }
    Ok(path)
}

fn format_next_hop(data: &[u8]) -> String {
    if data.is_empty() {
        return "".to_string();
    }
    if data.len() == 4 {
        let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
        return ip.to_string();
    }
    if data.len() == 16 {
        let ip = Ipv6Addr::from([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15]
        ]);
        return ip.to_string();
    }
    if data.len() == 32 {
        let ip1 = Ipv6Addr::from([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15]
        ]);
        let ip2 = Ipv6Addr::from([
            data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            data[24], data[25], data[26], data[27], data[28], data[29], data[30], data[31]
        ]);
        return format!("{},{}", ip1, ip2);
    }
    hex_encode(data)
}

// ── Flowspec NLRI Parser ───────────────────────────────────────────────────

fn parse_nlri_list(payload: &[u8], afi: u16) -> Vec<Value> {
    let mut routes = Vec::new();
    let mut offset = 0;

    while offset < payload.len() {
        let first = payload[offset];
        let nlri_len: usize;
        if first < 0xF0 {
            nlri_len = first as usize;
            offset += 1;
        } else {
            if offset + 2 > payload.len() { break; }
            nlri_len = (((first & 0x0F) as usize) << 8) | payload[offset + 1] as usize;
            offset += 2;
        }

        if offset + nlri_len > payload.len() {
            break;
        }
        let raw = &payload[offset..offset + nlri_len];
        offset += nlri_len;

        let components = parse_nlri_components(raw, afi);
        routes.push(serde_json::to_value(&components).unwrap());
    }
    routes
}

fn parse_nlri_components(data: &[u8], afi: u16) -> HashMap<String, Value> {
    let mut components = HashMap::new();
    let mut offset = 0;

    while offset < data.len() {
        let ftype = data[offset];
        offset += 1;
        let name = component_name(ftype, afi);

        if ftype == 1 || ftype == 2 { // Prefix components
            if let Ok((prefix, new_offset)) = parse_prefix(data, offset, afi) {
                components.insert(name, Value::String(prefix));
                offset = new_offset;
            } else {
                break;
            }
        } else if ftype == 9 || ftype == 12 { // Bitmask components
            let mut values = Vec::new();
            loop {
                if offset >= data.len() { break; }
                let op = data[offset];
                offset += 1;
                let (end, length, opname) = decode_bitmask_op(op);
                if offset + length > data.len() { break; }
                let value = read_be_int(&data[offset..offset + length]);
                offset += length;
                values.push(Value::String(format_bitmask_value(ftype, &opname, value)));
                if end { break; }
            }
            components.insert(name, Value::Array(values));
        } else { // Numeric components
            let mut values = Vec::new();
            loop {
                if offset >= data.len() { break; }
                let op = data[offset];
                offset += 1;
                let (end, length, sym) = decode_op(op);
                if offset + length > data.len() { break; }
                let value = read_be_int(&data[offset..offset + length]);
                offset += length;
                values.push(Value::String(format!("{}{}", sym, format_numeric_value(ftype, value))));
                if end { break; }
            }
            components.insert(name, Value::Array(values));
        }
    }
    components
}

fn component_name(ftype: u8, afi: u16) -> String {
    if afi == AFI_IPV6 && ftype == 13 {
        return "flow-label".to_string();
    }
    let name = match ftype {
        1 => "dst-prefix",
        2 => "src-prefix",
        3 => "ip-proto",
        4 => "port",
        5 => "dst-port",
        6 => "src-port",
        7 => "icmp-type",
        8 => "icmp-code",
        9 => "tcp-flags",
        10 => "pkt-len",
        11 => "dscp",
        12 => "fragment",
        _ => return format!("type{}", ftype),
    };
    name.to_string()
}

fn parse_prefix(data: &[u8], mut offset: usize, afi: u16) -> Result<(String, usize), ()> {
    if offset >= data.len() { return Err(()); }
    let prefix_len = data[offset] as usize;
    offset += 1;
    let num_bytes = (prefix_len + 7) / 8;
    if offset + num_bytes > data.len() { return Err(()); }
    let raw = &data[offset..offset + num_bytes];
    offset += num_bytes;

    let addr_str = if afi == AFI_IPV6 {
        let mut buf = [0u8; 16];
        buf[..num_bytes].copy_from_slice(raw);
        let ip = Ipv6Addr::from(buf);
        ip.to_string()
    } else if afi == AFI_IPV4 {
        let mut buf = [0u8; 4];
        buf[..num_bytes].copy_from_slice(raw);
        let ip = Ipv4Addr::from(buf);
        ip.to_string()
    } else {
        return Err(());
    };

    Ok((format!("{}/{}", addr_str, prefix_len), offset))
}

fn decode_op(op: u8) -> (bool, usize, String) {
    let end_of_list = (op & 0x80) != 0;
    let length = 1 << ((op >> 4) & 0x03);
    let lt = (op & 0x04) != 0;
    let gt = (op & 0x02) != 0;
    let eq = (op & 0x01) != 0;

    let mut sym = String::new();
    if lt { sym.push('<'); }
    if gt { sym.push('>'); }
    if eq { sym.push('='); }
    if sym.is_empty() { sym.push('?'); }

    (end_of_list, length, sym)
}

fn decode_bitmask_op(op: u8) -> (bool, usize, String) {
    let end_of_list = (op & 0x80) != 0;
    let length = 1 << ((op >> 4) & 0x03);
    let negated = (op & 0x02) != 0;
    let match_all = (op & 0x01) != 0;

    let opname = if match_all {
        if negated { "not-all" } else { "all" }
    } else {
        if negated { "none" } else { "any" }
    };
    (end_of_list, length, opname.to_string())
}

fn read_be_int(data: &[u8]) -> u64 {
    let mut val = 0u64;
    for &b in data {
        val = (val << 8) | b as u64;
    }
    val
}

fn format_numeric_value(ftype: u8, value: u64) -> String {
    if ftype == 3 {
        let proto = match value {
            1 => "icmp",
            2 => "igmp",
            6 => "tcp",
            17 => "udp",
            41 => "ipv6",
            47 => "gre",
            50 => "esp",
            51 => "ah",
            58 => "icmpv6",
            89 => "ospf",
            132 => "sctp",
            _ => return value.to_string(),
        };
        return format!("{}({})", proto, value);
    }
    if ftype == 11 {
        return (value & 0x3F).to_string();
    }
    value.to_string()
}

fn format_bitmask_value(ftype: u8, opname: &str, value: u64) -> String {
    let names = if ftype == 9 {
        tcp_flag_names(value)
    } else {
        fragment_flag_names(value)
    };
    let rendered = if !names.is_empty() {
        names.join(",")
    } else {
        format!("0x{:x}", value)
    };
    format!("{}({})", opname, rendered)
}

fn tcp_flag_names(value: u64) -> Vec<String> {
    let flags = [
        (0x001, "fin"),
        (0x002, "syn"),
        (0x004, "rst"),
        (0x008, "psh"),
        (0x010, "ack"),
        (0x020, "urg"),
        (0x040, "ece"),
        (0x080, "cwr"),
        (0x100, "ns"),
    ];
    let mut names = Vec::new();
    for &(bit, name) in &flags {
        if (value & bit) != 0 {
            names.push(name.to_string());
        }
    }
    names
}

fn fragment_flag_names(value: u64) -> Vec<String> {
    let flags = [
        (0x01, "df"),
        (0x02, "is-fragment"),
        (0x04, "first-fragment"),
        (0x08, "last-fragment"),
    ];
    let mut names = Vec::new();
    for &(bit, name) in &flags {
        if (value & bit) != 0 {
            names.push(name.to_string());
        }
    }
    names
}

// ── Extended Community Parsers ──────────────────────────────────────────────

fn parse_ext_communities(data: &[u8]) -> Vec<String> {
    let mut actions = Vec::new();
    for chunk in data.chunks_exact(8) {
        let t = chunk[0];
        let s = chunk[1];

        if (t, s) == (0x80, 0x06) {
            let rate = read_f32(&chunk[4..8]);
            actions.push(if rate == 0.0 { "discard".to_string() } else { format!("rate-limit={:.0}bps", rate) });
        } else if (t, s) == (0x80, 0x0C) {
            let rate = read_f32(&chunk[4..8]);
            actions.push(if rate == 0.0 { "discard-packets".to_string() } else { format!("rate-limit={:.0}pps", rate) });
        } else if (t, s) == (0x80, 0x07) {
            let sample = (chunk[7] & 0x02) != 0;
            let terminal = (chunk[7] & 0x01) != 0;
            actions.push(format!("traffic-action(sample={},terminal={})", sample, terminal));
        } else if (t, s) == (0x80, 0x08) {
            let asn = u16::from_be_bytes([chunk[2], chunk[3]]);
            let val = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            actions.push(format!("rt-redirect={}:{}", asn, val));
        } else if (t, s) == (0x81, 0x08) {
            let ip = Ipv4Addr::new(chunk[2], chunk[3], chunk[4], chunk[5]);
            let val = u16::from_be_bytes([chunk[6], chunk[7]]);
            actions.push(format!("rt-redirect={}:{}", ip, val));
        } else if (t, s) == (0x82, 0x08) {
            let asn = u32::from_be_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);
            let val = u16::from_be_bytes([chunk[6], chunk[7]]);
            actions.push(format!("rt-redirect={}:{}", asn, val));
        } else if (t, s) == (0x80, 0x09) {
            actions.push(format!("mark-dscp={}", chunk[7] & 0x3F));
        } else if (t, s) == (0x01, 0x0C) || (t, s) == (0x80, 0x0b) || (t, s) == (0x08, 0x00) {
            let ip = Ipv4Addr::new(chunk[2], chunk[3], chunk[4], chunk[5]);
            let flags = u16::from_be_bytes([chunk[6], chunk[7]]);
            actions.push(redirect_to_ip_action("ipv4", &ip.to_string(), flags));
        } else {
            if [0x80, 0x81, 0x82].contains(&t) {
                actions.push(format!("unknown-flowspec-ec={}", hex_encode(chunk)));
            } else {
                actions.push(format!("ec={}", hex_encode(chunk)));
            }
        }
    }
    actions
}

fn parse_ipv6_ext_communities(data: &[u8]) -> Vec<String> {
    let mut actions = Vec::new();
    for chunk in data.chunks_exact(20) {
        let etype = u16::from_be_bytes([chunk[0], chunk[1]]);
        let ip = Ipv6Addr::from([
            chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7], chunk[8], chunk[9],
            chunk[10], chunk[11], chunk[12], chunk[13], chunk[14], chunk[15], chunk[16], chunk[17]
        ]);
        let val = u16::from_be_bytes([chunk[18], chunk[19]]);

        if etype == 0x000C {
            actions.push(redirect_to_ip_action("ipv6", &ip.to_string(), val));
        } else if etype == 0x000D {
            actions.push(format!("rt-redirect=[{}]:{}", ip, val));
        } else {
            if (0x000C..=0x0010).contains(&etype) {
                actions.push(format!("unknown-flowspec-ipv6-ec={}", hex_encode(chunk)));
            } else {
                actions.push(format!("ipv6-ec={}", hex_encode(chunk)));
            }
        }
    }
    actions
}

fn redirect_to_ip_action(family: &str, addr: &str, flags: u16) -> String {
    let verb = if (flags & 0x0001) != 0 { "copy-to" } else { "redirect-to" };
    let extra = if flags == 0 || flags == 1 {
        "".to_string()
    } else {
        format!("(flags=0x{:04x})", flags)
    };
    if addr == "0.0.0.0" || addr == "::" {
        return format!("{}-next-hop{}", verb, extra);
    }
    format!("{}-{}={}{}", verb, family, addr, extra)
}

fn read_f32(bytes: &[u8]) -> f32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes);
    f32::from_be_bytes(buf)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}
