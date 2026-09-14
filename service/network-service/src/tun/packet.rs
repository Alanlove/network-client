//! Minimal IPv4/IPv6 packet inspection used by the future TUN data plane.
//! Only header fields needed for routing decisions are parsed — no
//! allocation, no protocol stack (that remains a later milestone).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpFlow {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub protocol: u8, // IANA protocol number (6 TCP, 17 UDP, 1 ICMP ...)
    pub length: usize,
}

pub fn parse_ip(buf: &[u8]) -> Option<IpFlow> {
    let byte0 = *buf.first()?;
    match byte0 >> 4 {
        4 => parse_v4(buf),
        6 => parse_v6(buf),
        _ => None,
    }
}

fn parse_v4(buf: &[u8]) -> Option<IpFlow> {
    if buf.len() < 20 {
        return None;
    }
    let ihl = (buf[0] & 0x0f) as usize * 4;
    if ihl < 20 || buf.len() < ihl {
        return None;
    }
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let protocol = buf[9];
    let src = Ipv4Addr::new(buf[12], buf[13], buf[14], buf[15]);
    let dst = Ipv4Addr::new(buf[16], buf[17], buf[18], buf[19]);
    Some(IpFlow {
        src: IpAddr::V4(src),
        dst: IpAddr::V4(dst),
        protocol,
        length,
    })
}

fn parse_v6(buf: &[u8]) -> Option<IpFlow> {
    if buf.len() < 40 {
        return None;
    }
    let length = 40 + u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let next_header = buf[6];
    let mut src_bytes = [0u8; 16];
    src_bytes.copy_from_slice(&buf[8..24]);
    let mut dst_bytes = [0u8; 16];
    dst_bytes.copy_from_slice(&buf[24..40]);
    Some(IpFlow {
        src: IpAddr::V6(Ipv6Addr::from(src_bytes)),
        dst: IpAddr::V6(Ipv6Addr::from(dst_bytes)),
        protocol: next_header,
        length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_header() {
        let mut pkt = [0u8; 20];
        pkt[0] = 0x45;
        pkt[2..4].copy_from_slice(&84u16.to_be_bytes());
        pkt[9] = 6;
        pkt[12..16].copy_from_slice(&[192, 168, 1, 2]);
        pkt[16..20].copy_from_slice(&[10, 0, 0, 1]);
        let f = parse_ip(&pkt).unwrap();
        assert_eq!(f.protocol, 6);
        assert_eq!(f.length, 84);
        assert_eq!(f.src.to_string(), "192.168.1.2");
        assert_eq!(f.dst.to_string(), "10.0.0.1");
    }

    #[test]
    fn rejects_truncated() {
        assert!(parse_ip(&[0x45]).is_none());
        assert!(parse_ip(&[0x60]).is_none());
    }
}
