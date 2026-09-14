//! Rule / Flow model and fixed priority ladder (design doc v1.1 §12–§16).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Action {
    Direct,
    Proxy,
    Block,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "DIRECT" => Some(Action::Direct),
            "PROXY" => Some(Action::Proxy),
            "BLOCK" => Some(Action::Block),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Domain,
    DomainSuffix,
    DomainKeyword,
    DomainRegex,
    IpCidr,
    Port,
    Process,
    Protocol,
    Geoip,
}

impl RuleKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "domain" => RuleKind::Domain,
            "domain_suffix" => RuleKind::DomainSuffix,
            "domain_keyword" => RuleKind::DomainKeyword,
            "domain_regex" => RuleKind::DomainRegex,
            "ip_cidr" => RuleKind::IpCidr,
            "port" => RuleKind::Port,
            "process" => RuleKind::Process,
            "protocol" => RuleKind::Protocol,
            "geoip" => RuleKind::Geoip,
            _ => return None,
        })
    }

    /// Fixed priority ladder from design doc §14:
    /// 1000 user / 900 process / 800 domain / 700 IP+proto / 600 geoip / 0 final.
    pub fn default_priority(self) -> i32 {
        match self {
            RuleKind::Process => 900,
            RuleKind::Domain
            | RuleKind::DomainSuffix
            | RuleKind::DomainKeyword
            | RuleKind::DomainRegex => 800,
            RuleKind::IpCidr | RuleKind::Port | RuleKind::Protocol => 700,
            RuleKind::Geoip => 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub id: String,
    pub kind: RuleKind,
    pub value: String,
    pub action: Action,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 0 means "derive from kind".
    #[serde(default)]
    pub priority: i32,
}

fn default_true() -> bool {
    true
}

impl Rule {
    pub fn effective_priority(&self) -> i32 {
        if self.priority > 0 {
            self.priority
        } else {
            self.kind.default_priority()
        }
    }

    /// User authored rules sit at the top of the ladder.
    pub fn as_user(mut self) -> Self {
        self.priority = 1000;
        self
    }
}

/// L4 flow abstraction. The packet layer never reaches the rule engine
/// directly (design doc v1.1 §11).
#[derive(Debug, Clone, Default)]
pub struct Flow {
    pub protocol: String, // "tcp" | "udp" | "icmp"
    pub src_ip: Option<std::net::IpAddr>,
    pub src_port: u16,
    pub dst_ip: Option<std::net::IpAddr>,
    pub dst_port: u16,
    pub domain: Option<String>,
    pub process_id: u32,
    pub process_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub action: Action,
    pub rule_id: Option<String>,
    pub reason: String,
}

impl Decision {
    pub fn direct(reason: impl Into<String>) -> Self {
        Self {
            action: Action::Direct,
            rule_id: None,
            reason: reason.into(),
        }
    }
    pub fn proxy(reason: impl Into<String>) -> Self {
        Self {
            action: Action::Proxy,
            rule_id: None,
            reason: reason.into(),
        }
    }
}
