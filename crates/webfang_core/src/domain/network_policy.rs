//! Per-provider network allowlist policy (B2, ADR-0004, issue #1947).
//!
//! Types and config validation only. The dial-time verdict that consumes a
//! `NetworkPolicy` lives in
//! [`crate::domain::ssrf_guard::is_forbidden_ip_with_policy`]; the client
//! threading into the embedding provider path is slice 2 of B2.
//!
//! FIN-017: the never-allowable ranges (link-local, unspecified, multicast,
//! broadcast, reserved, Teredo and the NAT64/6to4 translation prefixes) are
//! rejected at config load AND re-checked at dial time — an allowlist entry
//! can never reach them, so a naive allowlist cannot open `169.254.169.254`.
//! Loopback stays exclusively under the B1 `allow_loopback` flag (#1462).
//!
//! `allow_hosts` is parsed and validated here; its resolver-side enforcement
//! (a hostname that resolves into an allowlisted network) is slice 2, wired
//! together with the embedding-client threading.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Never-allowable prefixes (FIN-017). Parsed as const tuples, never from
/// text — the list is trusted compile-time data, so no runtime parse can
/// fail on it. The NAT64/6to4 translation prefixes are here so an allowlist
/// can never blanket-permit a whole translation range (the embedded IPv4
/// must be allowlisted individually instead).
const NEVER_ALLOWABLE: &[(IpAddr, u8, &str)] = &[
    (IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 32, "unspecified"),
    (
        IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)),
        16,
        "link-local (cloud metadata)",
    ),
    (IpAddr::V4(Ipv4Addr::new(224, 0, 0, 0)), 4, "multicast"),
    (IpAddr::V4(Ipv4Addr::new(240, 0, 0, 0)), 4, "reserved"),
    (
        IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255)),
        32,
        "broadcast",
    ),
    (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 128, "unspecified"),
    (
        IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)),
        10,
        "link-local",
    ),
    (
        IpAddr::V6(Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0)),
        8,
        "multicast",
    ),
    (
        IpAddr::V6(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0)),
        32,
        "Teredo",
    ),
    (
        IpAddr::V6(Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0, 0)),
        96,
        "NAT64 translation prefix",
    ),
    (
        IpAddr::V6(Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0)),
        16,
        "6to4 translation prefix",
    ),
];

/// Loopback is not an allowlist concern: it is governed exclusively by the
/// B1 `allow_loopback` flag (#1462), so an allowlist entry overlapping it is
/// a config error pointing at that flag.
const LOOPBACK_PREFIXES: &[(IpAddr, u8)] = &[
    (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8),
    (IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), 128),
];

/// Whether a provider's allowlist entries take effect at all (B2, ADR-0004
/// Q6 option (b)). `Restricted` is today's behavior: public endpoints only,
/// loopback via `allow_loopback`. Entries declared under `Restricted` are a
/// config error, never a silent no-op.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicyMode {
    /// Today's behavior: public endpoints only; loopback is governed
    /// exclusively by `ProviderConfig::allow_loopback` (B1, #1462).
    #[default]
    Restricted,
    /// The `allow_cidrs`/`allow_hosts` entries take effect.
    Allowlist,
}

/// A parsed CIDR block (`a.b.c.d/len` or IPv6/len), std-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrBlock {
    /// Network address of the block.
    pub addr: IpAddr,
    /// Prefix length (0..=32 for v4, 0..=128 for v6).
    pub prefix_len: u8,
}

impl CidrBlock {
    /// Parse `text` as a CIDR block. Errors are user-facing (Spanish) — this
    /// runs at config load.
    pub fn parse(text: &str) -> Result<Self, NetworkPolicyError> {
        let entry = text.trim();
        let invalid = || NetworkPolicyError::InvalidCidr {
            entry: text.to_string(),
        };
        let (addr_text, prefix_text) = entry.split_once('/').ok_or_else(invalid)?;
        let addr: IpAddr = addr_text.parse().map_err(|_| invalid())?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix_len: u8 = prefix_text.parse().map_err(|_| invalid())?;
        if prefix_len > max {
            return Err(invalid());
        }
        Ok(Self { addr, prefix_len })
    }

    /// Whether `ip` falls inside this block. IPv4-mapped IPv6 forms
    /// (`::ffff:a.b.c.d`) are normalized to IPv4 first, matching
    /// [`crate::domain::ssrf_guard::is_forbidden_ip`]; mismatched families
    /// never match.
    #[must_use]
    pub fn contains(&self, ip: &IpAddr) -> bool {
        let block_addr = self.addr;
        let ip = normalize_mapped(*ip);
        match (block_addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let bits = u32::from(self.prefix_len);
                if bits == 0 {
                    return true;
                }
                let mask = u32::MAX << (32 - bits);
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            },
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let bits = u32::from(self.prefix_len);
                if bits == 0 {
                    return true;
                }
                let shift = 128 - bits;
                (u128::from(net) >> shift) == (u128::from(ip) >> shift)
            },
            // Family mismatch (after mapped normalization) never matches.
            _ => false,
        }
    }
}

impl fmt::Display for CidrBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix_len)
    }
}

impl serde::Serialize for CidrBlock {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Normalizes IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to its IPv4 form. Only the
/// mapped form — the deprecated IPv4-compatible form is NOT normalized here,
/// it is rejected at config validation and re-validated by the guard.
fn normalize_mapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// The per-provider network policy (B2). Absent on a provider = today's
/// behavior exactly (see [`NetworkPolicyMode::Restricted`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct NetworkPolicy {
    /// Whether the entries take effect.
    pub mode: NetworkPolicyMode,
    /// Explicitly allowed networks (LAN use case: RFC1918, ULA, CGNAT —
    /// CGNAT here IS the deliberate opt-in the ADR demands).
    pub allow_cidrs: Vec<CidrBlock>,
    /// Hostnames allowed to bypass the private-resolution denial once the
    /// resolver yields an allowlisted network. Enforced in slice 2.
    pub allow_hosts: Vec<String>,
}

impl NetworkPolicy {
    /// Whether `ip` falls inside an allowlisted block. Dial-time helper used
    /// by [`crate::domain::ssrf_guard::is_forbidden_ip_with_policy`]; never
    /// a verdict on its own — the always-denied set is checked first.
    #[must_use]
    pub fn allows_ip(&self, ip: &IpAddr) -> bool {
        let ip = normalize_mapped(*ip);
        self.allow_cidrs.iter().any(|block| block.contains(&ip))
    }
}

/// Raw serde mirror: validation converts it into [`NetworkPolicy`], so every
/// rule below can fail loud instead of being a derive-time default.
#[derive(Debug, serde::Deserialize)]
struct NetworkPolicyRaw {
    #[serde(default)]
    mode: NetworkPolicyMode,
    #[serde(default)]
    allow_cidrs: Vec<String>,
    #[serde(default)]
    allow_hosts: Vec<String>,
}

impl NetworkPolicyRaw {
    /// Validate and convert. Every rule fails loud (#1462): a config typo
    /// is an error, never a silent default or a silently-ignored entry.
    fn into_policy(self) -> Result<NetworkPolicy, NetworkPolicyError> {
        let has_entries = !self.allow_cidrs.is_empty() || !self.allow_hosts.is_empty();
        if has_entries && self.mode == NetworkPolicyMode::Restricted {
            return Err(NetworkPolicyError::EntriesWithoutAllowlistMode);
        }
        let mut allow_cidrs = Vec::with_capacity(self.allow_cidrs.len());
        for entry_text in &self.allow_cidrs {
            let entry = entry_text.trim();
            let block = CidrBlock::parse(entry)?;
            if let IpAddr::V6(v6) = block.addr {
                // IPv4-mapped/compatible IPv6 entries are a config mistake:
                // the author means an IPv4 range, so they must write it as
                // one — anything else drifts from what the file says.
                if v6.to_ipv4().is_some() {
                    return Err(NetworkPolicyError::MappedV6Entry {
                        entry: entry_text.clone(),
                    });
                }
            }
            for (addr, prefix_len, class) in NEVER_ALLOWABLE {
                let deny = CidrBlock {
                    addr: *addr,
                    prefix_len: *prefix_len,
                };
                if intersects(&deny, &block) {
                    return Err(NetworkPolicyError::NeverAllowable {
                        entry: entry_text.clone(),
                        range: deny.to_string(),
                        class: (*class).to_string(),
                    });
                }
            }
            for (addr, prefix_len) in LOOPBACK_PREFIXES {
                let loopback = CidrBlock {
                    addr: *addr,
                    prefix_len: *prefix_len,
                };
                if intersects(&loopback, &block) {
                    return Err(NetworkPolicyError::LoopbackEntry {
                        entry: entry_text.clone(),
                    });
                }
            }
            allow_cidrs.push(block);
        }
        let mut allow_hosts = Vec::with_capacity(self.allow_hosts.len());
        for host_text in &self.allow_hosts {
            let host = host_text.trim();
            if host.is_empty() {
                return Err(NetworkPolicyError::HostBlank);
            }
            if host.parse::<IpAddr>().is_ok() {
                return Err(NetworkPolicyError::HostIpLiteral {
                    entry: host_text.clone(),
                });
            }
            // DNS names are case-insensitive; store normalized so the
            // resolver-side comparison (slice 2) cannot miss on case.
            allow_hosts.push(host.to_ascii_lowercase());
        }
        Ok(NetworkPolicy {
            mode: self.mode,
            allow_cidrs,
            allow_hosts,
        })
    }
}

/// Whether two blocks overlap: each contains the other's network address.
#[must_use]
fn intersects(a: &CidrBlock, b: &CidrBlock) -> bool {
    a.contains(&b.addr) || b.contains(&a.addr)
}

impl<'de> serde::Deserialize<'de> for NetworkPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = NetworkPolicyRaw::deserialize(deserializer)?;
        raw.into_policy().map_err(serde::de::Error::custom)
    }
}

/// Config-validation errors for [`NetworkPolicy`] — user-facing, Spanish,
/// fail-loud (#1462 pattern: a typo is an error, never a silent default).
#[derive(Debug, thiserror::Error)]
pub enum NetworkPolicyError {
    /// The entry is not a CIDR block.
    #[error(
        "allow_cidrs: '{entry}' no es un CIDR válido (formato esperado: a.b.c.d/len o IPv6/len)"
    )]
    InvalidCidr {
        /// The offending entry.
        entry: String,
    },
    /// The entry overlaps a range that can never be allowlisted (FIN-017).
    #[error("allow_cidrs: '{entry}' se solapa con '{range}' ({class}), un rango que nunca se puede habilitar")]
    NeverAllowable {
        /// The offending entry.
        entry: String,
        /// The never-allowable range it overlaps.
        range: String,
        /// Human name of the range class.
        class: String,
    },
    /// The entry overlaps loopback, which is `allow_loopback`'s domain.
    #[error("allow_cidrs: '{entry}' incluye loopback; para endpoints locales usá 'allow_loopback' en el provider")]
    LoopbackEntry {
        /// The offending entry.
        entry: String,
    },
    /// Entries were declared without `mode: "allowlist"`.
    #[error("network_policy: hay entradas declaradas pero mode no es 'allowlist'; las entradas sin ese mode nunca toman efecto")]
    EntriesWithoutAllowlistMode,
    /// A host entry is an IP literal — it belongs in `allow_cidrs`.
    #[error("allow_hosts: '{entry}' es una IP literal; declará su rango en allow_cidrs")]
    HostIpLiteral {
        /// The offending entry.
        entry: String,
    },
    /// A host entry is blank.
    #[error("allow_hosts: las entradas no pueden estar vacías")]
    HostBlank,
    /// A CIDR entry is an IPv4 address written in IPv6 form.
    #[error("allow_cidrs: '{entry}' es una IPv4 en forma IPv6 (mapeada/compatible); usá el CIDR IPv4 directo (ej.: 10.0.0.0/8)")]
    MappedV6Entry {
        /// The offending entry.
        entry: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_json(mode: &str, cidrs: &str, hosts: &str) -> String {
        format!("{{\"mode\": \"{mode}\", \"allow_cidrs\": {cidrs}, \"allow_hosts\": {hosts}}}")
    }

    #[test]
    fn cidr_parse_rejects_garbage() {
        let err = CidrBlock::parse("no-un-cidr").expect_err("garbage must fail");
        assert!(
            err.to_string().contains("no es un CIDR válido"),
            "got: {err}"
        );
    }

    #[test]
    fn cidr_parse_rejects_bad_prefix() {
        let err = CidrBlock::parse("10.0.0.0/33").expect_err("prefix > family max must fail");
        assert!(
            err.to_string().contains("no es un CIDR válido"),
            "got: {err}"
        );
        let err = CidrBlock::parse("fd00::/129").expect_err("v6 prefix > 128 must fail");
        assert!(
            err.to_string().contains("no es un CIDR válido"),
            "got: {err}"
        );
    }

    #[test]
    fn cidr_contains_family_mismatch() {
        let block = CidrBlock::parse("10.0.0.0/8").expect("valid v4 entry");
        let native_v6: IpAddr = "fd00::1".parse().expect("test literal");
        assert!(!block.contains(&native_v6));
    }

    #[test]
    fn cidr_contains_mapped_v4_matches_v4() {
        let block = CidrBlock::parse("10.0.0.0/8").expect("valid v4 entry");
        let mapped: IpAddr = "::ffff:10.0.0.5".parse().expect("test literal");
        assert!(block.contains(&mapped));
    }

    #[test]
    fn allowlist_169_254_is_rejected() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[\"169.254.0.0/16\"]",
            "[]",
        ))
        .expect_err("FIN-017: cloud metadata must never be allowlistable");
        assert!(
            err.to_string().contains("nunca se puede habilitar"),
            "got: {err}"
        );
    }

    #[test]
    fn allowlist_teredo_is_rejected() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[\"2001:0::/32\"]",
            "[]",
        ))
        .expect_err("Teredo is fail-closed");
        assert!(err.to_string().contains("Teredo"), "got: {err}");
    }

    #[test]
    fn allowlist_nat64_translation_prefix_is_rejected() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[\"64:ff9b::/96\"]",
            "[]",
        ))
        .expect_err("blanket translation prefix must be rejected");
        assert!(err.to_string().contains("NAT64"), "got: {err}");
    }

    #[test]
    fn allowlist_loopback_is_rejected_with_pointer_to_allow_loopback() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[\"127.0.0.0/8\"]",
            "[]",
        ))
        .expect_err("loopback belongs to allow_loopback");
        assert!(err.to_string().contains("allow_loopback"), "got: {err}");
    }

    #[test]
    fn cgnat_is_a_valid_explicit_opt_in() {
        let parsed: NetworkPolicy =
            serde_json::from_str(&policy_json("allowlist", "[\"100.64.0.0/10\"]", "[]"))
                .expect("CGNAT in the allowlist is the explicit opt-in");
        assert_eq!(parsed.mode, NetworkPolicyMode::Allowlist);
        assert_eq!(parsed.allow_cidrs.len(), 1);
        let ip: IpAddr = "100.64.0.5".parse().expect("test literal");
        assert!(parsed.allows_ip(&ip));
    }

    #[test]
    fn rfc1918_is_valid_entry() {
        let parsed: NetworkPolicy = serde_json::from_str(&policy_json(
            "allowlist",
            "[\"10.0.0.0/8\", \"172.16.0.0/12\", \"192.168.0.0/16\"]",
            "[]",
        ))
        .expect("RFC1918 is the LAN use case");
        let ip: IpAddr = "10.0.0.5".parse().expect("test literal");
        assert!(parsed.allows_ip(&ip));
    }

    #[test]
    fn ula_is_valid_entry() {
        let parsed: NetworkPolicy =
            serde_json::from_str(&policy_json("allowlist", "[\"fc00::/7\"]", "[]"))
                .expect("v6 ULA is the v6 LAN use case");
        let ip: IpAddr = "fd00::1".parse().expect("test literal");
        assert!(parsed.allows_ip(&ip));
    }

    #[test]
    fn entries_without_allowlist_mode_error() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "restricted",
            "[\"10.0.0.0/8\"]",
            "[]",
        ))
        .expect_err("entries under restricted mode never take effect");
        assert!(
            err.to_string().contains("mode no es 'allowlist'"),
            "got: {err}"
        );
    }

    #[test]
    fn host_ip_literal_is_rejected() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[]",
            "[\"10.0.0.5\"]",
        ))
        .expect_err("IP literals belong in allow_cidrs");
        assert!(err.to_string().contains("es una IP literal"), "got: {err}");
    }

    #[test]
    fn host_blank_is_rejected() {
        let err =
            serde_json::from_str::<NetworkPolicy>(&policy_json("allowlist", "[]", "[\"   \"]"))
                .expect_err("blank host entries are a typo");
        assert!(
            err.to_string().contains("no pueden estar vacías"),
            "got: {err}"
        );
    }

    #[test]
    fn mapped_v6_entry_is_rejected() {
        let err = serde_json::from_str::<NetworkPolicy>(&policy_json(
            "allowlist",
            "[\"::ffff:10.0.0.0/104\"]",
            "[]",
        ))
        .expect_err("IPv4-in-IPv6 forms must be written as plain IPv4");
        assert!(err.to_string().contains("forma IPv6"), "got: {err}");
    }

    #[test]
    fn mode_null_is_error() {
        // serde_json reports enum-from-null as "expected value" (not the
        // "invalid type" a bool field yields) — the invariant is that null
        // errors instead of silently defaulting, which expect_err proves.
        let err = serde_json::from_str::<NetworkPolicy>(
            "{\"mode\": null, \"allow_cidrs\": [], \"allow_hosts\": []}",
        )
        .expect_err("explicit null must never silently default");
        let text = err.to_string();
        assert!(
            text.contains("invalid type") || text.contains("expected value"),
            "unexpected null-mode error text: {text}"
        );
    }

    #[test]
    fn empty_policy_defaults_match_today() {
        let parsed: NetworkPolicy = serde_json::from_str(&policy_json("restricted", "[]", "[]"))
            .expect("empty restricted policy must parse");
        assert_eq!(parsed, NetworkPolicy::default());
        let ip: IpAddr = "10.0.0.5".parse().expect("test literal");
        assert!(!parsed.allows_ip(&ip));
    }
}
