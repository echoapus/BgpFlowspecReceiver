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
    for c in data.as_chunks::<8>().0 {
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
                if c[7] & 0x02 != 0 {"True"} else {"False"},
                if c[7] & 0x01 != 0 {"True"} else {"False"}
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
    for c in data.as_chunks::<20>().0 {
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
        } else if (0x000C..0x0010).contains(&etype) {
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
