use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Maximum number of addresses accepted from one hostname lookup.
///
/// A DNS response is attacker-controlled at the resolver boundary.  Keeping
/// the result bounded prevents a pathological answer from forcing an
/// unbounded vector allocation or an unbounded connect fan-out.  Callers still
/// validate every returned address for their own network policy.
pub const MAX_DNS_RESOLVED_ADDRESSES: usize = 32;

/// Upper bound used by callers that do not have a tighter request deadline.
pub const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

pub fn parse_ip_literal_host(host: &str) -> Option<IpAddr> {
    host.parse().ok().or_else(|| {
        host.strip_prefix('[')?
            .strip_suffix(']')?
            .parse::<Ipv6Addr>()
            .ok()
            .map(IpAddr::V6)
    })
}

/// Resolve a host while bounding both resolver wait time and answer count.
///
/// The iterator is consumed one item past the allowed count so an answer set
/// larger than the policy is rejected rather than silently truncated.  This
/// keeps validation and connection pinning based on the complete, bounded
/// answer set.
pub async fn lookup_host_with_limits(
    host: &str,
    port: u16,
    timeout: Duration,
) -> io::Result<Vec<SocketAddr>> {
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "DNS lookup timeout must be non-zero",
        ));
    }
    if let Some(ip) = parse_ip_literal_host(host) {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let mut resolved = tokio::time::timeout(timeout, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "DNS lookup timed out"))??;

    collect_resolved_addresses_with_limit(&mut resolved)
}

fn collect_resolved_addresses_with_limit(
    resolved: &mut impl Iterator<Item = SocketAddr>,
) -> io::Result<Vec<SocketAddr>> {
    let mut addresses = Vec::with_capacity(MAX_DNS_RESOLVED_ADDRESSES.min(8));
    for address in resolved.by_ref() {
        if addresses.len() >= MAX_DNS_RESOLVED_ADDRESSES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DNS lookup returned too many addresses",
            ));
        }
        addresses.push(address);
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{
        collect_resolved_addresses_with_limit, lookup_host_with_limits, parse_ip_literal_host,
        DEFAULT_DNS_LOOKUP_TIMEOUT, MAX_DNS_RESOLVED_ADDRESSES,
    };

    #[test]
    fn ip_literal_parser_does_not_turn_bracketed_names_into_hosts() {
        for host in [
            "localhost",
            "[localhost]",
            "[127.0.0.1]",
            "[::1",
            "::1]",
            "[[::1]]",
        ] {
            assert_eq!(parse_ip_literal_host(host), None, "{host}");
        }
        for (host, expected) in [
            ("198.18.78.41", "198.18.78.41"),
            ("::1", "::1"),
            ("[::1]", "::1"),
            ("[::ffff:127.0.0.1]", "::ffff:127.0.0.1"),
        ] {
            assert_eq!(parse_ip_literal_host(host), Some(expected.parse().unwrap()));
        }
    }

    #[tokio::test]
    async fn rejects_zero_dns_timeout_before_resolving() {
        let error = lookup_host_with_limits("localhost", 80, std::time::Duration::ZERO)
            .await
            .expect_err("zero timeout must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn resolves_bracketed_ipv6_without_a_hostname_lookup() {
        for host in ["[::1]", "[2606:4700:4700::1111]", "[fd00::1]"] {
            let addresses = lookup_host_with_limits(host, 8443, DEFAULT_DNS_LOOKUP_TIMEOUT)
                .await
                .expect("URL-form IPv6 literals must not be sent to DNS");
            assert_eq!(addresses, vec![format!("{host}:8443").parse().unwrap()]);
        }
    }

    #[tokio::test]
    async fn resolves_within_shared_address_bound() {
        let addresses = lookup_host_with_limits("localhost", 80, DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .expect("localhost should resolve in the test environment");
        assert!(!addresses.is_empty());
        assert!(addresses.len() <= MAX_DNS_RESOLVED_ADDRESSES);
    }

    #[test]
    fn preserves_every_answer_at_the_shared_bound() {
        let expected = (0..MAX_DNS_RESOLVED_ADDRESSES)
            .map(|index| SocketAddr::from(([198, 18, 0, index as u8], 443)))
            .collect::<Vec<_>>();
        let mut resolved = expected.clone().into_iter();
        assert_eq!(
            collect_resolved_addresses_with_limit(&mut resolved).unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_an_answer_set_larger_than_the_shared_bound() {
        let mut resolved = (0..=MAX_DNS_RESOLVED_ADDRESSES)
            .map(|index| SocketAddr::from(([192, 0, 2, (index % 254 + 1) as u8], 443)));
        let error = collect_resolved_addresses_with_limit(&mut resolved)
            .expect_err("more than 32 DNS answers must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
