use std::collections::BTreeSet;
use std::net::IpAddr;

use url::{Host, Url};

/// Parse the case-insensitive field names nominated by HTTP/1 `Connection`
/// headers. Those fields are hop-by-hop even when their names are otherwise
/// application-defined.
pub fn connection_declared_header_names<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<String> {
    values
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| valid_http_token(value))
        .map(str::to_ascii_lowercase)
        .collect()
}

fn valid_http_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Return true for addresses that an untrusted URL must not be allowed to
/// reach, including private/link-local ranges and IPv6 transition formats
/// that can embed an IPv4 destination.
pub fn is_private_or_reserved_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.is_multicast()
                // The complete 0.0.0.0/8 block is reserved for "this
                // network" destinations.  `Ipv4Addr::is_unspecified()`
                // only covers the single 0.0.0.0 address.
                || octets[0] == 0
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || octets[0] >= 240
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_private_or_reserved_ip(IpAddr::V4(mapped));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || (segments[0] & 0xffc0 == 0xfec0)
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0]
                || segments[..3] == [0x0064, 0xff9b, 0x0001]
                || segments[0] == 0x2002
                || segments[..2] == [0x2001, 0]
                || segments[..6] == [0, 0, 0, 0, 0, 0]
                || segments[..6] == [0, 0, 0, 0, 0xffff, 0]
                || (matches!(segments[4], 0 | 0x0200) && segments[5] == 0x5efe)
        }
    }
}

/// Return whether an address belongs to RFC 2544's IPv4 benchmarking range.
///
/// Local DNS interception tools commonly synthesize answers from
/// `198.18.0.0/15`. The range is intentionally still
/// classified as reserved by [`is_private_or_reserved_ip`]; callers may use
/// this predicate only when they have independently established that the
/// hostname is a trusted, fixed destination.  Keeping the predicates separate
/// prevents a compatibility exception from weakening the generic SSRF guard.
pub fn is_ipv4_benchmarking_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets[0] == 198 && (18..=19).contains(&octets[1])
        }
        IpAddr::V6(_) => false,
    }
}

/// Return true only when a URL names loopback without relying on DNS.
///
/// This is intentionally stricter than accepting names that currently resolve
/// to loopback: DNS answers can change between validation and connection.
pub fn url_has_literal_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Sensitive HTTP traffic may use cleartext only for a literal loopback host.
pub fn is_https_or_loopback_http_url(url: &Url) -> bool {
    url.scheme() == "https" || (url.scheme() == "http" && url_has_literal_loopback_host(url))
}

#[cfg(test)]
mod tests {
    use super::{
        connection_declared_header_names, is_https_or_loopback_http_url,
        is_ipv4_benchmarking_fake_ip, is_private_or_reserved_ip, url_has_literal_loopback_host,
    };

    #[test]
    fn parses_multiple_connection_values_and_rejects_invalid_names() {
        let names = connection_declared_header_names([
            "keep-alive, X-Private",
            "x-accel-redirect, invalid name, x-private",
        ]);

        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["keep-alive", "x-accel-redirect", "x-private"]
        );
    }

    #[test]
    fn blocks_private_and_transition_addresses_but_allows_public_addresses() {
        for address in [
            "127.0.0.1",
            "0.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "64:ff9b::10.0.0.1",
            "2002:0a00:0001::1",
            "2001:0000:4136:e378:8000:63bf:3fff:fdd2",
        ] {
            assert!(
                is_private_or_reserved_ip(address.parse().expect("IP address")),
                "address should be blocked: {address}"
            );
        }
        assert!(!is_private_or_reserved_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_or_reserved_ip(
            "2606:4700:4700::1111".parse().unwrap()
        ));
    }

    #[test]
    fn benchmarking_fake_ip_predicate_is_narrow_and_does_not_change_private_policy() {
        for address in ["198.18.0.1", "198.19.255.254"] {
            let ip = address.parse().expect("benchmarking address");
            assert!(is_ipv4_benchmarking_fake_ip(ip));
            assert!(is_private_or_reserved_ip(ip));
        }
        for address in ["198.17.255.254", "198.20.0.1", "2001:db8::1"] {
            let ip = address.parse().expect("non-benchmarking address");
            assert!(!is_ipv4_benchmarking_fake_ip(ip));
        }
    }

    #[test]
    fn sensitive_http_transport_allows_https_or_literal_loopback_only() {
        for allowed in [
            "https://api.example.test/v1",
            "http://localhost:8080/v1",
            "http://127.42.0.1:8080/v1",
            "http://[::1]:8080/v1",
        ] {
            let url = url::Url::parse(allowed).unwrap();
            assert!(is_https_or_loopback_http_url(&url), "rejected {allowed}");
        }

        for rejected in [
            "http://api.example.test/v1",
            "http://10.0.0.1/v1",
            "http://0.0.0.0:8080/v1",
            "http://[::ffff:127.0.0.1]:8080/v1",
            "ftp://localhost/resource",
        ] {
            let url = url::Url::parse(rejected).unwrap();
            assert!(!is_https_or_loopback_http_url(&url), "accepted {rejected}");
        }

        assert!(url_has_literal_loopback_host(
            &url::Url::parse("https://localhost/").unwrap()
        ));
        assert!(!url_has_literal_loopback_host(
            &url::Url::parse("https://localhost.example/").unwrap()
        ));
    }
}
