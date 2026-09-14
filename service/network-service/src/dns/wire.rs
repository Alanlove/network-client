//! Minimal DNS wire-format builder/parser (RFC 1035). Only what the service
//! needs: A / AAAA questions/answers with compression-aware name parsing.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const TYPE_A: u16 = 1;
pub const TYPE_CNAME: u16 = 5;
pub const TYPE_AAAA: u16 = 28;

#[derive(Debug, Clone)]
pub struct DnsRecord {
    pub name: String,
    pub qtype: u16,
    pub ttl: u32,
    pub ip: Option<IpAddr>,
}

pub fn build_query(name: &str, qtype: u16, id: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());

    for label in name.split('.').filter(|l| !l.is_empty()) {
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // IN
    buf
}

fn read_name(buf: &[u8], mut offset: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut jumped = false;
    let mut next_after = offset;
    let mut hops = 0u32;
    loop {
        if offset >= buf.len() {
            return None;
        }
        let len = buf[offset];
        if len == 0 {
            offset += 1;
            if !jumped {
                next_after = offset;
            }
            break;
        }
        if len & 0xC0 == 0xC0 {
            // Compression pointer.
            if offset + 1 >= buf.len() {
                return None;
            }
            let ptr = u16::from_be_bytes([len & 0x3F, buf[offset + 1]]) as usize;
            if !jumped {
                next_after = offset + 2;
            }
            offset = ptr;
            jumped = true;
            hops += 1;
            if hops > 16 {
                return None;
            }
            continue;
        }
        offset += 1;
        let end = offset + len as usize;
        if end > buf.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&buf[offset..end]).to_string());
        offset = end;
        if !jumped {
            next_after = offset;
        }
    }
    Some((labels.join("."), next_after))
}

pub fn parse_response(buf: &[u8]) -> Option<Vec<DnsRecord>> {
    if buf.len() < 12 {
        return None;
    }
    // QR must be 1.
    if buf[2] & 0x80 == 0 {
        return None;
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let an = u16::from_be_bytes([buf[6], buf[7]]) as usize;

    let mut offset = 12;
    for _ in 0..qd {
        let (_, next) = read_name(buf, offset)?;
        offset = next + 4;
    }

    let mut records = Vec::new();
    for _ in 0..an {
        let (name, next) = read_name(buf, offset)?;
        offset = next;
        if offset + 10 > buf.len() {
            break;
        }
        let qtype = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let _class = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);
        let ttl = u32::from_be_bytes([
            buf[offset + 4],
            buf[offset + 5],
            buf[offset + 6],
            buf[offset + 7],
        ]);
        let rdlen = u16::from_be_bytes([buf[offset + 8], buf[offset + 9]]) as usize;
        offset += 10;
        if offset + rdlen > buf.len() {
            break;
        }
        let rdata = &buf[offset..offset + rdlen];
        let ip = match qtype {
            TYPE_A if rdlen == 4 => Some(IpAddr::V4(Ipv4Addr::new(
                rdata[0], rdata[1], rdata[2], rdata[3],
            ))),
            TYPE_AAAA if rdlen == 16 => {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(rdata);
                Some(IpAddr::V6(Ipv6Addr::from(arr)))
            }
            _ => None,
        };
        records.push(DnsRecord {
            name,
            qtype,
            ttl,
            ip,
        });
        offset += rdlen;
    }
    Some(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_parse_roundtrip_shape() {
        let q = build_query("example.com", TYPE_A, 0x1234);
        assert_eq!(&q[..2], &[0x12, 0x34]);
        assert!(q.ends_with(&[0x00, 0x01, 0x00, 0x01]));
    }

    #[test]
    fn parses_a_answer_with_pointer() {
        // Hand-crafted: id=1, answer name pointer to question, 1.2.3.4 ttl 60.
        let mut msg = vec![];
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0x8180u32.to_be_bytes()[2..]); // flags
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes()); // 1 answer
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(b"\x07example\x03com\x00");
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        // answer: compression pointer -> 12
        msg.extend_from_slice(&0xC00Cu16.to_be_bytes());
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&60u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[1, 2, 3, 4]);
        let records = parse_response(&msg).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].ip.unwrap().to_string(), "1.2.3.4");
        assert_eq!(records[0].ttl, 60);
        assert_eq!(records[0].name, "example.com");
    }
}
