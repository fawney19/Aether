use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::wait_until;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug)]
pub struct ManagedRedisServer {
    child: Option<Child>,
    binary: String,
    port: u16,
    workdir: PathBuf,
    redis_url: String,
}

impl ManagedRedisServer {
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = reserve_local_port()?;
        let workdir = std::env::temp_dir().join(format!(
            "aether-redis-baseline-{}-{}",
            std::process::id(),
            port
        ));
        std::fs::create_dir_all(&workdir)?;

        let binary = std::env::var("AETHER_REDIS_SERVER_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "redis-server".to_string());
        let redis_url = format!("redis://127.0.0.1:{port}/0");
        let mut server = Self {
            child: None,
            binary,
            port,
            workdir,
            redis_url,
        };
        server.restart().await?;
        Ok(server)
    }

    pub fn redis_url(&self) -> &str {
        &self.redis_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&mut self) -> Result<(), std::io::Error> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    pub async fn restart(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop()?;
        let child = Command::new(&self.binary)
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--dir")
            .arg(&self.workdir)
            .arg("--bind")
            .arg("127.0.0.1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.child = Some(child);

        let port = self.port;
        let ready = wait_until(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(50),
            || async move { redis_ping(("127.0.0.1", port)).await.unwrap_or(false) },
        )
        .await;
        if !ready {
            self.stop()?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for local redis-server",
            )
            .into());
        }
        Ok(())
    }
}

async fn redis_ping(addr: (&str, u16)) -> Result<bool, std::io::Error> {
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream.write_all(b"*1\r\n$4\r\nPING\r\n").await?;
    let mut buffer = [0_u8; 16];
    let len = stream.read(&mut buffer).await?;
    Ok(buffer[..len].starts_with(b"+PONG"))
}

impl Drop for ManagedRedisServer {
    fn drop(&mut self) {
        let _ = self.stop();
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

fn reserve_local_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
