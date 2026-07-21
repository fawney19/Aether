use std::fmt;
use std::net::SocketAddr;

use axum::Router;

pub struct SpawnedServer {
    base_url: String,
    port: u16,
    handle: tokio::task::JoinHandle<()>,
}

impl SpawnedServer {
    pub async fn start(app: Router) -> Result<Self, std::io::Error> {
        let port = reserve_local_port()?;
        Self::start_on_port(port, app).await
    }

    pub async fn start_on_port(port: u16, app: Router) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("spawned server should run");
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            port: addr.port(),
            handle,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Debug for SpawnedServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnedServer")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub fn reserve_local_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
