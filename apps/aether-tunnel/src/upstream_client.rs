use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use aether_contracts::{
    ResolvedTransportProfile, TRANSPORT_BACKEND_HYPER_RUSTLS, TRANSPORT_BACKEND_REQWEST_RUSTLS,
    TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
};
use bytes::Bytes;
use futures_util::Stream;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::rt;
use hyper::Response;
use hyper::Uri;
pub use hyper_util::client::legacy::connect::capture_connection;
use hyper_util::client::legacy::connect::dns::Name;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tower_service::Service;

use crate::config::Config;
use crate::egress_proxy::{
    connect_target_via_proxy, connect_validated_target_via_proxy, ProxyConnectOptions,
    UpstreamProxyConfig,
};
use crate::target_filter::DnsCache;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

type PlainStream = TokioIo<TcpStream>;
type TlsStream = TokioIo<tokio_rustls::client::TlsStream<TcpStream>>;

pub type UpstreamRequestBody = UnsyncBoxBody<Bytes, io::Error>;
pub type UpstreamClient = Client<InstrumentedConnector, UpstreamRequestBody>;

const DEFAULT_PROFILE_ID: &str = "default";
const DEFAULT_BACKEND: &str = TRANSPORT_BACKEND_HYPER_RUSTLS;
const DEFAULT_HTTP_MODE: &str = "auto";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UpstreamClientPoolKey {
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub profile_id: String,
    pub backend: String,
    pub http_mode: String,
    pub validated_target: ValidatedUpstreamTarget,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ValidatedUpstreamTarget {
    scheme: String,
    host: String,
    port: u16,
    resolution: UpstreamTargetResolution,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum UpstreamTargetResolution {
    Pinned(Vec<SocketAddr>),
    ProxyDns,
}

impl ValidatedUpstreamTarget {
    pub fn new(target_url: &url::Url, mut addrs: Vec<SocketAddr>) -> Result<Self, String> {
        if addrs.is_empty() {
            return Err("validated upstream target has no addresses".to_string());
        }
        addrs.sort_unstable();
        addrs.dedup();
        Self::with_resolution(target_url, UpstreamTargetResolution::Pinned(addrs))
    }

    pub(crate) fn proxy_resolved(target_url: &url::Url) -> Result<Self, String> {
        if !matches!(target_url.host(), Some(url::Host::Domain(_))) {
            return Err("IP literal targets must use pinned addresses".to_string());
        }
        Self::with_resolution(target_url, UpstreamTargetResolution::ProxyDns)
    }

    fn with_resolution(
        target_url: &url::Url,
        resolution: UpstreamTargetResolution,
    ) -> Result<Self, String> {
        if !target_url.username().is_empty()
            || target_url.password().is_some()
            || target_url.fragment().is_some()
        {
            return Err("upstream target must not contain credentials or a fragment".to_string());
        }
        let scheme = target_url.scheme().to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err(format!("unsupported upstream scheme {scheme}"));
        }
        let host = target_url
            .host_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "missing host in upstream URL".to_string())?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        let port = target_url
            .port_or_known_default()
            .ok_or_else(|| "missing port in upstream URL".to_string())?;
        if let UpstreamTargetResolution::Pinned(addrs) = &resolution {
            if addrs.iter().any(|addr| addr.port() != port) {
                return Err("validated upstream target address has the wrong port".to_string());
            }
        }
        Ok(Self {
            scheme,
            host,
            port,
            resolution,
        })
    }

    fn ensure_matches_uri(&self, uri: &Uri) -> Result<(), io::Error> {
        let scheme = uri
            .scheme_str()
            .ok_or_else(|| io::Error::other("missing scheme"))?;
        let host = uri_host(uri)?;
        let port = uri_port_or_default(uri, scheme)?;
        if !scheme.eq_ignore_ascii_case(&self.scheme)
            || !host.eq_ignore_ascii_case(&self.host)
            || port != self.port
        {
            return Err(io::Error::other(
                "upstream connector target does not match its validated origin",
            ));
        }
        Ok(())
    }

    pub(crate) fn uses_proxy_dns(&self) -> bool {
        matches!(self.resolution, UpstreamTargetResolution::ProxyDns)
    }
}

#[derive(Clone)]
pub struct UpstreamClientPool {
    config: Arc<Config>,
    clients: Arc<Mutex<HashMap<UpstreamClientPoolKey, UpstreamClientPoolEntry>>>,
    access_counter: Arc<AtomicU64>,
}

#[derive(Clone)]
struct UpstreamClientPoolEntry {
    client: UpstreamClient,
    last_used: u64,
}

impl UpstreamClientPool {
    pub fn new(config: Arc<Config>, _dns_cache: Arc<DnsCache>) -> Self {
        Self {
            config,
            clients: Arc::new(Mutex::new(HashMap::new())),
            access_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get_or_build(&self, key: UpstreamClientPoolKey) -> Result<UpstreamClient, String> {
        {
            let mut clients = self.clients.lock().expect("client pool lock");
            if let Some(entry) = clients.get_mut(&key) {
                entry.last_used = self.next_access_id();
                return Ok(entry.client.clone());
            }
        }

        validate_proxy_transport_backend(&key.backend)?;
        let http1_only = key
            .http_mode
            .eq_ignore_ascii_case(TRANSPORT_HTTP_MODE_HTTP1_ONLY);
        let h2c_prior_knowledge = !http1_only
            && key
                .http_mode
                .eq_ignore_ascii_case(TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE);
        let client = build_upstream_client_with_protocol(
            &self.config,
            key.validated_target.clone(),
            http1_only,
            h2c_prior_knowledge,
        )?;
        let mut clients = self.clients.lock().expect("client pool lock");
        if let Some(entry) = clients.get_mut(&key) {
            entry.last_used = self.next_access_id();
            return Ok(entry.client.clone());
        }
        evict_lru_client_if_needed(
            &mut clients,
            self.config.upstream_client_pool_capacity.max(1),
        );
        clients.insert(
            key,
            UpstreamClientPoolEntry {
                client: client.clone(),
                last_used: self.next_access_id(),
            },
        );
        Ok(client)
    }

    fn next_access_id(&self) -> u64 {
        self.access_counter.fetch_add(1, Ordering::Relaxed)
    }
}

fn evict_lru_client_if_needed(
    clients: &mut HashMap<UpstreamClientPoolKey, UpstreamClientPoolEntry>,
    capacity: usize,
) {
    if clients.len() < capacity {
        return;
    }
    let Some(oldest_key) = clients
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(key, _)| key.clone())
    else {
        return;
    };
    clients.remove(&oldest_key);
}

pub fn upstream_client_pool_key(
    provider_id: Option<&str>,
    endpoint_id: Option<&str>,
    key_id: Option<&str>,
    profile: Option<&ResolvedTransportProfile>,
    http1_only: bool,
    validated_target: ValidatedUpstreamTarget,
) -> UpstreamClientPoolKey {
    let profile_http_mode = profile
        .map(|profile| profile.http_mode.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_HTTP_MODE);
    let http_mode = if http1_only {
        TRANSPORT_HTTP_MODE_HTTP1_ONLY
    } else {
        profile_http_mode
    };
    UpstreamClientPoolKey {
        provider_id: normalized_pool_key_part(provider_id),
        endpoint_id: normalized_pool_key_part(endpoint_id),
        key_id: normalized_pool_key_part(key_id),
        profile_id: profile
            .map(|profile| profile.profile_id.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_PROFILE_ID)
            .to_string(),
        backend: profile
            .map(|profile| profile.backend.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_BACKEND)
            .to_string(),
        http_mode: http_mode.to_string(),
        validated_target,
    }
}

fn normalized_pool_key_part(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn validate_proxy_transport_backend(backend: &str) -> Result<(), String> {
    if backend.eq_ignore_ascii_case(TRANSPORT_BACKEND_HYPER_RUSTLS)
        || backend.eq_ignore_ascii_case(TRANSPORT_BACKEND_REQWEST_RUSTLS)
    {
        return Ok(());
    }
    Err(format!("unsupported transport profile backend: {backend}"))
}

pub fn stream_request_body<S>(stream: S) -> UpstreamRequestBody
where
    S: Stream<Item = Result<Frame<Bytes>, io::Error>> + Send + 'static,
{
    StreamBody::new(stream).boxed_unsync()
}

#[cfg(test)]
pub fn full_request_body(body: Bytes) -> UpstreamRequestBody {
    http_body_util::Full::new(body)
        .map_err(|err: std::convert::Infallible| match err {})
        .boxed_unsync()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectTiming {
    pub connect_ms: u64,
    pub tls_ms: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RequestTiming {
    pub connection_acquire_ms: u64,
    pub connect_ms: u64,
    pub tls_ms: u64,
    pub response_wait_ms: u64,
    pub connection_reused: bool,
}

#[derive(Clone, Debug)]
struct PinnedResolver {
    target: ValidatedUpstreamTarget,
}

impl PinnedResolver {
    fn new(target: ValidatedUpstreamTarget) -> Self {
        Self { target }
    }
}

pub struct ValidatedAddrs {
    inner: std::vec::IntoIter<std::net::SocketAddr>,
}

impl Iterator for ValidatedAddrs {
    type Item = std::net::SocketAddr;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl Service<Name> for PinnedResolver {
    type Response = ValidatedAddrs;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let requested_host = name.as_str().to_string();
        let target = self.target.clone();
        Box::pin(async move {
            if !requested_host.eq_ignore_ascii_case(&target.host) {
                return Err(io::Error::other(
                    "DNS request does not match the validated upstream host",
                ));
            }
            match target.resolution {
                UpstreamTargetResolution::Pinned(addrs) => Ok(ValidatedAddrs {
                    inner: addrs.into_iter(),
                }),
                UpstreamTargetResolution::ProxyDns => Err(io::Error::other(
                    "proxy-resolved target must not fall back to local DNS",
                )),
            }
        })
    }
}

#[derive(Clone)]
pub struct InstrumentedConnector {
    http: HttpConnector<PinnedResolver>,
    tls_config: Arc<ClientConfig>,
    proxy: Option<UpstreamProxyConfig>,
    validated_target: ValidatedUpstreamTarget,
    connect_timeout: Duration,
    tcp_nodelay: bool,
    tcp_keepalive: Option<Duration>,
}

impl Service<Uri> for InstrumentedConnector {
    type Response = TimedConn;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.http.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, dst: Uri) -> Self::Future {
        if let Err(error) = self.validated_target.ensure_matches_uri(&dst) {
            return Box::pin(async move { Err(error.into()) });
        }
        let scheme = dst.scheme_str().map(|value| value.to_ascii_lowercase());
        let tls_config = Arc::clone(&self.tls_config);
        if let Some(proxy) = self.proxy.clone() {
            let validated_target = self.validated_target.clone();
            let options = ProxyConnectOptions {
                connect_timeout: self.connect_timeout,
                tcp_nodelay: self.tcp_nodelay,
                tcp_keepalive: self.tcp_keepalive,
                ip_family: crate::egress_proxy::IpFamily::Any,
            };
            let connect_start = std::time::Instant::now();
            return Box::pin(async move {
                tokio::time::timeout(
                    options.connect_timeout,
                    connect_via_proxy(
                        dst,
                        scheme,
                        tls_config,
                        proxy,
                        validated_target,
                        options,
                        connect_start,
                    ),
                )
                .await
                .map_err(|_| {
                    Box::new(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "upstream proxy connection timed out",
                    )) as BoxError
                })?
            });
        }
        let connecting = self.http.call(dst.clone());
        let connect_start = std::time::Instant::now();

        Box::pin(async move {
            match scheme.as_deref() {
                Some("http") => {
                    let tcp = connecting.await.map_err(|err| Box::new(err) as BoxError)?;
                    let connect_ms = connect_start.elapsed().as_millis() as u64;
                    Ok(TimedConn::new(
                        MaybeHttpsStream::Http {
                            stream: tcp,
                            is_proxy: false,
                        },
                        ConnectTiming {
                            connect_ms,
                            tls_ms: 0,
                        },
                    ))
                }
                Some("https") => {
                    let server_name = resolve_server_name(&dst)?;
                    let tcp = connecting.await.map_err(|err| Box::new(err) as BoxError)?;
                    let connect_ms = connect_start.elapsed().as_millis() as u64;

                    let tls_start = std::time::Instant::now();
                    let tls_stream = TlsConnector::from(tls_config)
                        .connect(server_name, tcp.into_inner())
                        .await
                        .map_err(io::Error::other)?;
                    let tls_ms = tls_start.elapsed().as_millis() as u64;

                    Ok(TimedConn::new(
                        MaybeHttpsStream::Https(TokioIo::new(tls_stream)),
                        ConnectTiming { connect_ms, tls_ms },
                    ))
                }
                Some(other) => Err(io::Error::other(format!("unsupported scheme {other}")).into()),
                None => Err(io::Error::other("missing scheme").into()),
            }
        })
    }
}

async fn connect_via_proxy(
    dst: Uri,
    scheme: Option<String>,
    tls_config: Arc<ClientConfig>,
    proxy: UpstreamProxyConfig,
    validated_target: ValidatedUpstreamTarget,
    options: ProxyConnectOptions,
    connect_start: std::time::Instant,
) -> Result<TimedConn, BoxError> {
    let scheme = scheme.ok_or_else(|| io::Error::other("missing scheme"))?;
    let tcp = match &validated_target.resolution {
        UpstreamTargetResolution::ProxyDns => {
            if !proxy.supports_remote_target_dns() {
                return Err(
                    io::Error::other("upstream proxy does not support remote target DNS").into(),
                );
            }
            connect_target_via_proxy(
                &proxy,
                &validated_target.host,
                validated_target.port,
                options,
            )
            .await?
        }
        UpstreamTargetResolution::Pinned(addrs) => {
            let mut last_error = None;
            let mut connected = None;
            for target_addr in addrs.iter().copied() {
                match connect_validated_target_via_proxy(&proxy, target_addr, options).await {
                    Ok(tcp) => {
                        connected = Some(tcp);
                        break;
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            connected.ok_or_else(|| {
                last_error.unwrap_or_else(|| {
                    io::Error::other("validated upstream target has no addresses")
                })
            })?
        }
    };

    let connect_ms = connect_start.elapsed().as_millis() as u64;

    match scheme.as_str() {
        "http" => Ok(TimedConn::new(
            MaybeHttpsStream::Http {
                stream: TokioIo::new(tcp),
                is_proxy: false,
            },
            ConnectTiming {
                connect_ms,
                tls_ms: 0,
            },
        )),
        "https" => {
            let tls_start = std::time::Instant::now();
            let tls_stream = TlsConnector::from(tls_config)
                .connect(resolve_server_name(&dst)?, tcp)
                .await
                .map_err(io::Error::other)?;
            let tls_ms = tls_start.elapsed().as_millis() as u64;
            Ok(TimedConn::new(
                MaybeHttpsStream::Https(TokioIo::new(tls_stream)),
                ConnectTiming { connect_ms, tls_ms },
            ))
        }
        other => Err(io::Error::other(format!("unsupported scheme {other}")).into()),
    }
}

fn uri_host(uri: &Uri) -> Result<String, io::Error> {
    uri.host()
        .map(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .to_string()
        })
        .filter(|host| !host.is_empty())
        .ok_or_else(|| io::Error::other("missing host"))
}

fn uri_port_or_default(uri: &Uri, scheme: &str) -> Result<u16, io::Error> {
    uri.port_u16()
        .or(match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        })
        .ok_or_else(|| io::Error::other(format!("missing port for scheme {scheme}")))
}

fn build_upstream_client_with_protocol(
    config: &Config,
    validated_target: ValidatedUpstreamTarget,
    http1_only: bool,
    h2c_prior_knowledge: bool,
) -> Result<UpstreamClient, String> {
    let proxy = config
        .upstream_proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(UpstreamProxyConfig::parse)
        .transpose()?;
    if validated_target.uses_proxy_dns()
        && (!config.upstream_proxy_remote_dns
            || !proxy
                .as_ref()
                .is_some_and(UpstreamProxyConfig::supports_remote_target_dns))
    {
        return Err(
            "proxy-resolved upstream requires explicit remote DNS and an HTTP or SOCKS5h proxy"
                .to_string(),
        );
    }
    let mut http = HttpConnector::new_with_resolver(PinnedResolver::new(validated_target.clone()));
    http.enforce_http(false);
    http.set_connect_timeout(Some(Duration::from_secs(
        config.upstream_connect_timeout_secs,
    )));
    http.set_nodelay(config.upstream_tcp_nodelay);
    if config.upstream_tcp_keepalive_secs > 0 {
        http.set_keepalive(Some(Duration::from_secs(
            config.upstream_tcp_keepalive_secs,
        )));
    } else {
        http.set_keepalive(None);
    }

    let connector = InstrumentedConnector {
        http,
        tls_config: build_tls_config(http1_only),
        validated_target,
        proxy,
        connect_timeout: Duration::from_secs(config.upstream_connect_timeout_secs),
        tcp_nodelay: config.upstream_tcp_nodelay,
        tcp_keepalive: (config.upstream_tcp_keepalive_secs > 0)
            .then(|| Duration::from_secs(config.upstream_tcp_keepalive_secs)),
    };

    let mut builder = Client::builder(TokioExecutor::new());
    if h2c_prior_knowledge {
        builder.http2_only(true);
    }
    builder.pool_max_idle_per_host(config.upstream_pool_max_idle_per_host);
    builder.pool_idle_timeout(Duration::from_secs(config.upstream_pool_idle_timeout_secs));
    builder.pool_timer(TokioTimer::new());
    Ok(builder.build(connector))
}

pub fn resolve_request_timing<B>(
    response: &Response<B>,
    connection_acquire_ms: Option<u64>,
    ttfb_ms: u64,
) -> RequestTiming {
    let raw = response
        .extensions()
        .get::<ConnectTiming>()
        .copied()
        .unwrap_or_default();

    let raw_connection_ms = raw.connect_ms.saturating_add(raw.tls_ms);
    let measured_acquire_ms = connection_acquire_ms.unwrap_or(raw_connection_ms.min(ttfb_ms));
    let likely_reused = measured_acquire_ms <= 5 && raw_connection_ms > 0;
    let connector_matches_request = raw_connection_ms <= measured_acquire_ms.saturating_add(25);

    let (connect_ms, tls_ms) = if likely_reused || !connector_matches_request {
        (0, 0)
    } else {
        (raw.connect_ms, raw.tls_ms)
    };

    RequestTiming {
        connection_acquire_ms: measured_acquire_ms,
        connect_ms,
        tls_ms,
        response_wait_ms: ttfb_ms.saturating_sub(measured_acquire_ms),
        connection_reused: likely_reused,
    }
}

fn build_tls_config(http1_only: bool) -> Arc<ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    config.alpn_protocols = if http1_only {
        vec![b"http/1.1".to_vec()]
    } else {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    };
    Arc::new(config)
}

fn resolve_server_name(uri: &Uri) -> Result<ServerName<'static>, BoxError> {
    let host = uri.host().ok_or_else(|| io::Error::other("missing host"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ServerName::from(ip));
    }

    Ok(ServerName::try_from(host.to_string())?)
}

pub struct TimedConn {
    inner: MaybeHttpsStream,
    timing: ConnectTiming,
}

impl TimedConn {
    fn new(inner: MaybeHttpsStream, timing: ConnectTiming) -> Self {
        Self { inner, timing }
    }
}

impl Connection for TimedConn {
    fn connected(&self) -> Connected {
        self.inner.connected().extra(self.timing)
    }
}

impl rt::Read for TimedConn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl rt::Write for TimedConn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

pub enum MaybeHttpsStream {
    Http { stream: PlainStream, is_proxy: bool },
    Https(TlsStream),
}

impl Connection for MaybeHttpsStream {
    fn connected(&self) -> Connected {
        match self {
            Self::Http { stream, is_proxy } => stream.connected().proxy(*is_proxy),
            Self::Https(stream) => {
                let (tcp, tls) = stream.inner().get_ref();
                if tls.alpn_protocol() == Some(b"h2") {
                    tcp.connected().negotiated_h2()
                } else {
                    tcp.connected()
                }
            }
        }
    }
}

impl rt::Read for MaybeHttpsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http { stream, .. } => Pin::new(stream).poll_read(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl rt::Write for MaybeHttpsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        match Pin::get_mut(self) {
            Self::Http { stream, .. } => Pin::new(stream).poll_write(cx, buf),
            Self::Https(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http { stream, .. } => Pin::new(stream).poll_flush(cx),
            Self::Https(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        match Pin::get_mut(self) {
            Self::Http { stream, .. } => Pin::new(stream).poll_shutdown(cx),
            Self::Https(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Http { stream, .. } => stream.is_write_vectored(),
            Self::Https(stream) => stream.is_write_vectored(),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        match Pin::get_mut(self) {
            Self::Http { stream, .. } => Pin::new(stream).poll_write_vectored(cx, bufs),
            Self::Https(stream) => Pin::new(stream).poll_write_vectored(cx, bufs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::ResolvedTransportProfile;
    use clap::Parser;
    use http_body_util::BodyExt;
    use hyper::Response;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::egress_proxy::socks5_target_address;

    #[test]
    fn fresh_connection_uses_connector_breakdown() {
        let mut response = Response::new(());
        response.extensions_mut().insert(ConnectTiming {
            connect_ms: 80,
            tls_ms: 40,
        });

        let timing = resolve_request_timing(&response, Some(125), 600);

        assert_eq!(timing.connection_acquire_ms, 125);
        assert_eq!(timing.connect_ms, 80);
        assert_eq!(timing.tls_ms, 40);
        assert_eq!(timing.response_wait_ms, 475);
        assert!(!timing.connection_reused);
    }

    #[test]
    fn reused_connection_zeroes_stale_connect_timings() {
        let mut response = Response::new(());
        response.extensions_mut().insert(ConnectTiming {
            connect_ms: 70,
            tls_ms: 30,
        });

        let timing = resolve_request_timing(&response, Some(0), 310);

        assert_eq!(timing.connection_acquire_ms, 0);
        assert_eq!(timing.connect_ms, 0);
        assert_eq!(timing.tls_ms, 0);
        assert_eq!(timing.response_wait_ms, 310);
        assert!(timing.connection_reused);
    }

    #[test]
    fn falls_back_to_connector_timings_when_capture_missing() {
        let mut response = Response::new(());
        response.extensions_mut().insert(ConnectTiming {
            connect_ms: 55,
            tls_ms: 25,
        });

        let timing = resolve_request_timing(&response, None, 400);

        assert_eq!(timing.connection_acquire_ms, 80);
        assert_eq!(timing.connect_ms, 55);
        assert_eq!(timing.tls_ms, 25);
        assert_eq!(timing.response_wait_ms, 320);
        assert!(!timing.connection_reused);
    }

    #[test]
    fn upstream_client_pool_key_includes_profile_identity() {
        let profile = ResolvedTransportProfile {
            profile_id: "profile-a".to_string(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string(),
            http_mode: "auto".to_string(),
            pool_scope: "key".to_string(),
            header_fingerprint: None,
            extra: None,
        };
        let target_url = url::Url::parse("https://example.com/").expect("target URL");
        let validated_target = ValidatedUpstreamTarget::new(
            &target_url,
            vec![SocketAddr::from(([203, 0, 113, 10], 443))],
        )
        .expect("validated target");
        let pool_key = upstream_client_pool_key(
            Some("provider-1"),
            Some("endpoint-1"),
            Some("key-1"),
            Some(&profile),
            false,
            validated_target,
        );

        assert_eq!(pool_key.provider_id, "provider-1");
        assert_eq!(pool_key.endpoint_id, "endpoint-1");
        assert_eq!(pool_key.key_id, "key-1");
        assert_eq!(pool_key.profile_id, "profile-a");
        assert_eq!(pool_key.backend, TRANSPORT_BACKEND_REQWEST_RUSTLS);
        assert_eq!(pool_key.http_mode, "auto");
    }

    #[test]
    fn upstream_client_pool_rejects_unsupported_backend() {
        let error = validate_proxy_transport_backend("utls").unwrap_err();

        assert!(error.contains("unsupported transport profile backend"));
    }

    #[test]
    fn upstream_client_pool_evicts_lru_clients_above_capacity() {
        let config = Arc::new(
            Config::try_parse_from([
                "aether-tunnel",
                "--aether-url",
                "https://aether.example.com",
                "--management-token",
                "ae_test",
                "--node-name",
                "tunnel-test",
                "--upstream-client-pool-capacity",
                "2",
            ])
            .expect("config should parse"),
        );
        let pool =
            UpstreamClientPool::new(config, Arc::new(DnsCache::new(Duration::from_secs(60), 16)));
        let key_a = test_pool_key("key-a");
        let key_b = test_pool_key("key-b");
        let key_c = test_pool_key("key-c");

        pool.get_or_build(key_a.clone())
            .expect("client A should build");
        pool.get_or_build(key_b.clone())
            .expect("client B should build");
        pool.get_or_build(key_a.clone())
            .expect("client A should be reused and become most recent");
        pool.get_or_build(key_c.clone())
            .expect("client C should build");

        let clients = pool.clients.lock().expect("client pool lock");
        assert_eq!(clients.len(), 2);
        assert!(clients.contains_key(&key_a));
        assert!(clients.contains_key(&key_c));
        assert!(!clients.contains_key(&key_b));
    }

    fn test_pool_key(key_id: &str) -> UpstreamClientPoolKey {
        let target_url = url::Url::parse("https://example.com/").expect("target URL");
        let validated_target = ValidatedUpstreamTarget::new(
            &target_url,
            vec![SocketAddr::from(([203, 0, 113, 10], 443))],
        )
        .expect("validated target");
        upstream_client_pool_key(
            Some("provider-1"),
            Some("endpoint-1"),
            Some(key_id),
            None,
            false,
            validated_target,
        )
    }

    #[tokio::test]
    async fn socks5h_target_address_uses_domain_name() {
        let request = socks5_target_address("example.com", 443, true)
            .await
            .expect("SOCKS target should build");

        assert_eq!(
            request,
            [
                &[0x05, 0x01, 0x00, 0x03, 11][..],
                b"example.com",
                &[0x01, 0xbb][..],
            ]
            .concat()
        );
    }

    #[tokio::test]
    async fn http_proxy_connects_to_pinned_ip_and_preserves_origin_host() {
        let pinned_addr = SocketAddr::from(([203, 0, 113, 77], 80));
        let (proxy_url, connect_rx, request_rx) = spawn_http_proxy().await;
        let client = proxied_client(&proxy_url, "http://example.com/", pinned_addr);
        let request = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri("http://example.com/tunnel-test")
            .body(full_request_body(Bytes::new()))
            .expect("request should build");

        let response = client.request(request).await.expect("request should pass");
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body should collect")
            .to_bytes();
        let connect = connect_rx.await.expect("proxy should receive CONNECT");
        let raw_request = request_rx.await.expect("proxy should receive request");

        assert_eq!(status, hyper::StatusCode::OK);
        assert_eq!(&body[..], b"ok");
        assert!(
            connect.starts_with("CONNECT 203.0.113.77:80 HTTP/1.1\r\n"),
            "unexpected proxy CONNECT: {connect:?}"
        );
        assert!(
            raw_request.starts_with("GET /tunnel-test HTTP/1.1\r\n"),
            "unexpected proxy request: {raw_request:?}"
        );
        assert!(
            raw_request
                .to_ascii_lowercase()
                .contains("\r\nhost: example.com\r\n"),
            "original Host header should be preserved: {raw_request:?}"
        );
    }

    #[tokio::test]
    async fn https_proxy_connects_to_pinned_ip_while_sni_uses_hostname() {
        let pinned_addr = SocketAddr::from(([203, 0, 113, 78], 443));
        let (proxy_url, connect_rx) = spawn_connect_only_http_proxy().await;
        let client = proxied_client(&proxy_url, "https://sni.example/", pinned_addr);
        let request = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri("https://sni.example/secure")
            .body(full_request_body(Bytes::new()))
            .expect("request should build");

        let _ = client.request(request).await;
        let connect = connect_rx.await.expect("proxy should receive CONNECT");
        assert!(
            connect.starts_with("CONNECT 203.0.113.78:443 HTTP/1.1\r\n"),
            "unexpected proxy CONNECT: {connect:?}"
        );
        let uri: Uri = "https://sni.example/secure".parse().expect("URI");
        match resolve_server_name(&uri).expect("server name") {
            ServerName::DnsName(name) => assert_eq!(name.as_ref(), "sni.example"),
            other => panic!("expected DNS SNI, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn socks5_proxy_connects_to_pinned_ip_and_preserves_origin_host() {
        assert_socks_proxy_uses_pinned_ip("socks5").await;
    }

    #[tokio::test]
    async fn socks5h_proxy_connects_to_pinned_ip_and_preserves_origin_host() {
        assert_socks_proxy_uses_pinned_ip("socks5h").await;
    }

    async fn assert_socks_proxy_uses_pinned_ip(scheme: &str) {
        let pinned_addr = SocketAddr::from(([203, 0, 113, 79], 80));
        let (proxy_addr, target_rx, request_rx) = spawn_socks5_proxy().await;
        let proxy_url = format!("{scheme}://{proxy_addr}");
        let client = proxied_client(&proxy_url, "http://example.com/", pinned_addr);
        let request = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri("http://example.com/socks-test")
            .body(full_request_body(Bytes::new()))
            .expect("request should build");

        let response = client.request(request).await.expect("request should pass");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body should collect")
            .to_bytes();
        let target = target_rx.await.expect("SOCKS proxy should receive target");
        let raw_request = request_rx
            .await
            .expect("SOCKS proxy should receive HTTP request");

        assert_eq!(&body[..], b"ok");
        assert_eq!(target, pinned_addr);
        assert!(
            raw_request.starts_with("GET /socks-test HTTP/1.1\r\n"),
            "unexpected SOCKS tunneled request: {raw_request:?}"
        );
        assert!(
            raw_request
                .to_ascii_lowercase()
                .contains("\r\nhost: example.com\r\n"),
            "original Host header should be preserved: {raw_request:?}"
        );
    }

    #[tokio::test]
    async fn trusted_http_proxy_resolves_hostname_without_local_dns() {
        let (proxy_url, connect_rx, request_rx) = spawn_http_proxy().await;
        let client = remote_dns_client(&proxy_url, "http://remote-dns-test.invalid/");
        let request = hyper::Request::builder()
            .uri("http://remote-dns-test.invalid/remote-dns")
            .body(full_request_body(Bytes::new()))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
            .await
            .unwrap()
            .expect("proxy should resolve the target without local DNS");
        assert_eq!(response.status(), hyper::StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "ok"
        );
        assert!(connect_rx
            .await
            .unwrap()
            .starts_with("CONNECT remote-dns-test.invalid:80 HTTP/1.1\r\n"));
        let request = request_rx.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /remote-dns http/1.1\r\n"));
        assert!(request.contains("\r\nhost: remote-dns-test.invalid\r\n"));
    }

    #[tokio::test]
    async fn trusted_socks5h_proxy_receives_hostname_not_a_locally_resolved_ip() {
        let (proxy_url, target_rx, request_rx) = spawn_remote_dns_socks_proxy().await;
        let client = remote_dns_client(&proxy_url, "http://remote-dns-test.invalid/");
        let request = hyper::Request::builder()
            .uri("http://remote-dns-test.invalid/remote-dns")
            .body(full_request_body(Bytes::new()))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
            .await
            .unwrap()
            .expect("SOCKS proxy should receive the unresolved target");
        assert_eq!(response.status(), hyper::StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "ok"
        );
        assert_eq!(
            target_rx.await.unwrap(),
            ("remote-dns-test.invalid".to_string(), 80)
        );
        assert!(request_rx
            .await
            .unwrap()
            .to_ascii_lowercase()
            .contains("\r\nhost: remote-dns-test.invalid\r\n"));
    }

    #[tokio::test]
    async fn remote_dns_https_preserves_hostname_for_connect_and_sni() {
        let (proxy_url, connect_rx) = spawn_connect_only_http_proxy().await;
        let client = remote_dns_client(&proxy_url, "https://remote-dns-test.invalid/");
        let uri: Uri = "https://remote-dns-test.invalid/secure".parse().unwrap();
        let request = hyper::Request::builder()
            .uri(uri.clone())
            .body(full_request_body(Bytes::new()))
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), client.request(request))
            .await
            .unwrap();
        assert!(connect_rx
            .await
            .unwrap()
            .starts_with("CONNECT remote-dns-test.invalid:443 HTTP/1.1\r\n"));
        match resolve_server_name(&uri).unwrap() {
            ServerName::DnsName(name) => assert_eq!(name.as_ref(), "remote-dns-test.invalid"),
            other => panic!("expected hostname for TLS verification, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn remote_dns_targets_cannot_fall_back_to_local_dns_or_change_origin() {
        let url = url::Url::parse("https://remote-dns-test.invalid/").unwrap();
        let target = ValidatedUpstreamTarget::proxy_resolved(&url).unwrap();
        let mut resolver = PinnedResolver::new(target.clone());
        let error = resolver
            .call("remote-dns-test.invalid".parse().unwrap())
            .await
            .err()
            .unwrap();
        assert!(error
            .to_string()
            .contains("must not fall back to local DNS"));
        for uri in [
            "http://remote-dns-test.invalid/",
            "https://another-target.invalid/",
            "https://remote-dns-test.invalid:8443/",
        ] {
            assert!(target.ensure_matches_uri(&uri.parse().unwrap()).is_err());
        }
        for url in ["http://127.0.0.1/", "https://[::1]/"] {
            assert!(
                ValidatedUpstreamTarget::proxy_resolved(&url::Url::parse(url).unwrap()).is_err()
            );
        }
    }

    #[test]
    fn remote_dns_clients_require_opt_in_and_do_not_share_pinned_pool_entries() {
        let mut config = remote_dns_config("http://127.0.0.1:8080");
        let url = url::Url::parse("https://remote-dns-test.invalid/").unwrap();
        let remote = ValidatedUpstreamTarget::proxy_resolved(&url).unwrap();
        config.upstream_proxy_remote_dns = false;
        assert!(build_upstream_client_with_protocol(&config, remote.clone(), true, false).is_err());
        config.upstream_proxy_remote_dns = true;
        for proxy in [None, Some("socks5://127.0.0.1:1080")] {
            config.upstream_proxy_url = proxy.map(str::to_string);
            assert!(
                build_upstream_client_with_protocol(&config, remote.clone(), true, false).is_err()
            );
        }
        config.upstream_proxy_url = Some("http://127.0.0.1:8080".to_string());
        let pinned =
            ValidatedUpstreamTarget::new(&url, vec!["8.8.8.8:443".parse().unwrap()]).unwrap();
        let remote_key = upstream_client_pool_key(None, None, None, None, false, remote);
        let pinned_key = upstream_client_pool_key(None, None, None, None, false, pinned);
        assert_ne!(remote_key, pinned_key);
        let pool = UpstreamClientPool::new(
            Arc::new(config),
            Arc::new(DnsCache::new(Duration::from_secs(60), 16)),
        );
        pool.get_or_build(remote_key).unwrap();
        pool.get_or_build(pinned_key).unwrap();
        assert_eq!(pool.clients.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn remote_dns_proxy_connect_timeout_covers_connect_and_tls_handshakes() {
        for tls in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_http_headers(&mut stream).await;
                if tls {
                    stream
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await
                        .unwrap();
                }
                std::future::pending::<()>().await;
                drop(stream);
            });
            let target_url = if tls {
                "https://remote-dns-test.invalid/"
            } else {
                "http://remote-dns-test.invalid/"
            };
            let client = remote_dns_client(&proxy_url, target_url);
            let request = hyper::Request::builder()
                .uri(target_url)
                .body(full_request_body(Bytes::new()))
                .unwrap();
            let result =
                tokio::time::timeout(Duration::from_secs(5), client.request(request)).await;
            server.abort();
            let error = result
                .expect("configured connect timeout must include proxy and TLS handshakes")
                .unwrap_err();
            assert!(error.is_connect());
        }
    }

    fn remote_dns_config(proxy_url: &str) -> Config {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--upstream-proxy-url",
            proxy_url,
            "--upstream-proxy-remote-dns",
            "--upstream-connect-timeout-secs",
            "1",
        ])
    }

    fn remote_dns_client(proxy_url: &str, target_url: &str) -> UpstreamClient {
        let config = remote_dns_config(proxy_url);
        let target =
            ValidatedUpstreamTarget::proxy_resolved(&url::Url::parse(target_url).unwrap()).unwrap();
        build_upstream_client_with_protocol(&config, target, true, false).unwrap()
    }

    async fn spawn_remote_dns_socks_proxy() -> (
        String,
        tokio::sync::oneshot::Receiver<(String, u16)>,
        tokio::sync::oneshot::Receiver<String>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_url = format!("socks5h://{}", listener.local_addr().unwrap());
        let (target_tx, target_rx) = tokio::sync::oneshot::channel();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0u8; 3];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [0x05, 0x01, 0x00]);
            stream.write_all(&[0x05, 0x00]).await.unwrap();
            let mut header = [0u8; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..4], &[0x05, 0x01, 0x00, 0x03]);
            let mut hostname = vec![0; header[4] as usize];
            stream.read_exact(&mut hostname).await.unwrap();
            let port = stream.read_u16().await.unwrap();
            target_tx
                .send((String::from_utf8(hostname).unwrap(), port))
                .unwrap();
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            request_tx
                .send(read_http_headers(&mut stream).await)
                .unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        (proxy_url, target_rx, request_rx)
    }

    fn proxied_client(
        proxy_url: &str,
        target_url: &str,
        pinned_addr: SocketAddr,
    ) -> UpstreamClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = Config::try_parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://aether.example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--upstream-proxy-url",
            proxy_url,
            "--upstream-connect-timeout-secs",
            "2",
        ])
        .expect("config should parse");
        let target_url = url::Url::parse(target_url).expect("target URL should parse");
        let validated_target = ValidatedUpstreamTarget::new(&target_url, vec![pinned_addr])
            .expect("target should validate");
        build_upstream_client_with_protocol(&config, validated_target, true, false)
            .expect("client should build")
    }

    async fn spawn_http_proxy() -> (
        String,
        tokio::sync::oneshot::Receiver<String>,
        tokio::sync::oneshot::Receiver<String>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should exist");
        let (connect_tx, connect_rx) = tokio::sync::oneshot::channel();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("proxy should accept");
            let connect = read_http_headers(&mut stream).await;
            let _ = connect_tx.send(connect);
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .expect("CONNECT response should write");
            let request = read_http_headers(&mut stream).await;
            let _ = request_tx.send(request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .expect("proxy response should write");
        });
        (format!("http://{addr}"), connect_rx, request_rx)
    }

    async fn spawn_connect_only_http_proxy() -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should exist");
        let (connect_tx, connect_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("proxy should accept");
            let connect = read_http_headers(&mut stream).await;
            let _ = connect_tx.send(connect);
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .expect("CONNECT response should write");
        });
        (format!("http://{addr}"), connect_rx)
    }

    async fn spawn_socks5_proxy() -> (
        SocketAddr,
        tokio::sync::oneshot::Receiver<SocketAddr>,
        tokio::sync::oneshot::Receiver<String>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should exist");
        let (target_tx, target_rx) = tokio::sync::oneshot::channel();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("SOCKS proxy should accept");
            let mut greeting = [0u8; 3];
            stream
                .read_exact(&mut greeting)
                .await
                .expect("SOCKS greeting should read");
            assert_eq!(greeting, [0x05, 0x01, 0x00]);
            stream
                .write_all(&[0x05, 0x00])
                .await
                .expect("SOCKS method should write");

            let mut request_head = [0u8; 4];
            stream
                .read_exact(&mut request_head)
                .await
                .expect("SOCKS request head should read");
            assert_eq!(request_head, [0x05, 0x01, 0x00, 0x01]);
            let mut ip = [0u8; 4];
            stream
                .read_exact(&mut ip)
                .await
                .expect("SOCKS IPv4 target should read");
            let mut port = [0u8; 2];
            stream
                .read_exact(&mut port)
                .await
                .expect("SOCKS port should read");
            let port = u16::from_be_bytes(port);
            let target = SocketAddr::from((ip, port));
            let _ = target_tx.send(target);
            stream
                .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .expect("SOCKS connect response should write");

            let request = read_http_headers(&mut stream).await;
            let _ = request_tx.send(request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .expect("SOCKS tunneled response should write");
        });
        (addr, target_rx, request_rx)
    }

    async fn read_http_headers(stream: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.expect("request should read");
            assert!(n > 0, "connection closed before headers finished");
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(buf).expect("headers should be UTF-8")
    }
}
