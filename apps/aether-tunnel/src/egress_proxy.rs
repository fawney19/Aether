use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use socket2::{SockRef, TcpKeepalive};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpFamily {
    Any,
    Ipv4Only,
    Ipv6Only,
}

impl IpFamily {
    pub(crate) fn allows(self, addr: SocketAddr) -> bool {
        match self {
            Self::Any => true,
            Self::Ipv4Only => addr.is_ipv4(),
            Self::Ipv6Only => addr.is_ipv6(),
        }
    }

    pub(crate) fn no_address_message(self, context: &str) -> String {
        match self {
            Self::Any => format!("{context} DNS returned no addresses"),
            Self::Ipv4Only => format!("{context} DNS returned no IPv4 addresses"),
            Self::Ipv6Only => format!("{context} DNS returned no IPv6 addresses"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamProxyScheme {
    Http,
    Socks5,
    Socks5h,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpstreamProxyConfig {
    raw: String,
    scheme: UpstreamProxyScheme,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

impl UpstreamProxyConfig {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("upstream proxy URL must not be empty".to_string());
        }

        let parsed =
            Url::parse(trimmed).map_err(|err| format!("invalid upstream proxy URL: {err}"))?;
        let scheme = match parsed.scheme().to_ascii_lowercase().as_str() {
            "http" => UpstreamProxyScheme::Http,
            "socks5" => UpstreamProxyScheme::Socks5,
            "socks5h" => UpstreamProxyScheme::Socks5h,
            other => {
                return Err(format!(
                    "unsupported upstream proxy scheme `{other}`; use http, socks5, or socks5h"
                ))
            }
        };
        let host = parsed
            .host_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "upstream proxy URL must include a host".to_string())?
            .to_string();
        let port = parsed.port().unwrap_or(match scheme {
            UpstreamProxyScheme::Http => 80,
            UpstreamProxyScheme::Socks5 | UpstreamProxyScheme::Socks5h => 1080,
        });
        let username = non_empty_url_part(parsed.username());
        let password = parsed.password().and_then(non_empty_url_part);

        Ok(Self {
            raw: trimmed.to_string(),
            scheme,
            host,
            port,
            username,
            password,
        })
    }

    pub(crate) fn scheme(&self) -> UpstreamProxyScheme {
        self.scheme
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub(crate) fn uses_remote_dns(&self) -> bool {
        self.scheme == UpstreamProxyScheme::Socks5h
    }

    pub(crate) fn basic_auth_header(&self) -> Option<String> {
        let username = self.username()?;
        let mut credentials = String::with_capacity(
            username.len() + self.password.as_ref().map(|value| value.len()).unwrap_or(0) + 1,
        );
        credentials.push_str(username);
        credentials.push(':');
        if let Some(password) = self.password() {
            credentials.push_str(password);
        }
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        ))
    }

    pub(crate) fn redacted_url(&self) -> String {
        let Ok(mut parsed) = Url::parse(&self.raw) else {
            return "<invalid>".to_string();
        };
        if !parsed.username().is_empty() {
            let _ = parsed.set_username("****");
        }
        if parsed.password().is_some() {
            let _ = parsed.set_password(Some("****"));
        }
        parsed.to_string()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProxyConnectOptions {
    pub connect_timeout: Duration,
    pub tcp_nodelay: bool,
    pub tcp_keepalive: Option<Duration>,
    pub ip_family: IpFamily,
}

pub(crate) async fn connect_target_via_proxy(
    proxy: &UpstreamProxyConfig,
    target_host: &str,
    target_port: u16,
    options: ProxyConnectOptions,
) -> io::Result<TcpStream> {
    let mut tcp = connect_proxy_tcp(
        proxy,
        options.connect_timeout,
        options.tcp_nodelay,
        options.tcp_keepalive,
        options.ip_family,
    )
    .await?;

    match proxy.scheme() {
        UpstreamProxyScheme::Http => {
            http_connect(&mut tcp, &target_authority(target_host, target_port), proxy).await?;
        }
        UpstreamProxyScheme::Socks5 | UpstreamProxyScheme::Socks5h => {
            socks5_connect(&mut tcp, proxy, target_host, target_port).await?;
        }
    }

    Ok(tcp)
}

pub(crate) async fn connect_proxy_tcp(
    proxy: &UpstreamProxyConfig,
    connect_timeout: Duration,
    tcp_nodelay: bool,
    tcp_keepalive: Option<Duration>,
    ip_family: IpFamily,
) -> io::Result<TcpStream> {
    let resolved = tokio::time::timeout(
        connect_timeout,
        tokio::net::lookup_host((proxy.host(), proxy.port())),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy DNS timeout"))?
    .map_err(|err| io::Error::other(format!("proxy DNS failed: {err}")))?;

    let mut last_error = None;
    for addr in resolved.filter(|addr| ip_family.allows(*addr)) {
        match tokio::time::timeout(connect_timeout, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                configure_tcp_stream(&stream, tcp_nodelay, tcp_keepalive)?;
                return Ok(stream);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => {
                last_error = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("proxy connect timeout: {addr}"),
                ));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| io::Error::other(ip_family.no_address_message("proxy"))))
}

fn configure_tcp_stream(
    stream: &TcpStream,
    tcp_nodelay: bool,
    tcp_keepalive: Option<Duration>,
) -> io::Result<()> {
    stream.set_nodelay(tcp_nodelay)?;
    if let Some(keepalive) = tcp_keepalive {
        let keepalive = TcpKeepalive::new().with_time(keepalive);
        SockRef::from(stream).set_tcp_keepalive(&keepalive)?;
    }
    Ok(())
}

pub(crate) async fn http_connect(
    stream: &mut TcpStream,
    target_authority: &str,
    proxy: &UpstreamProxyConfig,
) -> io::Result<()> {
    let mut request = format!(
        "CONNECT {target_authority} HTTP/1.1\r\nHost: {target_authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(auth) = proxy.basic_auth_header() {
        request.push_str("Proxy-Authorization: ");
        request.push_str(&auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if response.len() >= 16 * 1024 {
            return Err(io::Error::other("proxy CONNECT response too large"));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy closed during CONNECT",
            ));
        }
        response.extend_from_slice(&chunk[..n]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    let status_line_end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| io::Error::other("proxy CONNECT response missing status line"))?;
    let status_line = std::str::from_utf8(&response[..status_line_end])
        .map_err(|_| io::Error::other("proxy CONNECT status line is not UTF-8"))?;
    let status = status_line.split_whitespace().nth(1).unwrap_or_default();
    if status == "200" {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "proxy CONNECT failed: {status_line}"
        )))
    }
}

pub(crate) async fn socks5_connect(
    stream: &mut TcpStream,
    proxy: &UpstreamProxyConfig,
    target_host: &str,
    target_port: u16,
) -> io::Result<()> {
    let requires_auth = proxy.username().is_some();
    if requires_auth {
        stream.write_all(&[0x05, 0x02, 0x00, 0x02]).await?;
    } else {
        stream.write_all(&[0x05, 0x01, 0x00]).await?;
    }

    let mut method_response = [0u8; 2];
    stream.read_exact(&mut method_response).await?;
    if method_response[0] != 0x05 {
        return Err(io::Error::other("invalid SOCKS5 method response"));
    }
    match method_response[1] {
        0x00 => {}
        0x02 => socks5_authenticate(stream, proxy).await?,
        0xff => return Err(io::Error::other("SOCKS5 proxy rejected all auth methods")),
        method => {
            return Err(io::Error::other(format!(
                "SOCKS5 proxy selected unsupported auth method 0x{method:02x}"
            )))
        }
    }

    let address = socks5_target_address(target_host, target_port, proxy.uses_remote_dns()).await?;
    stream.write_all(&address).await?;

    let mut response = [0u8; 4];
    stream.read_exact(&mut response).await?;
    if response[0] != 0x05 {
        return Err(io::Error::other("invalid SOCKS5 connect response"));
    }
    if response[1] != 0x00 {
        return Err(io::Error::other(format!(
            "SOCKS5 connect failed: {}",
            socks5_reply_message(response[1])
        )));
    }

    match response[3] {
        0x01 => {
            let mut ignored = [0u8; 4 + 2];
            stream.read_exact(&mut ignored).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut ignored = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut ignored).await?;
        }
        0x04 => {
            let mut ignored = [0u8; 16 + 2];
            stream.read_exact(&mut ignored).await?;
        }
        atyp => {
            return Err(io::Error::other(format!(
                "SOCKS5 proxy returned unsupported address type 0x{atyp:02x}"
            )))
        }
    }

    Ok(())
}

async fn socks5_authenticate(
    stream: &mut TcpStream,
    proxy: &UpstreamProxyConfig,
) -> io::Result<()> {
    let username = proxy.username().unwrap_or_default().as_bytes();
    let password = proxy.password().unwrap_or_default().as_bytes();
    if username.len() > u8::MAX as usize || password.len() > u8::MAX as usize {
        return Err(io::Error::other(
            "SOCKS5 username/password must be at most 255 bytes",
        ));
    }

    let mut request = Vec::with_capacity(username.len() + password.len() + 3);
    request.push(0x01);
    request.push(username.len() as u8);
    request.extend_from_slice(username);
    request.push(password.len() as u8);
    request.extend_from_slice(password);
    stream.write_all(&request).await?;

    let mut response = [0u8; 2];
    stream.read_exact(&mut response).await?;
    if response[0] != 0x01 || response[1] != 0x00 {
        return Err(io::Error::other("SOCKS5 username/password auth failed"));
    }
    Ok(())
}

pub(crate) async fn socks5_target_address(
    target_host: &str,
    target_port: u16,
    remote_dns: bool,
) -> io::Result<Vec<u8>> {
    let mut request = vec![0x05, 0x01, 0x00];
    if let Ok(ip) = target_host.parse::<IpAddr>() {
        push_socks5_ip_address(&mut request, ip);
    } else if remote_dns {
        let host = target_host.as_bytes();
        if host.len() > u8::MAX as usize {
            return Err(io::Error::other("SOCKS5 target hostname is too long"));
        }
        request.push(0x03);
        request.push(host.len() as u8);
        request.extend_from_slice(host);
    } else {
        let mut resolved = tokio::net::lookup_host((target_host, target_port))
            .await
            .map_err(|err| io::Error::other(format!("SOCKS5 target DNS failed: {err}")))?;
        let addr = resolved
            .next()
            .ok_or_else(|| io::Error::other("SOCKS5 target DNS returned no addresses"))?;
        push_socks5_socket_address(&mut request, addr);
    }
    request.extend_from_slice(&target_port.to_be_bytes());
    Ok(request)
}

fn push_socks5_socket_address(request: &mut Vec<u8>, addr: SocketAddr) {
    push_socks5_ip_address(request, addr.ip());
}

fn push_socks5_ip_address(request: &mut Vec<u8>, ip: IpAddr) {
    match ip {
        IpAddr::V4(ip) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            request.push(0x04);
            request.extend_from_slice(&ip.octets());
        }
    }
}

fn socks5_reply_message(reply: u8) -> &'static str {
    match reply {
        0x01 => "general failure",
        0x02 => "connection not allowed",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown error",
    }
}

pub(crate) fn target_authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn non_empty_url_part(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_proxy_with_default_port() {
        let proxy = UpstreamProxyConfig::parse("http://proxy.example").expect("proxy should parse");

        assert_eq!(proxy.scheme(), UpstreamProxyScheme::Http);
        assert_eq!(proxy.host(), "proxy.example");
        assert_eq!(proxy.port(), 80);
    }

    #[test]
    fn parses_socks5h_proxy_with_auth() {
        let proxy = UpstreamProxyConfig::parse("socks5h://user:pass@127.0.0.1:1080")
            .expect("proxy should parse");

        assert_eq!(proxy.scheme(), UpstreamProxyScheme::Socks5h);
        assert_eq!(proxy.username(), Some("user"));
        assert_eq!(proxy.password(), Some("pass"));
        assert!(proxy.uses_remote_dns());
        assert_eq!(
            proxy.basic_auth_header().as_deref(),
            Some("Basic dXNlcjpwYXNz")
        );
    }

    #[test]
    fn rejects_unsupported_proxy_scheme() {
        let error = UpstreamProxyConfig::parse("https://proxy.example:8443")
            .expect_err("https proxy scheme should be rejected");

        assert!(error.contains("unsupported upstream proxy scheme"));
    }
}
