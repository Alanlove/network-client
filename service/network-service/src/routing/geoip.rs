//! GeoIP lookup backed by local CIDR rule-set files (design doc §36).
//!
//! No online APIs, no embedded region hardcoding: operators drop e.g.
//! `cache/geoip/CN.txt` (one CIDR per line) into the data directory. The
//! built-in table only covers RFC private/special ranges, which always map to
//! `PRIVATE` and are routed direct in smart mode.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use crate::routing::matcher::{ipv4_bits, ipv6_bits, BitTrie};

pub const PRIVATE: &str = "PRIVATE";

const PRIVATE_V4: &[(&str, u8)] = &[
    ("0.0.0.0", 8),
    ("10.0.0.0", 8),
    ("100.64.0.0", 10),
    ("127.0.0.0", 8),
    ("169.254.0.0", 16),
    ("172.16.0.0", 12),
    ("192.168.0.0", 16),
    ("224.0.0.0", 4),
];

const PRIVATE_V6: &[(&str, u8)] = &[("::1", 128), ("fc00::", 7), ("fe80::", 10)];

pub struct GeoIp {
    v4: BitTrie<String>,
    v6: BitTrie<String>,
}

impl GeoIp {
    pub fn empty() -> Self {
        Self {
            v4: BitTrie::new(),
            v6: BitTrie::new(),
        }
    }

    pub fn with_private() -> Self {
        let mut g = Self::empty();
        for (net, prefix) in PRIVATE_V4 {
            let ip: Ipv4Addr = net.parse().unwrap();
            g.v4
                .insert(&ipv4_bits(ip), *prefix as usize, PRIVATE.to_string());
        }
        for (net, prefix) in PRIVATE_V6 {
            let ip: Ipv6Addr = net.parse().unwrap();
            g.v6
                .insert(&ipv6_bits(ip), *prefix as usize, PRIVATE.to_string());
        }
        g
    }

    /// Load every `<COUNTRY>.txt` (CIDR-per-line) from a directory.
    /// Missing directory / unreadable lines are skipped, never fatal.
    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let country = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_ascii_uppercase(),
                None => continue,
            };
            if let Ok(body) = std::fs::read_to_string(&path) {
                for line in body.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let _ = self.insert_cidr(line, country.clone());
                }
            }
        }
    }

    pub fn insert_cidr(&mut self, cidr: &str, country: String) -> Result<(), String> {
        let (ip_part, prefix) = match cidr.split_once('/') {
            Some((ip, p)) => (
                ip,
                p.parse::<usize>().map_err(|_| format!("bad prefix {cidr}"))?,
            ),
            None => (cidr, 128),
        };
        match ip_part.parse::<IpAddr>().map_err(|e: std::net::AddrParseError| e.to_string())? {
            IpAddr::V4(ip) => {
                if prefix > 32 {
                    return Err(format!("bad v4 prefix {cidr}"));
                }
                self.v4.insert(&ipv4_bits(ip), prefix, country);
            }
            IpAddr::V6(ip) => {
                if prefix > 128 {
                    return Err(format!("bad v6 prefix {cidr}"));
                }
                self.v6.insert(&ipv6_bits(ip), prefix, country);
            }
        }
        Ok(())
    }

    pub fn country(&self, ip: &IpAddr) -> Option<&str> {
        match ip {
            IpAddr::V4(ip) => self.v4.lookup(&ipv4_bits(*ip)).map(|s| s.as_str()),
            IpAddr::V6(ip) => self.v6.lookup(&ipv6_bits(*ip)).map(|s| s.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges_are_direct_class() {
        let g = GeoIp::with_private();
        assert_eq!(g.country(&"192.168.1.1".parse().unwrap()), Some(PRIVATE));
        assert_eq!(g.country(&"8.8.8.8".parse().unwrap()), None);
        assert_eq!(g.country(&"fe80::1".parse().unwrap()), Some(PRIVATE));
    }
}
