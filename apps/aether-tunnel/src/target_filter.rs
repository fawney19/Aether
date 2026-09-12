use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Check if an IP address belongs to a private/reserved network.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    // Keep every egress path on the same conservative classification as the
    // gateway. This includes documentation, benchmarking, transition, and
    // other reserved ranges that the standard `IpAddr::is_private` helpers do
    // not cover.
    aether_http::is_private_or_reserved_ip(*ip)
}

#[derive(Debug)]
pub enum FilterError {
    PrivateIp(IpAddr),
    PortNotAllowed(u16),
    DnsResolutionFailed(String),
    NoPublicAddrs(String),
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrivateIp(ip) => write!(f, "target IP {} is in private/reserved range", ip),
            Self::PortNotAllowed(port) => write!(f, "port {} not in allowed list", port),
            Self::DnsResolutionFailed(host) => write!(f, "DNS resolution failed for {}", host),
            Self::NoPublicAddrs(host) => {
                write!(
                    f,
                    "all resolved addresses for {} are private/reserved",
                    host
                )
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DnsCacheKey {
    host: String,
    port: u16,
    allow_private: bool,
}

struct DnsCacheEntry {
    addrs: Arc<Vec<SocketAddr>>,
    expires_at: Instant,
    inserted_at: Instant,
}

/// Lightweight DNS cache with TTL + capacity bounds.
/// Stores validated addresses per host and port so ACL checks and connection
/// setup can reuse the same resolution result.
pub struct DnsCache {
    ttl: Duration,
    capacity: usize,
    entries: RwLock<HashMap<DnsCacheKey, DnsCacheEntry>>,
}

impl DnsCache {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Look up cached public addresses for a host + port.
    ///
    /// This compatibility wrapper uses the restrictive (public-only) policy.
    /// Callers that explicitly allow private targets must use
    /// [`Self::get_for_policy`] so entries cannot cross the policy boundary.
    #[allow(dead_code)]
    pub async fn get(&self, host: &str, port: u16) -> Option<Arc<Vec<SocketAddr>>> {
        self.get_for_policy(host, port, false).await
    }

    /// Look up a cached resolution under the exact target policy used to
    /// validate it.  Private-target and public-only resolutions are kept in
    /// separate entries; otherwise a cache populated while private targets
    /// are enabled could bypass filtering after a policy change.
    pub async fn get_for_policy(
        &self,
        host: &str,
        port: u16,
        allow_private: bool,
    ) -> Option<Arc<Vec<SocketAddr>>> {
        if self.capacity == 0 || self.ttl.is_zero() {
            return None;
        }
        let key = Self::key(host, port, allow_private);
        let now = Instant::now();

        // Fast path: read lock for cache hit
        {
            let entries = self.entries.read().await;
            match entries.get(&key) {
                Some(entry) if entry.expires_at > now => return Some(Arc::clone(&entry.addrs)),
                None => return None,
                Some(_) => {} // expired, fall through to evict
            }
        }

        // Slow path: write lock to remove expired entry
        let mut entries = self.entries.write().await;
        entries.remove(&key);
        None
    }

    /// Insert resolved public addresses into the restrictive (public-only)
    /// cache.  This compatibility wrapper preserves the original API;
    /// policy-aware callers should use [`Self::insert_for_policy`].
    #[allow(dead_code)]
    pub async fn insert(&self, host: &str, port: u16, addrs: Arc<Vec<SocketAddr>>) {
        self.insert_for_policy(host, port, false, addrs).await;
    }

    /// Insert addresses under the exact target policy that produced them.
    pub async fn insert_for_policy(
        &self,
        host: &str,
        port: u16,
        allow_private: bool,
        addrs: Arc<Vec<SocketAddr>>,
    ) {
        if self.capacity == 0 || self.ttl.is_zero() || addrs.is_empty() {
            return;
        }
        let key = Self::key(host, port, allow_private);
        let now = Instant::now();
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| entry.expires_at > now);
        while entries.len() >= self.capacity {
            let oldest_key = entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone());
            if let Some(key) = oldest_key {
                entries.remove(&key);
            } else {
                break;
            }
        }
        entries.insert(
            key,
            DnsCacheEntry {
                addrs,
                expires_at: now + self.ttl,
                inserted_at: now,
            },
        );
    }

    fn key(host: &str, port: u16, allow_private: bool) -> DnsCacheKey {
        DnsCacheKey {
            host: host.to_ascii_lowercase(),
            port,
            allow_private,
        }
    }
}

/// Resolve a hostname to validated socket addresses.
///
/// Results are cached in `dns_cache`. Private/reserved IPs are filtered out
/// unless `allow_private` is enabled. Returns an error if filtering removes
/// every resolved address.
pub async fn resolve_public_addrs(
    host: &str,
    port: u16,
    allow_private: bool,
    dns_cache: &DnsCache,
) -> Result<Vec<SocketAddr>, FilterError> {
    // Cache hit
    if let Some(addrs) = dns_cache.get_for_policy(host, port, allow_private).await {
        return Ok((*addrs).clone());
    }

    // Async DNS resolution.  Keep resolver wait time and answer count
    // bounded before applying the private-address policy below.
    let resolved: Vec<SocketAddr> =
        aether_http::lookup_host_with_limits(host, port, aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .map_err(|_| FilterError::DnsResolutionFailed(host.to_string()))?;

    if resolved.is_empty() {
        return Err(FilterError::DnsResolutionFailed(host.to_string()));
    }

    // Filter out private/reserved addresses unless explicitly allowed.
    let public: Vec<SocketAddr> = if allow_private {
        resolved
    } else {
        resolved
            .into_iter()
            .filter(|addr| !is_private_ip(&addr.ip()))
            .collect()
    };

    if public.is_empty() {
        return Err(FilterError::NoPublicAddrs(host.to_string()));
    }

    // Cache the validated public addresses
    let arc_addrs = Arc::new(public);
    dns_cache
        .insert_for_policy(host, port, allow_private, Arc::clone(&arc_addrs))
        .await;
    Ok((*arc_addrs).clone())
}

/// Validate that the target host:port is allowed.
///
/// Performs port whitelist check, private IP filtering, and DNS resolution
/// with caching. The caller must use the returned addresses for the actual
/// connection rather than resolving the hostname again.
pub async fn validate_target(
    host: &str,
    port: u16,
    allowed_ports: &HashSet<u16>,
    allow_private: bool,
    dns_cache: &DnsCache,
) -> Result<Vec<SocketAddr>, FilterError> {
    if let Some(address) = validate_target_literal(host, port, allowed_ports, allow_private)? {
        return Ok(vec![address]);
    }

    resolve_public_addrs(host, port, allow_private, dns_cache).await
}

pub(crate) fn validate_target_literal(
    host: &str,
    port: u16,
    allowed_ports: &HashSet<u16>,
    allow_private: bool,
) -> Result<Option<SocketAddr>, FilterError> {
    if !allowed_ports.contains(&port) {
        return Err(FilterError::PortNotAllowed(port));
    }

    // Try parsing as IP directly (no DNS needed)
    if let Some(ip) = aether_http::parse_ip_literal_host(host) {
        if !allow_private && is_private_ip(&ip) {
            return Err(FilterError::PrivateIp(ip));
        }
        return Ok(Some(SocketAddr::new(ip, port)));
    }
    if !allow_private && host.trim_end_matches('.').eq_ignore_ascii_case("localhost") {
        return Err(FilterError::NoPublicAddrs(host.to_string()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn ports() -> HashSet<u16> {
        [80, 443, 8080, 8443].into_iter().collect()
    }

    fn cache() -> DnsCache {
        DnsCache::new(Duration::from_secs(60), 128)
    }

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        // CGNAT
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(
            100, 127, 255, 254
        ))));
        assert!(!is_private_ip(&IpAddr::V4(Ipv4Addr::new(
            100, 63, 255, 254
        ))));
        // Benchmark testing
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));
        // Reserved
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        // Multicast
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))));
        // Public
        assert!(!is_private_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_private_ip(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn test_private_ipv6() {
        assert!(is_private_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_private_ip(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        // fc00::1 (ULA)
        assert!(is_private_ip(&IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fe80::1 (link-local)
        assert!(is_private_ip(&IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fec0::/10 (deprecated site-local)
        assert!(is_private_ip(&"fec0::1".parse().unwrap()));
        assert!(is_private_ip(
            &"feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()
        ));
        // ff00::/8 (multicast)
        assert!(is_private_ip(&IpAddr::V6(Ipv6Addr::new(
            0xff02, 0, 0, 0, 0, 0, 0, 1
        ))));
        // NAT64 well-known and local-use prefixes.
        assert!(is_private_ip(&"64:ff9b::10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"64:ff9b::ffff:ffff".parse().unwrap()));
        assert!(is_private_ip(&"64:ff9b:1::10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(
            &"64:ff9b:1:ffff:ffff:ffff:ffff:ffff".parse().unwrap()
        ));
        assert!(!is_private_ip(&"64:ff9a:ffff::1".parse().unwrap()));
        assert!(!is_private_ip(&"64:ff9b:0:1::1".parse().unwrap()));
        assert!(!is_private_ip(&"64:ff9b:2::1".parse().unwrap()));

        // IPv6 transition formats with embedded IPv4 addresses.
        assert!(is_private_ip(&"2002:0a00:0001::1".parse().unwrap()));
        assert!(is_private_ip(
            &"2002:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()
        ));
        assert!(!is_private_ip(&"2003::1".parse().unwrap()));
        assert!(is_private_ip(
            &"2001:0000:4136:e378:8000:63bf:3fff:fdd2".parse().unwrap()
        ));
        assert!(!is_private_ip(&"2001:1::1".parse().unwrap()));
        assert!(is_private_ip(&"::192.0.2.1".parse().unwrap()));
        assert!(is_private_ip(&"::ffff:0:192.0.2.1".parse().unwrap()));
        assert!(is_private_ip(&"2001:db8::5efe:10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(
            &"2001:db8::200:5efe:192.0.2.1".parse().unwrap()
        ));

        // IPv4-mapped public addresses remain allowed, while private mapped
        // addresses continue through the IPv4 classification.
        assert!(!is_private_ip(&"::ffff:8.8.8.8".parse().unwrap()));
        assert!(is_private_ip(&"::ffff:127.0.0.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn test_port_not_allowed() {
        let cache = cache();
        let result = validate_target("8.8.8.8", 22, &ports(), false, &cache).await;
        assert!(matches!(result, Err(FilterError::PortNotAllowed(22))));
    }

    #[tokio::test]
    async fn test_private_ip_blocked() {
        let cache = cache();
        let result = validate_target("127.0.0.1", 80, &ports(), false, &cache).await;
        assert!(matches!(result, Err(FilterError::PrivateIp(_))));
    }

    #[tokio::test]
    async fn test_ipv6_site_local_blocked_unless_private_targets_allowed() {
        let cache = cache();
        let result = validate_target("fec0::1", 443, &ports(), false, &cache).await;
        assert!(matches!(
            result,
            Err(FilterError::PrivateIp(IpAddr::V6(ip))) if ip == "fec0::1".parse::<Ipv6Addr>().unwrap()
        ));

        let result = validate_target("fec0::1", 443, &ports(), true, &cache)
            .await
            .unwrap();
        assert_eq!(
            result,
            vec![SocketAddr::new(IpAddr::V6("fec0::1".parse().unwrap()), 443)]
        );
    }

    #[tokio::test]
    async fn test_public_ip_allowed() {
        let cache = cache();
        let result = validate_target("8.8.8.8", 443, &ports(), false, &cache).await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip(), IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[tokio::test]
    async fn test_private_ip_allowed_when_enabled() {
        let cache = cache();
        let result = validate_target("127.0.0.1", 80, &ports(), true, &cache).await;
        assert!(result.is_ok());
        let addrs = result.unwrap();
        assert_eq!(
            addrs,
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 80)]
        );
    }

    #[tokio::test]
    async fn test_localhost_hostname_blocked_by_default() {
        let cache = cache();
        let result = validate_target("localhost", 80, &ports(), false, &cache).await;
        assert!(matches!(result, Err(FilterError::NoPublicAddrs(_))));
    }

    #[tokio::test]
    async fn test_localhost_hostname_allowed_when_enabled() {
        let cache = cache();
        let result = validate_target("localhost", 80, &ports(), true, &cache).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_cache_stores_multiple_addrs() {
        let cache = cache();
        let addrs = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)), 443),
        ];
        cache
            .insert("example.com", 443, Arc::new(addrs.clone()))
            .await;
        let cached = cache.get("example.com", 443).await.unwrap();
        assert_eq!(*cached, addrs);
    }

    #[tokio::test]
    async fn test_cache_key_case_insensitive() {
        let cache = cache();
        let addrs = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443)];
        cache
            .insert("Example.COM", 443, Arc::new(addrs.clone()))
            .await;
        let cached = cache.get("example.com", 443).await.unwrap();
        assert_eq!(*cached, addrs);
    }

    #[tokio::test]
    async fn test_cache_does_not_cross_private_target_policy() {
        let cache = cache();
        let private = Arc::new(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 80)]);
        let public = Arc::new(vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            80,
        )]);

        cache
            .insert_for_policy("example.com", 80, true, Arc::clone(&private))
            .await;
        assert!(cache
            .get_for_policy("example.com", 80, false)
            .await
            .is_none());
        assert_eq!(
            *cache
                .get_for_policy("example.com", 80, true)
                .await
                .expect("private-policy entry"),
            *private
        );

        cache
            .insert_for_policy("example.com", 80, false, Arc::clone(&public))
            .await;
        assert_eq!(
            *cache
                .get_for_policy("EXAMPLE.COM", 80, false)
                .await
                .expect("public-policy entry"),
            *public
        );
    }
}
