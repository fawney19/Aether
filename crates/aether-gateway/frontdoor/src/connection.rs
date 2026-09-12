use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

const MAX_HTTP_CONNECTIONS: usize = 65_536;
const FD_RESERVE: usize = 256;

/// Selects a per-server incoming TCP limit. The FD allowance leaves room for
/// upstream sockets and process infrastructure; it is not a complete FD budget.
pub fn http_connection_limit(
    configured: Option<usize>,
    request_limit: usize,
    websocket_limit: usize,
    fd_soft_limit: Option<usize>,
) -> usize {
    let configured = configured
        .filter(|limit| *limit > 0)
        .unwrap_or_else(|| request_limit.saturating_add(websocket_limit))
        .clamp(1, MAX_HTTP_CONNECTIONS);
    let fd_allowance = fd_soft_limit
        .map(|limit| (limit.saturating_sub(FD_RESERVE) / 2).max(1))
        .unwrap_or(MAX_HTTP_CONNECTIONS);
    configured.min(fd_allowance)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpConnectionBudgetSnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub high_watermark: usize,
    pub rejected_total: u64,
    pub accept_errors_total: u64,
}

/// Share one budget across listeners. Admission belongs to the underlying IO,
/// so HTTP/1 upgrades keep their permit and HTTP/2 streams share one permit.
#[derive(Debug)]
pub struct HttpConnectionBudget {
    limit: usize,
    permits: Arc<Semaphore>,
    in_flight: AtomicUsize,
    high_watermark: AtomicUsize,
    rejected_total: AtomicU64,
    accept_errors_total: AtomicU64,
    shutdown: CancellationToken,
}

impl HttpConnectionBudget {
    pub fn new(limit: usize) -> Self {
        let limit = limit.clamp(1, MAX_HTTP_CONNECTIONS.min(Semaphore::MAX_PERMITS));
        Self {
            limit,
            permits: Arc::new(Semaphore::new(limit)),
            in_flight: AtomicUsize::new(0),
            high_watermark: AtomicUsize::new(0),
            rejected_total: AtomicU64::new(0),
            accept_errors_total: AtomicU64::new(0),
            shutdown: CancellationToken::new(),
        }
    }

    /// Admit after accepting. Waiting for a permit before accept can let idle
    /// reuseport listeners monopolize permits needed by a busy listener.
    pub fn try_admit<T>(self: &Arc<Self>, io: T) -> Result<AdmittedConnection<T>, TryAcquireError> {
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .inspect_err(|_| {
                self.rejected_total.fetch_add(1, Ordering::Relaxed);
            })?;
        let in_flight = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        self.high_watermark.fetch_max(in_flight, Ordering::Relaxed);
        Ok(AdmittedConnection {
            io,
            read_shutdown: Box::pin(self.shutdown.clone().cancelled_owned()),
            write_shutdown: Box::pin(self.shutdown.clone().cancelled_owned()),
            _permit: ConnectionPermit {
                budget: Arc::clone(self),
                _permit: permit,
            },
        })
    }

    pub fn snapshot(&self) -> HttpConnectionBudgetSnapshot {
        HttpConnectionBudgetSnapshot {
            limit: self.limit,
            in_flight: self.in_flight.load(Ordering::Relaxed),
            high_watermark: self.high_watermark.load(Ordering::Relaxed),
            rejected_total: self.rejected_total.load(Ordering::Relaxed),
            accept_errors_total: self.accept_errors_total.load(Ordering::Relaxed),
        }
    }

    /// End the drain deadline for all sockets, including upgraded connections.
    pub fn force_close(&self) {
        self.permits.close();
        self.shutdown.cancel();
    }

    pub async fn wait_for_forced_close(&self) {
        self.shutdown.cancelled().await;
    }

    pub async fn accept(&self, listener: &TcpListener) -> (TcpStream, SocketAddr) {
        self.accept_with(|| listener.accept()).await
    }

    async fn accept_with<T, F, A>(&self, mut accept: A) -> T
    where
        A: FnMut() -> F,
        F: Future<Output = io::Result<T>>,
    {
        loop {
            match accept().await {
                Ok(connection) => return connection,
                Err(error) => {
                    self.accept_errors_total.fetch_add(1, Ordering::Relaxed);
                    // Match Axum's listener behavior: failed peers can be retried
                    // immediately; resource failures such as EMFILE need backoff.
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                    ) {
                        continue;
                    }
                    tracing::error!(
                        event_name = "http_connection_accept_failed",
                        error = %error,
                        "HTTP listener accept failed; retrying after one second"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

#[derive(Debug)]
struct ConnectionPermit {
    budget: Arc<HttpConnectionBudget>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.budget.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub struct AdmittedConnection<T> {
    // Close the socket before returning its permit, including upgrade teardown.
    io: T,
    read_shutdown: Pin<Box<WaitForCancellationFutureOwned>>,
    write_shutdown: Pin<Box<WaitForCancellationFutureOwned>>,
    _permit: ConnectionPermit,
}

impl<T: AsyncRead + Unpin> AsyncRead for AdmittedConnection<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.read_shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(shutdown_error()));
        }
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for AdmittedConnection<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(shutdown_error()));
        }
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(shutdown_error()));
        }
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.write_shutdown.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(shutdown_error()));
        }
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }
}

fn shutdown_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "gateway shutdown deadline reached",
    )
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
