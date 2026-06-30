use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::net::{Ipv4Addr, Ipv6Addr};

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

#[pyfunction]
fn parse_header(data: &[u8]) -> PyResult<(u8, u32)> {
    if data.len() < BGP_HEADER_LEN {
        return Err(pyo3::exceptions::PyValueError::new_err("Header too short"));
    }
    if &data[..16] != BGP_MARKER {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "Invalid BGP marker",
        ));
    }
    let length = u16::from_be_bytes([data[16], data[17]]) as u32;
    if length < BGP_HEADER_LEN as u32 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "BGP message length below minimum 19",
        ));
    }
    Ok((data[18], length - BGP_HEADER_LEN as u32))
}

#[pyfunction]
fn parse_open(py: Python, body: &[u8]) -> PyResult<PyObject> {
    if body.len() < 9 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "OPEN body too short",
        ));
    }

    let mut peer_as = u16::from_be_bytes([body[1], body[2]]) as u32;
    let mut supports_4byte_asn = false;

    if body.len() > 10 {
        let mut offset = 10;
        let end = (10 + body[9] as usize).min(body.len());
        while offset + 2 <= end {
            let param_type = body[offset];
            let param_len = body[offset + 1] as usize;
            offset += 2;
            if param_type == 2 {
                let mut cap_offset = offset;
                let cap_end = (offset + param_len).min(end);
                while cap_offset + 2 <= cap_end {
                    let cap_code = body[cap_offset];
                    let cap_len = body[cap_offset + 1] as usize;
                    cap_offset += 2;
                    if cap_code == 65 && cap_len == 4 && cap_offset + 4 <= body.len() {
                        supports_4byte_asn = true;
                        peer_as = u32::from_be_bytes([
                            body[cap_offset],
                            body[cap_offset + 1],
                            body[cap_offset + 2],
                            body[cap_offset + 3],
                        ]);
                    }
                    cap_offset += cap_len;
                }
            }
            offset += param_len;
        }
    }

    let d = PyDict::new_bound(py);
    d.set_item("version", body[0])?;
    d.set_item("peer_as", peer_as)?;
    d.set_item("hold_time", u16::from_be_bytes([body[3], body[4]]))?;
    d.set_item(
        "router_id",
        Ipv4Addr::new(body[5], body[6], body[7], body[8]).to_string(),
    )?;
    d.set_item("supports_4byte_asn", supports_4byte_asn)?;
    Ok(d.into_py(py))
}

#[pyfunction]
fn parse_update_details(py: Python, body: &[u8], asn_len: usize) -> PyResult<PyObject> {
    if asn_len != 2 && asn_len != 4 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "asn_len must be 2 or 4",
        ));
    }

    let mut offset = 0;
    if offset + 2 > body.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "UPDATE too short for withdrawn-routes length field",
        ));
    }
    let withdrawn_len = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
    offset += 2;
    if offset + withdrawn_len > body.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "UPDATE withdrawn_len exceeds message body",
        ));
    }
    let withdrawn_payload = &body[offset..offset + withdrawn_len];
    offset += withdrawn_len;

    if offset + 2 > body.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "UPDATE too short for path-attributes length field",
        ));
    }
    let attr_len = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
    offset += 2;
    let attr_end = offset + attr_len;
    if attr_end > body.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "UPDATE attr_len exceeds message body",
        ));
    }

    let announce = PyDict::new_bound(py);
    let withdraw = PyDict::new_bound(py);
    let actions = PyList::empty_bound(py);
    let path_attributes = PyList::empty_bound(py);
    let mut trailing_next_hop: Option<String> = None;

    while offset < attr_end {
        if offset + 2 > attr_end {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Truncated path-attribute header",
            ));
        }
        let flags = body[offset];
        let code = body[offset + 1];
        offset += 2;

        let alen = if flags & 0x10 != 0 {
            if offset + 2 > attr_end {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "Truncated extended path-attribute length",
                ));
            }
            let n = u16::from_be_bytes([body[offset], body[offset + 1]]) as usize;
            offset += 2;
            n
        } else {
            if offset >= attr_end {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "Truncated path-attribute length",
                ));
            }
            let n = body[offset] as usize;
            offset += 1;
            n
        };

        if offset + alen > attr_end {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "Path attribute exceeds declared attribute length",
            ));
        }
        let data = &body[offset..offset + alen];
        offset += alen;

        let attr = path_attr_py(
            py,
            flags,
            code,
            data,
            asn_len,
            &announce,
            &withdraw,
            &actions,
            &path_attributes,
            &mut trailing_next_hop,
        )?;
        path_attributes.append(attr)?;
    }

    if !withdrawn_payload.is_empty() {
        let routes = unicast_routes_py(py, withdrawn_payload, AFI_IPV4, None).map_err(|_| {
            pyo3::exceptions::PyValueError::new_err("Failed to parse withdrawn NLRI")
        })?;
        withdraw.set_item("ipv4-unicast", routes)?;
    }

    if attr_end < body.len() {
        let routes = unicast_routes_py(
            py,
            &body[attr_end..],
            AFI_IPV4,
            trailing_next_hop.as_deref(),
        )
        .map_err(|_| pyo3::exceptions::PyValueError::new_err("Failed to parse trailing NLRI"))?;
        announce.set_item("ipv4-unicast", routes)?;
    }

    let d = PyDict::new_bound(py);
    d.set_item("announce", announce)?;
    d.set_item("withdraw", withdraw)?;
    d.set_item("actions", actions)?;
    d.set_item("path_attributes", path_attributes)?;
    Ok(d.into_py(py))
}

#[pymodule]
fn bgpx_rust(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(parse_header, m)?)?;
    m.add_function(wrap_pyfunction!(parse_open, m)?)?;
    m.add_function(wrap_pyfunction!(parse_update_details, m)?)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn path_attr_py<'py>(
    py: Python<'py>,
    flags: u8,
    code: u8,
    data: &[u8],
    asn_len: usize,
    announce: &Bound<'py, PyDict>,
    withdraw: &Bound<'py, PyDict>,
    actions: &Bound<'py, PyList>,
    path_attributes: &Bound<'py, PyList>,
    trailing_next_hop: &mut Option<String>,
) -> PyResult<PyObject> {
    if code == ATTR_MP_REACH_NLRI && data.len() > 3 {
        let afi = u16::from_be_bytes([data[0], data[1]]);
        let safi = data[2];
        let nh_len = data[3] as usize;
        let nlri_start = 4 + nh_len + 1;
        if nlri_start > data.len() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "MP_REACH next-hop exceeds attribute length",
            ));
        }
        if afi == AFI_IPV4 || afi == AFI_IPV6 {
            if safi == SAFI_FLOWSPEC {
                let nh = next_hop_string(&data[4..4 + nh_len]);
                if !nh.is_empty() {
                    apply_flowspec_next_hop_actions(actions, path_attributes, &nh)?;
                }
                announce.set_item(
                    if afi == AFI_IPV6 {
                        "ipv6-flowspec"
                    } else {
                        "ipv4-flowspec"
                    },
                    flowspec_routes_py(py, &data[nlri_start..], afi)?,
                )?;
            } else if safi == 1 {
                let nh = next_hop_string(&data[4..4 + nh_len]);
                announce.set_item(
                    if afi == AFI_IPV6 {
                        "ipv6-unicast"
                    } else {
                        "ipv4-unicast"
                    },
                    unicast_routes_py(py, &data[nlri_start..], afi, Some(&nh)).map_err(|_| {
                        pyo3::exceptions::PyValueError::new_err("Failed to parse MP_REACH NLRI")
                    })?,
                )?;
            }
        }
    } else if code == ATTR_MP_UNREACH_NLRI && data.len() > 2 {
        let afi = u16::from_be_bytes([data[0], data[1]]);
        let safi = data[2];
        if afi == AFI_IPV4 || afi == AFI_IPV6 {
            if safi == SAFI_FLOWSPEC {
                withdraw.set_item(
                    if afi == AFI_IPV6 {
                        "ipv6-flowspec"
                    } else {
                        "ipv4-flowspec"
                    },
                    flowspec_routes_py(py, &data[3..], afi)?,
                )?;
            } else if safi == 1 {
                withdraw.set_item(
                    if afi == AFI_IPV6 {
                        "ipv6-unicast"
                    } else {
                        "ipv4-unicast"
                    },
                    unicast_routes_py(py, &data[3..], afi, None).map_err(|_| {
                        pyo3::exceptions::PyValueError::new_err("Failed to parse MP_UNREACH NLRI")
                    })?,
                )?;
            }
        }
    }

    let attr = PyDict::new_bound(py);
    attr.set_item("code", code)?;
    if let Some(name) = attr_name(code) {
        attr.set_item("name", name)?;
    } else {
        attr.set_item("name", format!("ATTR_{}", code))?;
    }
    let fd = PyDict::new_bound(py);
    fd.set_item("optional", flags & 0x80 != 0)?;
    fd.set_item("transitive", flags & 0x40 != 0)?;
    fd.set_item("partial", flags & 0x20 != 0)?;
    fd.set_item("extended_length", flags & 0x10 != 0)?;
    attr.set_item("flags", fd)?;
    attr.set_item("length", data.len())?;

    if code == ATTR_EXT_COMMUNITIES {
        let v = PyList::empty_bound(py);
        for s in parse_ext_communities(data) {
            actions.append(&s)?;
            v.append(s)?;
        }
        attr.set_item("value", v)?;
    } else if code == ATTR_IPV6_EXT_COMMUNITIES {
        let v = PyList::empty_bound(py);
        for s in parse_ipv6_ext_communities(data) {
            actions.append(&s)?;
            v.append(s)?;
        }
        attr.set_item("value", v)?;
    } else {
        match attr_value_py(py, code, data, asn_len, trailing_next_hop) {
            Ok(v) => attr.set_item("value", v)?,
            Err(_) => attr.set_item("raw", hex_encode(data))?,
        }
    }

    Ok(attr.into_py(py))
}

fn attr_name(code: u8) -> Option<&'static str> {
    Some(match code {
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
        _ => return None,
    })
}

fn attr_value_py(
    py: Python,
    code: u8,
    data: &[u8],
    asn_len: usize,
    trailing_next_hop: &mut Option<String>,
) -> Result<PyObject, ()> {
    match code {
        ATTR_ORIGIN => {
            if data.is_empty() {
                return Err(());
            }
            Ok(match data[0] {
                0 => "igp",
                1 => "egp",
                2 => "incomplete",
                _ => "unknown",
            }
            .into_py(py))
        }
        ATTR_AS_PATH => as_path_py(py, data, asn_len),
        ATTR_NEXT_HOP => {
            if data.len() != 4 {
                return Err(());
            }
            let s = Ipv4Addr::new(data[0], data[1], data[2], data[3]).to_string();
            *trailing_next_hop = Some(s.clone());
            Ok(s.into_py(py))
        }
        ATTR_MED | ATTR_LOCAL_PREF => {
            if data.len() != 4 {
                return Err(());
            }
            Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]).into_py(py))
        }
        ATTR_ATOMIC_AGGREGATE => {
            if data.is_empty() {
                Ok(true.into_py(py))
            } else {
                Err(())
            }
        }
        ATTR_AGGREGATOR => {
            if data.len() != 6 {
                return Err(());
            }
            let d = PyDict::new_bound(py);
            d.set_item("asn", u16::from_be_bytes([data[0], data[1]]))
                .map_err(|_| ())?;
            d.set_item(
                "router_id",
                Ipv4Addr::new(data[2], data[3], data[4], data[5]).to_string(),
            )
            .map_err(|_| ())?;
            Ok(d.into_py(py))
        }
        ATTR_COMMUNITIES => communities_py(py, data),
        ATTR_ORIGINATOR_ID => {
            if data.len() != 4 {
                return Err(());
            }
            Ok(Ipv4Addr::new(data[0], data[1], data[2], data[3])
                .to_string()
                .into_py(py))
        }
        ATTR_CLUSTER_LIST => cluster_list_py(py, data),
        ATTR_MP_REACH_NLRI | ATTR_MP_UNREACH_NLRI => mp_attr_py(py, code, data),
        ATTR_AS4_PATH => as_path_py(py, data, 4),
        ATTR_AS4_AGGREGATOR => {
            if data.len() != 8 {
                return Err(());
            }
            let d = PyDict::new_bound(py);
            d.set_item(
                "asn",
                u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
            )
            .map_err(|_| ())?;
            d.set_item(
                "router_id",
                Ipv4Addr::new(data[4], data[5], data[6], data[7]).to_string(),
            )
            .map_err(|_| ())?;
            Ok(d.into_py(py))
        }
        ATTR_LARGE_COMMUNITIES => large_communities_py(py, data),
        _ => Err(()),
    }
}

fn as_path_py(py: Python, data: &[u8], asn_len: usize) -> Result<PyObject, ()> {
    let path = PyList::empty_bound(py);
    let mut offset = 0;
    while offset + 2 <= data.len() {
        let seg_type = data[offset];
        let seg_len = data[offset + 1] as usize;
        offset += 2;
        let byte_len = seg_len * asn_len;
        if offset + byte_len > data.len() {
            return Err(());
        }
        let asns = PyList::empty_bound(py);
        let end = offset + byte_len;
        while offset < end {
            let asn = if asn_len == 2 {
                u16::from_be_bytes([data[offset], data[offset + 1]]) as u32
            } else {
                u32::from_be_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ])
            };
            asns.append(asn).map_err(|_| ())?;
            offset += asn_len;
        }
        let seg = PyDict::new_bound(py);
        seg.set_item(
            "type",
            match seg_type {
                1 => "AS_SET",
                2 => "AS_SEQUENCE",
                3 => "AS_CONFED_SEQUENCE",
                4 => "AS_CONFED_SET",
                _ => "SEGMENT_UNKNOWN",
            },
        )
        .map_err(|_| ())?;
        seg.set_item("asns", asns).map_err(|_| ())?;
        path.append(seg).map_err(|_| ())?;
    }
    if offset == data.len() {
        Ok(path.into_py(py))
    } else {
        Err(())
    }
}

fn communities_py(py: Python, data: &[u8]) -> Result<PyObject, ()> {
    if !data.len().is_multiple_of(4) {
        return Err(());
    }
    let out = PyList::empty_bound(py);
    for c in data.chunks_exact(4) {
        let val = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        let s = match val {
            0xFFFFFF01 => "NO_EXPORT".to_string(),
            0xFFFFFF02 => "NO_ADVERTISE".to_string(),
            0xFFFFFF03 => "NO_EXPORT_SUBCONFED".to_string(),
            0xFFFFFF04 => "NOPEER".to_string(),
            _ => format!(
                "{}:{}",
                u16::from_be_bytes([c[0], c[1]]),
                u16::from_be_bytes([c[2], c[3]])
            ),
        };
        out.append(s).map_err(|_| ())?;
    }
    Ok(out.into_py(py))
}

fn cluster_list_py(py: Python, data: &[u8]) -> Result<PyObject, ()> {
    if !data.len().is_multiple_of(4) {
        return Err(());
    }
    let out = PyList::empty_bound(py);
    for c in data.chunks_exact(4) {
        out.append(Ipv4Addr::new(c[0], c[1], c[2], c[3]).to_string())
            .map_err(|_| ())?;
    }
    Ok(out.into_py(py))
}

fn large_communities_py(py: Python, data: &[u8]) -> Result<PyObject, ()> {
    if !data.len().is_multiple_of(12) {
        return Err(());
    }
    let out = PyList::empty_bound(py);
    for c in data.chunks_exact(12) {
        out.append(format!(
            "{}:{}:{}",
            u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
            u32::from_be_bytes([c[4], c[5], c[6], c[7]]),
            u32::from_be_bytes([c[8], c[9], c[10], c[11]])
        ))
        .map_err(|_| ())?;
    }
    Ok(out.into_py(py))
}

fn mp_attr_py(py: Python, code: u8, data: &[u8]) -> Result<PyObject, ()> {
    if data.len() < 3 {
        return Err(());
    }
    let d = PyDict::new_bound(py);
    d.set_item("afi", u16::from_be_bytes([data[0], data[1]]))
        .map_err(|_| ())?;
    d.set_item("safi", data[2]).map_err(|_| ())?;
    if code == ATTR_MP_REACH_NLRI {
        if data.len() < 4 {
            return Err(());
        }
        let nh_len = data[3] as usize;
        if 4 + nh_len <= data.len() {
            d.set_item("next_hop", next_hop_string(&data[4..4 + nh_len]))
                .map_err(|_| ())?;
            d.set_item("nlri_length", data.len().saturating_sub(4 + nh_len + 1))
                .map_err(|_| ())?;
        }
    } else {
        d.set_item("nlri_length", data.len().saturating_sub(3))
            .map_err(|_| ())?;
    }
    Ok(d.into_py(py))
}

fn unicast_routes_py<'py>(
    py: Python<'py>,
    payload: &[u8],
    afi: u16,
    next_hop: Option<&str>,
) -> Result<Bound<'py, PyList>, ()> {
    let max_bits = if afi == AFI_IPV6 {
        128
    } else if afi == AFI_IPV4 {
        32
    } else {
        return Err(());
    };
    let routes = PyList::empty_bound(py);
    let mut offset = 0;
    while offset < payload.len() {
        let prefix_len = payload[offset] as usize;
        offset += 1;
        if prefix_len > max_bits {
            return Err(());
        }
        let byte_len = prefix_len.div_ceil(8);
        if offset + byte_len > payload.len() {
            return Err(());
        }
        let prefix = prefix_string(&payload[offset..offset + byte_len], prefix_len, afi, true)?;
        offset += byte_len;
        let r = PyDict::new_bound(py);
        r.set_item("prefix", prefix).map_err(|_| ())?;
        if let Some(nh) = next_hop {
            r.set_item("next_hop", nh).map_err(|_| ())?;
        }
        routes.append(r).map_err(|_| ())?;
    }
    Ok(routes)
}

fn flowspec_routes_py<'py>(
    py: Python<'py>,
    payload: &[u8],
    afi: u16,
) -> PyResult<Bound<'py, PyList>> {
    let routes = PyList::empty_bound(py);
    let mut offset = 0;
    while offset < payload.len() {
        let first = payload[offset];
        let nlri_len;
        if first < 0xF0 {
            nlri_len = first as usize;
            offset += 1;
        } else {
            if offset + 2 > payload.len() {
                break;
            }
            nlri_len = (((first & 0x0F) as usize) << 8) | payload[offset + 1] as usize;
            offset += 2;
        }
        if offset + nlri_len > payload.len() {
            break;
        }
        routes.append(flowspec_components_py(
            py,
            &payload[offset..offset + nlri_len],
            afi,
        )?)?;
        offset += nlri_len;
    }
    Ok(routes)
}

fn flowspec_components_py<'py>(
    py: Python<'py>,
    data: &[u8],
    afi: u16,
) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new_bound(py);
    let mut offset = 0;
    while offset < data.len() {
        let ftype = data[offset];
        offset += 1;
        let name = component_name(ftype, afi);
        if ftype == 1 || ftype == 2 {
            if let Ok((prefix, next)) = parse_prefix(data, offset, afi) {
                set_component(&d, name, ftype, prefix)?;
                offset = next;
            } else {
                break;
            }
        } else if ftype == 9 || ftype == 12 {
            let values = PyList::empty_bound(py);
            loop {
                if offset >= data.len() {
                    break;
                }
                let op = data[offset];
                offset += 1;
                let (end, length, opname) = decode_bitmask_op(op);
                if offset + length > data.len() {
                    break;
                }
                values.append(format_bitmask_value(
                    ftype,
                    opname,
                    read_be_int(&data[offset..offset + length]),
                ))?;
                offset += length;
                if end {
                    break;
                }
            }
            set_component(&d, name, ftype, values)?;
        } else {
            let values = PyList::empty_bound(py);
            loop {
                if offset >= data.len() {
                    break;
                }
                let op = data[offset];
                offset += 1;
                let (end, length, sym) = decode_op(op);
                if offset + length > data.len() {
                    break;
                }
                values.append(format!(
                    "{}{}",
                    sym,
                    format_numeric_value(ftype, read_be_int(&data[offset..offset + length]))
                ))?;
                offset += length;
                if end {
                    break;
                }
            }
            set_component(&d, name, ftype, values)?;
        }
    }
    Ok(d)
}

fn set_component<V: ToPyObject>(
    d: &Bound<'_, PyDict>,
    name: Option<&'static str>,
    ftype: u8,
    value: V,
) -> PyResult<()> {
    if let Some(name) = name {
        d.set_item(name, value)
    } else {
        d.set_item(format!("type{}", ftype), value)
    }
}

fn component_name(ftype: u8, afi: u16) -> Option<&'static str> {
    if afi == AFI_IPV6 && ftype == 13 {
        return Some("flow-label");
    }
    Some(match ftype {
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
        _ => return None,
    })
}

fn parse_prefix(data: &[u8], mut offset: usize, afi: u16) -> Result<(String, usize), ()> {
    if offset >= data.len() {
        return Err(());
    }
    let prefix_len = data[offset] as usize;
    offset += 1;
    let max_bits = if afi == AFI_IPV6 { 128 } else { 32 };
    if prefix_len > max_bits {
        return Err(());
    }
    let num_bytes = prefix_len.div_ceil(8);
    if offset + num_bytes > data.len() {
        return Err(());
    }
    Ok((
        prefix_string(&data[offset..offset + num_bytes], prefix_len, afi, false)?,
        offset + num_bytes,
    ))
}

fn prefix_string(raw: &[u8], prefix_len: usize, afi: u16, mask: bool) -> Result<String, ()> {
    if afi == AFI_IPV6 {
        let mut buf = [0u8; 16];
        buf[..raw.len()].copy_from_slice(raw);
        if mask {
            mask_prefix(&mut buf, prefix_len);
        }
        Ok(format!("{}/{}", Ipv6Addr::from(buf), prefix_len))
    } else if afi == AFI_IPV4 {
        let mut buf = [0u8; 4];
        buf[..raw.len()].copy_from_slice(raw);
        if mask {
            mask_prefix(&mut buf, prefix_len);
        }
        Ok(format!("{}/{}", Ipv4Addr::from(buf), prefix_len))
    } else {
        Err(())
    }
}

fn mask_prefix(address: &mut [u8], prefix_len: usize) {
    let full_bytes = prefix_len / 8;
    let remaining_bits = prefix_len % 8;
    if remaining_bits != 0 && full_bytes < address.len() {
        address[full_bytes] &= 0xff << (8 - remaining_bits);
    }
    let zero_from = full_bytes + usize::from(remaining_bits != 0);
    address[zero_from..].fill(0);
}

fn next_hop_string(data: &[u8]) -> String {
    match data.len() {
        0 => String::new(),
        4 => Ipv4Addr::new(data[0], data[1], data[2], data[3]).to_string(),
        16 => Ipv6Addr::from(<[u8; 16]>::try_from(data).unwrap()).to_string(),
        32 => format!(
            "{},{}",
            Ipv6Addr::from(<[u8; 16]>::try_from(&data[..16]).unwrap()),
            Ipv6Addr::from(<[u8; 16]>::try_from(&data[16..]).unwrap())
        ),
        _ => hex_encode(data),
    }
}

fn decode_op(op: u8) -> (bool, usize, &'static str) {
    let sym = match (op & 0x04 != 0, op & 0x02 != 0, op & 0x01 != 0) {
        (true, true, true) => "<>=",
        (true, true, false) => "<>",
        (true, false, true) => "<=",
        (true, false, false) => "<",
        (false, true, true) => ">=",
        (false, true, false) => ">",
        (false, false, true) => "=",
        _ => "?",
    };
    (op & 0x80 != 0, 1 << ((op >> 4) & 0x03), sym)
}

fn decode_bitmask_op(op: u8) -> (bool, usize, &'static str) {
    let opname = if op & 0x01 != 0 {
        if op & 0x02 != 0 {
            "not-all"
        } else {
            "all"
        }
    } else if op & 0x02 != 0 {
        "none"
    } else {
        "any"
    };
    (op & 0x80 != 0, 1 << ((op >> 4) & 0x03), opname)
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
        if let Some(name) = match value {
            1 => Some("icmp"),
            2 => Some("igmp"),
            6 => Some("tcp"),
            17 => Some("udp"),
            41 => Some("ipv6"),
            47 => Some("gre"),
            50 => Some("esp"),
            51 => Some("ah"),
            58 => Some("icmpv6"),
            89 => Some("ospf"),
            132 => Some("sctp"),
            _ => None,
        } {
            return format!("{}({})", name, value);
        }
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
    if names.is_empty() {
        format!("{}(0x{:x})", opname, value)
    } else {
        format!("{}({})", opname, names.join(","))
    }
}

fn tcp_flag_names(value: u64) -> Vec<&'static str> {
    let mut names = Vec::with_capacity(4);
    for &(bit, name) in &[
        (0x001, "fin"),
        (0x002, "syn"),
        (0x004, "rst"),
        (0x008, "psh"),
        (0x010, "ack"),
        (0x020, "urg"),
        (0x040, "ece"),
        (0x080, "cwr"),
        (0x100, "ns"),
    ] {
        if value & bit != 0 {
            names.push(name);
        }
    }
    names
}

fn fragment_flag_names(value: u64) -> Vec<&'static str> {
    let mut names = Vec::with_capacity(2);
    for &(bit, name) in &[
        (0x01, "df"),
        (0x02, "is-fragment"),
        (0x04, "first-fragment"),
        (0x08, "last-fragment"),
    ] {
        if value & bit != 0 {
            names.push(name);
        }
    }
    names
}

fn parse_ext_communities(data: &[u8]) -> Vec<String> {
    let mut actions = Vec::with_capacity(data.len() / 8);
    for c in data.chunks_exact(8) {
        let t = c[0];
        let s = c[1];
        if (t, s) == (0x80, 0x06) {
            let rate = read_f32(&c[4..8]);
            actions.push(if rate <= 0.0 {
                "discard".to_string()
            } else {
                format!("rate-limit={:.0}bps", rate * 8.0)
            });
        } else if (t, s) == (0x80, 0x0C) {
            let rate = read_f32(&c[4..8]);
            actions.push(if rate <= 0.0 {
                "discard-packets".to_string()
            } else {
                format!("rate-limit={:.0}pps", rate)
            });
        } else if (t, s) == (0x80, 0x07) {
            actions.push(format!(
                "traffic-action(sample={},terminal={})",
                c[7] & 0x02 != 0,
                c[7] & 0x01 != 0
            ));
        } else if (t, s) == (0x80, 0x08) {
            actions.push(format!(
                "rt-redirect={}:{}",
                u16::from_be_bytes([c[2], c[3]]),
                u32::from_be_bytes([c[4], c[5], c[6], c[7]])
            ));
        } else if (t, s) == (0x81, 0x08) {
            actions.push(format!(
                "rt-redirect={}:{}",
                Ipv4Addr::new(c[2], c[3], c[4], c[5]),
                u16::from_be_bytes([c[6], c[7]])
            ));
        } else if (t, s) == (0x82, 0x08) {
            actions.push(format!(
                "rt-redirect={}:{}",
                u32::from_be_bytes([c[2], c[3], c[4], c[5]]),
                u16::from_be_bytes([c[6], c[7]])
            ));
        } else if (t, s) == (0x80, 0x09) {
            actions.push(format!("mark-dscp={}", c[7] & 0x3F));
        } else if (t, s) == (0x01, 0x0C) {
            actions.push(redirect_to_ip_action(
                "ipv4",
                &Ipv4Addr::new(c[2], c[3], c[4], c[5]).to_string(),
                u16::from_be_bytes([c[6], c[7]]),
            ));
        } else if t == 0x08 {
            actions.push("redirect-to-next-hop".to_string());
        } else if (t, s) == (0x80, 0x0b) {
            actions.push(format!(
                "ec={}(juniper-redirect-to-next-hop)",
                hex_encode(c)
            ));
        } else if [0x80, 0x81, 0x82].contains(&t) {
            actions.push(format!("unknown-flowspec-ec={}", hex_encode(c)));
        } else {
            actions.push(format!("ec={}", hex_encode(c)));
        }
    }
    actions
}

fn parse_ipv6_ext_communities(data: &[u8]) -> Vec<String> {
    let mut actions = Vec::with_capacity(data.len() / 20);
    for c in data.chunks_exact(20) {
        let etype = u16::from_be_bytes([c[0], c[1]]);
        let ip = Ipv6Addr::from([
            c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9], c[10], c[11], c[12], c[13], c[14],
            c[15], c[16], c[17],
        ]);
        let val = u16::from_be_bytes([c[18], c[19]]);
        if etype == 0x000C {
            actions.push(redirect_to_ip_action("ipv6", &ip.to_string(), val));
        } else if etype == 0x000D {
            actions.push(format!("rt-redirect=[{}]:{}", ip, val));
        } else if (0x000C..=0x0010).contains(&etype) {
            actions.push(format!("unknown-flowspec-ipv6-ec={}", hex_encode(c)));
        } else {
            actions.push(format!("ipv6-ec={}", hex_encode(c)));
        }
    }
    actions
}

fn redirect_to_ip_action(family: &str, addr: &str, flags: u16) -> String {
    let verb = if flags & 0x0001 != 0 {
        "copy-to"
    } else {
        "redirect-to"
    };
    let extra = if flags == 0 || flags == 1 {
        String::new()
    } else {
        format!("(flags=0x{:04x})", flags)
    };
    if addr == "0.0.0.0" || addr == "::" {
        return format!("{}-next-hop{}", verb, extra);
    }
    format!("{}-{}={}{}", verb, family, addr, extra)
}

fn replace_next_hop_markers(actions: &Bound<'_, PyList>, next_hop: &str) -> PyResult<()> {
    for i in 0..actions.len() {
        let action: String = actions.get_item(i)?.extract()?;
        if action == "redirect-to-next-hop" {
            actions.set_item(i, format!("redirect-to-ipv4={}", next_hop))?;
        } else if action == "copy-to-next-hop" {
            actions.set_item(i, format!("copy-to-ipv4={}", next_hop))?;
        } else if let Some(prefix) = action.strip_suffix("(juniper-redirect-to-next-hop)") {
            actions.set_item(
                i,
                format!("{}(juniper-redirect-to-ipv4={})", prefix, next_hop),
            )?;
        }
    }
    Ok(())
}

fn apply_flowspec_next_hop_actions(
    actions: &Bound<'_, PyList>,
    path_attributes: &Bound<'_, PyList>,
    next_hop: &str,
) -> PyResult<()> {
    replace_next_hop_markers(actions, next_hop)?;
    for i in 0..path_attributes.len() {
        let attr = path_attributes.get_item(i)?;
        let attr = attr.downcast::<PyDict>()?;
        let code: u8 = attr.get_item("code")?.unwrap().extract()?;
        if code == ATTR_EXT_COMMUNITIES {
            if let Some(value) = attr.get_item("value")? {
                replace_next_hop_markers(value.downcast::<PyList>()?, next_hop)?;
            }
        }
    }
    Ok(())
}

fn read_f32(bytes: &[u8]) -> f32 {
    f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn hex_encode(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_flowspec_prefix() {
        assert!(parse_prefix(&[40, 203, 0, 113, 1, 2], 0, AFI_IPV4).is_err());
    }

    #[test]
    fn traffic_rate_bytes_is_rendered_as_bits_per_second() {
        let mut ec = vec![0x80, 0x06, 0x00, 0x00];
        ec.extend_from_slice(&9600.0f32.to_be_bytes());
        assert_eq!(parse_ext_communities(&ec), vec!["rate-limit=76800bps"]);
    }

    #[test]
    fn traffic_rate_packets_stays_packets_per_second() {
        let mut ec = vec![0x80, 0x0c, 0x00, 0x00];
        ec.extend_from_slice(&9600.0f32.to_be_bytes());
        assert_eq!(parse_ext_communities(&ec), vec!["rate-limit=9600pps"]);
    }

    #[test]
    fn non_positive_traffic_rates_are_discard() {
        let mut bytes_ec = vec![0x80, 0x06, 0x00, 0x00];
        bytes_ec.extend_from_slice(&(-1.0f32).to_be_bytes());
        let mut packets_ec = vec![0x80, 0x0c, 0x00, 0x00];
        packets_ec.extend_from_slice(&(-1.0f32).to_be_bytes());
        assert_eq!(parse_ext_communities(&bytes_ec), vec!["discard"]);
        assert_eq!(parse_ext_communities(&packets_ec), vec!["discard-packets"]);
    }
}
