//! Indexed matchers (design doc v1.1 §17): avoid per-flow linear scans.
//!
//! * domain suffix/exact -> reversed-label trie + hashmap
//! * keyword/regex       -> bounded ordered lists
//! * process             -> O(1) hashmap
//! * IP CIDR             -> binary longest-prefix-match trie (v4 + v6)

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use regex::Regex;

use crate::common::{SvcError, SvcResult};
use crate::routing::rule::{Action, Rule, RuleKind};

#[derive(Clone, Debug)]
pub(crate) struct M {
    pub(crate) rule_id: String,
    pub(crate) action: Action,
    pub(crate) priority: i32,
}

impl M {
    fn from_rule(rule: &Rule) -> Self {
        M {
            rule_id: rule.id.clone(),
            action: rule.action,
            priority: rule.effective_priority(),
        }
    }
}

// ---------- Domain trie -----------------------------------------------------

struct LabelNode {
    children: HashMap<String, usize>,
    leaf: Option<M>,
}

struct DomainTrie {
    nodes: Vec<LabelNode>,
}

impl DomainTrie {
    fn new() -> Self {
        Self {
            nodes: vec![LabelNode {
                children: HashMap::new(),
                leaf: None,
            }],
        }
    }

    /// Insert a suffix (e.g. "example.com").
    fn insert(&mut self, suffix: &str, m: M) {
        let lower = suffix.to_ascii_lowercase();
        let labels: Vec<&str> = lower
            .split('.')
            .filter(|l| !l.is_empty())
            .rev()
            .collect();
        let mut cur = 0usize;
        for label in labels {
            let next = match self.nodes[cur].children.get(label) {
                Some(id) => *id,
                None => {
                    let id = self.nodes.len();
                    self.nodes.push(LabelNode {
                        children: HashMap::new(),
                        leaf: None,
                    });
                    self.nodes[cur].children.insert(label.to_string(), id);
                    id
                }
            };
            cur = next;
        }
        self.nodes[cur].leaf = Some(m);
    }

    /// Longest-suffix match.
    fn lookup(&self, name: &str) -> Option<&M> {
        let lower = name.to_ascii_lowercase();
        let labels: Vec<&str> = lower
            .split('.')
            .filter(|l| !l.is_empty())
            .rev()
            .collect();
        let mut cur = 0usize;
        let mut best: Option<&M> = None;
        if let Some(leaf) = &self.nodes[cur].leaf {
            best = Some(leaf);
        }
        for label in labels {
            match self.nodes[cur].children.get(label) {
                Some(next) => {
                    cur = *next;
                    if let Some(leaf) = &self.nodes[cur].leaf {
                        best = Some(leaf);
                    }
                }
                None => break,
            }
        }
        best
    }
}

// ---------- CIDR bit trie ---------------------------------------------------

struct BitNode<T> {
    child: [Option<Box<BitNode<T>>>; 2],
    leaf: Option<T>,
}

impl<T> BitNode<T> {
    fn new() -> Self {
        Self {
            child: [None, None],
            leaf: None,
        }
    }
}

/// Generic binary longest-prefix-match trie. Used for rule CIDRs (leaf `M`)
/// and GeoIP country sets (leaf `String`).
pub(crate) struct BitTrie<T> {
    root: Box<BitNode<T>>,
}

impl<T: Clone> BitTrie<T> {
    pub(crate) fn new() -> Self {
        Self {
            root: Box::new(BitNode::new()),
        }
    }

    pub(crate) fn insert(&mut self, bits: &[u8], prefix: usize, leaf: T) {
        let mut node = &mut self.root;
        for &b in bits.iter().take(prefix) {
            let idx = b as usize;
            node = node.child[idx].get_or_insert_with(|| Box::new(BitNode::new()));
        }
        node.leaf = Some(leaf);
    }

    pub(crate) fn lookup(&self, bits: &[u8]) -> Option<&T> {
        let mut node = &self.root;
        let mut best = node.leaf.as_ref();
        for &b in bits {
            match &node.child[b as usize] {
                Some(next) => {
                    node = next;
                    if node.leaf.is_some() {
                        best = node.leaf.as_ref();
                    }
                }
                None => break,
            }
        }
        best
    }
}

pub(crate) fn ipv4_bits(addr: Ipv4Addr) -> Vec<u8> {
    addr.octets()
        .iter()
        .flat_map(|o| (0..8).rev().map(move |i| (o >> i) & 1))
        .collect()
}

pub(crate) fn ipv6_bits(addr: Ipv6Addr) -> Vec<u8> {
    addr.octets()
        .iter()
        .flat_map(|o| (0..8).rev().map(move |i| (o >> i) & 1))
        .collect()
}

pub struct IpIndex {
    v4: BitTrie<M>,
    v6: BitTrie<M>,
}

impl Default for IpIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl IpIndex {
    pub fn new() -> Self {
        Self {
            v4: BitTrie::new(),
            v6: BitTrie::new(),
        }
    }

    pub fn insert_cidr(&mut self, cidr: &str, m: M) -> SvcResult<()> {
        let (ip_part, prefix) = match cidr.split_once('/') {
            Some((ip, p)) => (
                ip,
                p.parse::<usize>()
                    .map_err(|_| SvcError::ConfigInvalid(format!("bad prefix: {cidr}")))?,
            ),
            None => (cidr, u32::MAX as usize),
        };
        match IpAddr::from_str(ip_part.trim()) {
            Ok(IpAddr::V4(ip)) => {
                if prefix > 32 {
                    return Err(SvcError::ConfigInvalid(format!("bad v4 prefix: {cidr}")));
                }
                self.v4.insert(&ipv4_bits(ip), prefix, m);
            }
            Ok(IpAddr::V6(ip)) => {
                if prefix > 128 {
                    return Err(SvcError::ConfigInvalid(format!("bad v6 prefix: {cidr}")));
                }
                self.v6.insert(&ipv6_bits(ip), prefix, m);
            }
            Err(_) => {
                return Err(SvcError::ConfigInvalid(format!("bad cidr: {cidr}")));
            }
        }
        Ok(())
    }

    pub fn lookup(&self, ip: &IpAddr) -> Option<&M> {
        match ip {
            IpAddr::V4(ip) => self.v4.lookup(&ipv4_bits(*ip)),
            IpAddr::V6(ip) => self.v6.lookup(&ipv6_bits(*ip)),
        }
    }
}

// ---------- Composite rule index -------------------------------------------

pub struct RuleIndex {
    suffix: DomainTrie,
    exact: HashMap<String, M>,
    keyword: Vec<(String, M)>,
    regex: Vec<(Regex, M)>,
    cidr: IpIndex,
    process: HashMap<String, M>,
    port_exact: HashMap<u16, M>,
    port_range: Vec<(u16, u16, M)>,
    protocol: HashMap<String, M>,
    geoip: HashMap<String, M>,
}

impl RuleIndex {
    pub fn build(rules: &[Rule]) -> SvcResult<Self> {
        let mut idx = Self {
            suffix: DomainTrie::new(),
            exact: HashMap::new(),
            keyword: Vec::new(),
            regex: Vec::new(),
            cidr: IpIndex::new(),
            process: HashMap::new(),
            port_exact: HashMap::new(),
            port_range: Vec::new(),
            protocol: HashMap::new(),
            geoip: HashMap::new(),
        };
        for rule in rules.iter().filter(|r| r.enabled) {
            let m = M::from_rule(rule);
            let value = rule.value.trim();
            match rule.kind {
                RuleKind::Domain => {
                    idx.exact.insert(value.to_ascii_lowercase(), m);
                }
                RuleKind::DomainSuffix => idx.suffix.insert(value, m),
                RuleKind::DomainKeyword => idx.keyword.push((value.to_ascii_lowercase(), m)),
                RuleKind::DomainRegex => {
                    let re = Regex::new(value)
                        .map_err(|e| SvcError::ConfigInvalid(format!("rule {}: {e}", rule.id)))?;
                    idx.regex.push((re, m));
                }
                RuleKind::IpCidr => idx.cidr.insert_cidr(value, m)?,
                RuleKind::Port => {
                    if let Some((a, b)) = value.split_once('-') {
                        idx.port_range.push((
                            a.trim().parse().map_err(|_| {
                                SvcError::ConfigInvalid(format!("bad port rule {}", rule.id))
                            })?,
                            b.trim().parse().map_err(|_| {
                                SvcError::ConfigInvalid(format!("bad port rule {}", rule.id))
                            })?,
                            m,
                        ));
                    } else {
                        idx.port_exact.insert(value.parse().map_err(|_| {
                            SvcError::ConfigInvalid(format!("bad port rule {}", rule.id))
                        })?, m);
                    }
                }
                RuleKind::Process => {
                    idx.process.insert(value.to_ascii_lowercase(), m);
                }
                RuleKind::Protocol => {
                    idx.protocol.insert(value.to_ascii_lowercase(), m);
                }
                RuleKind::Geoip => {
                    idx.geoip.insert(value.to_ascii_uppercase(), m);
                }
            }
        }
        Ok(idx)
    }

    fn better(current: &Option<M>, candidate: M) -> Option<M> {
        match current {
            Some(cur) if cur.priority >= candidate.priority => Some(cur.clone()),
            _ => Some(candidate),
        }
    }

    pub fn match_domain(&self, name: &str) -> Option<M> {
        let lower = name.to_ascii_lowercase();
        let mut best: Option<M> = None;
        if let Some(m) = self.exact.get(&lower) {
            best = Self::better(&best, m.clone());
        }
        if let Some(m) = self.suffix.lookup(name) {
            best = Self::better(&best, m.clone());
        }
        for (kw, m) in &self.keyword {
            if lower.contains(kw) {
                best = Self::better(&best, m.clone());
            }
        }
        for (re, m) in &self.regex {
            if re.is_match(name) {
                best = Self::better(&best, m.clone());
            }
        }
        best
    }

    pub fn match_ip(&self, ip: &IpAddr) -> Option<M> {
        self.cidr.lookup(ip).cloned()
    }

    pub fn match_process(&self, exe: &str) -> Option<M> {
        let lower = exe.to_ascii_lowercase();
        let key = lower.rsplit('\\').next().unwrap_or(&lower);
        let key = key.rsplit('/').next().unwrap_or(key);
        self.process.get(key).cloned()
    }

    pub fn match_protocol(&self, proto: &str) -> Option<M> {
        self.protocol.get(&proto.to_ascii_lowercase()).cloned()
    }

    pub fn match_port(&self, port: u16) -> Option<M> {
        if let Some(m) = self.port_exact.get(&port) {
            return Some(m.clone());
        }
        self.port_range
            .iter()
            .find(|(a, b, _)| port >= *a && port <= *b)
            .map(|(_, _, m)| m.clone())
    }

    pub fn match_geoip(&self, country: &str) -> Option<M> {
        self.geoip.get(&country.to_ascii_uppercase()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_trie_longest_match() {
        let mut t = DomainTrie::new();
        t.insert("example.com", M {
            rule_id: "a".into(),
            action: Action::Proxy,
            priority: 800,
        });
        t.insert("deep.example.com", M {
            rule_id: "b".into(),
            action: Action::Direct,
            priority: 800,
        });
        assert_eq!(t.lookup("x.deep.example.com").unwrap().rule_id, "b");
        assert_eq!(t.lookup("x.example.com").unwrap().rule_id, "a");
        assert!(t.lookup("evil-example.com").is_none());
    }

    #[test]
    fn cidr_lpm_v4_and_v6() {
        let mut ip = IpIndex::new();
        ip.insert_cidr("10.0.0.0/8", M {
            rule_id: "priv".into(),
            action: Action::Direct,
            priority: 700,
        })
        .unwrap();
        ip.insert_cidr("10.1.0.0/16", M {
            rule_id: "inner".into(),
            action: Action::Block,
            priority: 700,
        })
        .unwrap();
        assert_eq!(ip.lookup(&"10.1.2.3".parse().unwrap()).unwrap().rule_id, "inner");
        assert_eq!(ip.lookup(&"10.2.3.4".parse().unwrap()).unwrap().rule_id, "priv");
        assert!(ip.lookup(&"8.8.8.8".parse().unwrap()).is_none());
    }
}
