//! TTL DNS cache with bounded size and lazy expiry (design doc §17).

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Entry {
    ips: Vec<IpAddr>,
    expire_at: Instant,
}

pub struct DnsCache {
    inner: HashMap<String, Entry>,
    max_size: usize,
    enabled: bool,
}

impl DnsCache {
    pub fn new(enabled: bool, max_size: usize) -> Self {
        Self {
            inner: HashMap::new(),
            max_size: max_size.max(16),
            enabled,
        }
    }

    pub fn get(&mut self, name: &str) -> Option<Vec<IpAddr>> {
        if !self.enabled {
            return None;
        }
        let key = name.to_ascii_lowercase();
        let hit = self.inner.get(&key)?;
        if Instant::now() >= hit.expire_at {
            self.inner.remove(&key);
            return None;
        }
        Some(hit.ips.clone())
    }

    pub fn put(&mut self, name: &str, ips: Vec<IpAddr>, min_ttl: u32) {
        if !self.enabled || ips.is_empty() {
            return;
        }
        if self.inner.len() >= self.max_size {
            // Cheap eviction: drop expired entries first.
            let now = Instant::now();
            self.inner.retain(|_, e| e.expire_at > now);
            if self.inner.len() >= self.max_size {
                if let Some(oldest) = self
                    .inner
                    .iter()
                    .min_by_key(|(_, e)| e.expire_at)
                    .map(|(k, _)| k.clone())
                {
                    self.inner.remove(&oldest);
                }
            }
        }
        let ttl = min_ttl.clamp(10, 600) as u64;
        self.inner.insert(
            name.to_ascii_lowercase(),
            Entry {
                ips,
                expire_at: Instant::now() + Duration::from_secs(ttl),
            },
        );
    }

    pub fn flush(&mut self) {
        self.inner.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_flush() {
        let mut c = DnsCache::new(true, 64);
        let ips = vec!["1.1.1.1".parse().unwrap()];
        c.put("a.com", ips.clone(), 60);
        assert_eq!(c.get("A.COM").unwrap(), ips);
        c.flush();
        assert!(c.get("a.com").is_none());
    }
}
