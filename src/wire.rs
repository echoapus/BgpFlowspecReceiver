use serde_json::{json, Map, Value};
use std::net::{Ipv4Addr, Ipv6Addr};

const AFI_IPV4: u16 = 1;
const AFI_IPV6: u16 = 2;
type Result<T, E = String> = std::result::Result<T, E>;

fn take<'a>(data: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if data.len() < n {
        return Err("Truncated BGP data".into());
    }
    let (head, tail) = data.split_at(n);
    *data = tail;
    Ok(head)
}

fn u16_at(data: &mut &[u8]) -> Result<u16> {
    Ok(u16::from_be_bytes(take(data, 2)?.try_into().unwrap()))
}

pub fn header(data: &[u8]) -> Result<(u8, usize)> {
    if data.len() != 19 || data[..16] != [255; 16] {
        return Err("Invalid BGP header".into());
    }
    let length = u16::from_be_bytes([data[16], data[17]]) as usize;
    if !(19..=4096).contains(&length) {
        return Err("Invalid BGP message length".into());
    }
    Ok((data[18], length - 19))
}

pub fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut data = vec![255; 16];
    data.extend_from_slice(&((body.len() + 19) as u16).to_be_bytes());
    data.push(kind);
    data.extend_from_slice(body);
    data
}

pub fn build_open(asn: u32, hold: u16, router: Ipv4Addr) -> Vec<u8> {
    let mut caps = Vec::new();
    for afi in [1u16, 2] {
        for safi in [1, 133] {
            caps.extend_from_slice(&[1, 4]);
            caps.extend_from_slice(&afi.to_be_bytes());
            caps.extend_from_slice(&[0, safi]);
        }
    }
    caps.extend_from_slice(&[65, 4]);
    caps.extend_from_slice(&asn.to_be_bytes());
    let mut body = vec![4];
    body.extend_from_slice(&(if asn > 65535 { 23456 } else { asn as u16 }).to_be_bytes());
    body.extend_from_slice(&hold.to_be_bytes());
    body.extend_from_slice(&router.octets());
    body.extend_from_slice(&[(caps.len() + 2) as u8, 2, caps.len() as u8]);
    body.extend(caps);
    message(1, &body)
}

pub fn open(mut body: &[u8]) -> Result<Value> {
    let base = take(&mut body, 10)?;
    let mut asn = u16::from_be_bytes([base[1], base[2]]) as u32;
    let hold = u16::from_be_bytes([base[3], base[4]]);
    if base[0] != 4 || (hold != 0 && hold < 3) {
        return Err("Invalid BGP OPEN".into());
    }
    let mut params = take(&mut body, base[9] as usize)?;
    let mut as4 = false;
    while !params.is_empty() {
        let h = take(&mut params, 2)?;
        let mut caps = take(&mut params, h[1] as usize)?;
        if h[0] != 2 {
            continue;
        }
        while !caps.is_empty() {
            let h = take(&mut caps, 2)?;
            let val = take(&mut caps, h[1] as usize)?;
            if h[0] == 65 && val.len() == 4 {
                asn = u32::from_be_bytes(val.try_into().unwrap());
                as4 = true;
            }
        }
    }
    Ok(json!({"version":4,"peer_as":asn,"hold_time":hold,
        "router_id":Ipv4Addr::new(base[5],base[6],base[7],base[8]).to_string(),
        "supports_4byte_asn":as4}))
}

fn unicast(mut data: &[u8], afi: u16, next_hop: Option<&str>) -> Result<Vec<Value>> {
    let mut routes = Vec::new();
    while !data.is_empty() {
        let bits = take(&mut data, 1)?[0] as usize;
        if bits > if afi == AFI_IPV6 { 128 } else { 32 } {
            return Err("Invalid prefix length".into());
        }
        let raw = take(&mut data, bits.div_ceil(8))?;
        let prefix = prefix_string(raw, bits, afi, true).map_err(|_| "Invalid prefix")?;
        let mut route = json!({"prefix":prefix});
        if let Some(nh) = next_hop {
            route["next_hop"] = json!(nh);
        }
        routes.push(route);
    }
    Ok(routes)
}

fn components(mut data: &[u8], afi: u16) -> Result<Value> {
    let mut result = Map::new();
    while !data.is_empty() {
        let kind = take(&mut data, 1)?[0];
        let name = component_name(kind, afi)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("type{kind}"));
        let value = if kind == 1 || kind == 2 {
            let (prefix, used) =
                parse_prefix(data, 0, afi).map_err(|_| "Invalid FlowSpec prefix")?;
            take(&mut data, used)?;
            json!(prefix)
        } else {
            let bitmask = kind == 9 || kind == 12;
            let mut values = Vec::new();
            loop {
                let op = take(&mut data, 1)?[0];
                let (end, len, symbol) = if bitmask {
                    decode_bitmask_op(op)
                } else {
                    decode_op(op)
                };
                let value = read_be_int(take(&mut data, len)?);
                values.push(if bitmask {
                    format_bitmask_value(kind, symbol, value)
                } else {
                    format!("{}{}", symbol, format_numeric_value(kind, value))
                });
                if end {
                    break;
                }
            }
            json!(values)
        };
        result.insert(name, value);
    }
    Ok(Value::Object(result))
}

fn flowspec(mut data: &[u8], afi: u16) -> Result<Vec<Value>> {
    let mut routes = Vec::new();
    while !data.is_empty() {
        let first = take(&mut data, 1)?[0] as usize;
        let len = if first < 240 {
            first
        } else {
            ((first & 15) << 8) | take(&mut data, 1)?[0] as usize
        };
        routes.push(components(take(&mut data, len)?, afi)?);
    }
    Ok(routes)
}

fn attr_name(code: u8) -> String {
    match code {
        1 => "ORIGIN",
        2 => "AS_PATH",
        3 => "NEXT_HOP",
        4 => "MULTI_EXIT_DISC",
        5 => "LOCAL_PREF",
        6 => "ATOMIC_AGGREGATE",
        7 => "AGGREGATOR",
        8 => "COMMUNITIES",
        9 => "ORIGINATOR_ID",
        10 => "CLUSTER_LIST",
        14 => "MP_REACH_NLRI",
        15 => "MP_UNREACH_NLRI",
        16 => "EXTENDED_COMMUNITIES",
        17 => "AS4_PATH",
        18 => "AS4_AGGREGATOR",
        25 => "IPV6_ADDRESS_SPECIFIC_EXTENDED_COMMUNITIES",
        32 => "LARGE_COMMUNITIES",
        _ => return format!("ATTR_{code}"),
    }
    .into()
}

fn attribute(code: u8, mut data: &[u8], asn_len: usize) -> Result<Value> {
    Ok(match code {
        1 if data.len() == 1 => json!(match data[0] {
            0 => "igp".into(),
            1 => "egp".into(),
            2 => "incomplete".into(),
            n => format!("unknown-{n}"),
        }),
        2 | 17 => {
            let width = if code == 17 { 4 } else { asn_len };
            let mut path = Vec::new();
            while !data.is_empty() {
                let h = take(&mut data, 2)?;
                let asns: Vec<_> = take(&mut data, h[1] as usize * width)?
                    .chunks_exact(width)
                    .map(read_be_int)
                    .collect();
                let kind = match h[0] {
                    1 => "AS_SET".into(),
                    2 => "AS_SEQUENCE".into(),
                    3 => "AS_CONFED_SEQUENCE".into(),
                    4 => "AS_CONFED_SET".into(),
                    n => format!("SEGMENT_{n}"),
                };
                path.push(json!({"type":kind,"asns":asns}));
            }
            json!(path)
        }
        3 | 9 if data.len() == 4 => json!(next_hop_string(data)),
        4 | 5 if data.len() == 4 => json!(read_be_int(data)),
        6 if data.is_empty() => json!(true),
        7 | 18 if data.len() == if code == 7 { 6 } else { 8 } => {
            let width = data.len() - 4;
            json!({"asn":read_be_int(&data[..width]),"router_id":next_hop_string(&data[width..])})
        }
        8 if data.len().is_multiple_of(4) => json!(data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                match read_be_int(c) {
                    0xffffff01 => "NO_EXPORT".into(),
                    0xffffff02 => "NO_ADVERTISE".into(),
                    0xffffff03 => "NO_EXPORT_SUBCONFED".into(),
                    0xffffff04 => "NOPEER".into(),
                    _ => format!("{}:{}", read_be_int(&c[..2]), read_be_int(&c[2..])),
                }
            })
            .collect::<Vec<String>>()),
        10 if data.len().is_multiple_of(4) => json!(data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| next_hop_string(c))
            .collect::<Vec<_>>()),
        14 | 15 => {
            let afi = u16_at(&mut data)?;
            let safi = take(&mut data, 1)?[0];
            let mut info = json!({"afi":afi,"safi":safi});
            if code == 14 {
                let n = take(&mut data, 1)?[0] as usize;
                info["next_hop"] = json!(next_hop_string(take(&mut data, n)?));
                take(&mut data, 1)?;
            }
            info["nlri_length"] = json!(data.len());
            info
        }
        16 if data.len().is_multiple_of(8) => json!(parse_ext_communities(data)),
        25 if data.len().is_multiple_of(20) => json!(parse_ipv6_ext_communities(data)),
        32 if data.len().is_multiple_of(12) => json!(data
            .as_chunks::<12>()
            .0
            .iter()
            .map(|c| format!(
                "{}:{}:{}",
                read_be_int(&c[..4]),
                read_be_int(&c[4..8]),
                read_be_int(&c[8..])
            ))
            .collect::<Vec<_>>()),
        _ => return Err("Unknown or malformed attribute".into()),
    })
}

pub fn update(mut body: &[u8], asn_len: usize) -> Result<Value> {
    if asn_len != 2 && asn_len != 4 {
        return Err("Invalid ASN width".into());
    }
    let len = u16_at(&mut body)? as usize;
    let withdrawn = take(&mut body, len)?;
    let len = u16_at(&mut body)? as usize;
    let mut attrs = take(&mut body, len)?;
    let mut announce = Map::new();
    let mut withdraw = Map::new();
    let mut attributes = Vec::new();
    let mut actions = Vec::new();
    let mut next_hop = String::new();
    let mut flow_next_hop = String::new();
    while !attrs.is_empty() {
        let h = take(&mut attrs, 2)?;
        let (flags, code) = (h[0], h[1]);
        let n = if flags & 16 != 0 {
            u16_at(&mut attrs)? as usize
        } else {
            take(&mut attrs, 1)?[0] as usize
        };
        let data = take(&mut attrs, n)?;
        let decoded = attribute(code, data, asn_len);
        let mut attr = json!({"code":code,"name":attr_name(code),"length":n,
            "flags":{"optional":flags&128!=0,"transitive":flags&64!=0,"partial":flags&32!=0,"extended_length":flags&16!=0}});
        match decoded {
            Ok(v) => {
                attr["value"] = v;
            }
            Err(_) => {
                attr["raw"] = json!(hex_encode(data));
            }
        }
        if code == 3 {
            next_hop = attr["value"].as_str().unwrap_or("").into();
        }
        if code == 16 || code == 25 {
            if let Some(v) = attr["value"].as_array() {
                actions.extend(v.iter().cloned());
            }
        }
        if code == 14 || code == 15 {
            let mut nlri = data;
            let afi = u16_at(&mut nlri)?;
            let safi = take(&mut nlri, 1)?[0];
            let mut nh = String::new();
            if code == 14 {
                let n = take(&mut nlri, 1)?[0] as usize;
                nh = next_hop_string(take(&mut nlri, n)?);
                take(&mut nlri, 1)?;
            }
            if [1, 2].contains(&afi) && [1, 133].contains(&safi) {
                let family = if safi == 133 { "flowspec" } else { "unicast" };
                let label = format!("ipv{}-{family}", if afi == 2 { 6 } else { 4 });
                let routes = if safi == 133 {
                    if !nh.is_empty() {
                        flow_next_hop = nh.clone();
                    }
                    flowspec(nlri, afi)?
                } else {
                    unicast(nlri, afi, if code == 14 { Some(&nh) } else { None })?
                };
                if code == 14 {
                    announce.insert(label, json!(routes));
                } else {
                    withdraw.insert(label, json!(routes));
                }
            }
        }
        attributes.push(attr);
    }
    if !flow_next_hop.is_empty() {
        let replace = |values: &mut Vec<Value>| {
            for value in values {
                let Some(s) = value.as_str() else { continue };
                let replacement = match s {
                    "redirect-to-next-hop" => Some(format!("redirect-to-ipv4={flow_next_hop}")),
                    "copy-to-next-hop" => Some(format!("copy-to-ipv4={flow_next_hop}")),
                    _ => s
                        .strip_suffix("(juniper-redirect-to-next-hop)")
                        .map(|p| format!("{p}(juniper-redirect-to-ipv4={flow_next_hop})")),
                };
                if let Some(s) = replacement {
                    *value = json!(s);
                }
            }
        };
        replace(&mut actions);
        for attr in &mut attributes {
            if attr["code"] == 16 {
                if let Some(v) = attr["value"].as_array_mut() {
                    replace(v);
                }
            }
        }
    }
    if !withdrawn.is_empty() {
        withdraw.insert("ipv4-unicast".into(), json!(unicast(withdrawn, 1, None)?));
    }
    if !body.is_empty() {
        announce.insert(
            "ipv4-unicast".into(),
            json!(unicast(body, 1, Some(&next_hop))?),
        );
    }
    Ok(
        json!({"announce":announce,"withdraw":withdraw,"actions":actions,"path_attributes":attributes}),
    )
}

include!("wire_helpers.rs");
