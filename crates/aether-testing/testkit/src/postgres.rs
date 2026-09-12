use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use aether_data::driver::postgres::PostgresPoolConfig;
use aether_data::{DataBackends, DataLayerConfig};
use sqlx::{Connection, PgConnection};

use crate::wait_until;

#[derive(Debug)]
pub struct ManagedPostgresServer {
    child: Option<Child>,
    postgres_bin: String,
    pg_ctl_bin: PathBuf,
    port: u16,
    workdir: PathBuf,
    data_dir: PathBuf,
    database_url: String,
}

impl ManagedPostgresServer {
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = reserve_local_port()?;
        let workdir = std::env::temp_dir().join(format!(
            "aether-postgres-baseline-{}-{}",
            std::process::id(),
            port
        ));
        let data_dir = workdir.join("data");
        std::fs::create_dir(&workdir)?;

        let initdb_bin = std::env::var("AETHER_INITDB_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "initdb".to_string());
        let postgres_bin = std::env::var("AETHER_POSTGRES_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "postgres".to_string());
        let pg_ctl_bin = std::env::var("AETHER_PG_CTL_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(&postgres_bin).with_file_name(if cfg!(windows) {
                    "pg_ctl.exe"
                } else {
                    "pg_ctl"
                })
            });
        let database_url = format!("postgres://aether@127.0.0.1:{port}/postgres");
        let mut server = Self {
            child: None,
            postgres_bin,
            pg_ctl_bin,
            port,
            workdir,
            data_dir,
            database_url,
        };

        let init_output = Command::new(&initdb_bin)
            .arg("-D")
            .arg(&server.data_dir)
            .arg("-U")
            .arg("aether")
            .arg("--auth=trust")
            .arg("--encoding=UTF8")
            .arg("--no-instructions")
            .output()?;
        if !init_output.status.success() {
            return Err(std::io::Error::other(format!(
                "initdb failed: {}",
                String::from_utf8_lossy(&init_output.stderr)
            ))
            .into());
        }

        server.restart().await?;
        Ok(server)
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stop(&mut self) -> Result<(), std::io::Error> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_some() {
            self.child = None;
            return Ok(());
        }

        let output = Command::new(&self.pg_ctl_bin)
            .arg("-D")
            .arg(&self.data_dir)
            .args(["stop", "-m", "fast", "-w", "-t", "10"])
            .output()?;
        if !output.status.success() && child.try_wait()?.is_none() {
            return Err(std::io::Error::other(format!(
                "pg_ctl stop failed for {}: {}{}",
                self.data_dir.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )));
        }
        child.wait()?;
        self.child = None;
        Ok(())
    }

    pub async fn restart(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop()?;
        let log_path = self.workdir.join("postgres.log");
        let stdout = std::fs::File::create(&log_path)?;
        let stderr = stdout.try_clone()?;
        let child = Command::new(&self.postgres_bin)
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-h")
            .arg("127.0.0.1")
            .arg("-p")
            .arg(self.port.to_string())
            .arg("-F")
            .arg("-c")
            .arg("unix_socket_directories=")
            .arg("-c")
            .arg("fsync=off")
            .arg("-c")
            .arg("synchronous_commit=off")
            .arg("-c")
            .arg("full_page_writes=off")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;
        self.child = Some(child);

        let database_url = self.database_url.clone();
        let ready = wait_until(
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(50),
            || {
                let database_url = database_url.clone();
                async move {
                    match PgConnection::connect(&database_url).await {
                        Ok(connection) => connection.close().await.is_ok(),
                        Err(_) => false,
                    }
                }
            },
        )
        .await;
        if !ready {
            self.stop()?;
            let logs = std::fs::read_to_string(&log_path).unwrap_or_default();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out waiting for local postgres; logs:\n{logs}"),
            )
            .into());
        }
        Ok(())
    }
}

impl Drop for ManagedPostgresServer {
    fn drop(&mut self) {
        match self.stop() {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&self.workdir);
            }
            Err(error) => {
                eprintln!(
                    "failed to stop managed postgres; preserving {}: {error}",
                    self.workdir.display(),
                );
            }
        }
    }
}

pub async fn prepare_aether_postgres_schema(
    database_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = PostgresPoolConfig {
        database_url: database_url.to_string(),
        ..Default::default()
    };

    let backends = DataBackends::from_config(DataLayerConfig::from_postgres(config))?;
    let pending_migrations = backends
        .prepare_database_for_startup()
        .await?
        .unwrap_or_default();
    if !pending_migrations.is_empty() {
        backends.run_database_migrations().await?;
    }

    Ok(())
}

fn reserve_local_port() -> Result<u16, std::io::Error> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires local initdb, postgres, and pg_ctl binaries"]
    async fn live_managed_postgres_restarts_cleanly_with_open_connections() {
        let mut server = ManagedPostgresServer::start().await.unwrap();
        let workdir = server.workdir.clone();
        let mut connection = PgConnection::connect(server.database_url()).await.unwrap();
        sqlx::query("CREATE TABLE restart_probe (value INTEGER NOT NULL)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO restart_probe VALUES (42)")
            .execute(&mut connection)
            .await
            .unwrap();

        for _iteration in 0..4 {
            server.stop().unwrap();
            server.stop().unwrap();
            assert!(server.child.is_none());
            assert!(!server.data_dir.join("postmaster.pid").exists());
            assert!(server.data_dir.exists());
            server.restart().await.unwrap();
            connection = PgConnection::connect(server.database_url()).await.unwrap();
            let value: i32 = sqlx::query_scalar("SELECT value FROM restart_probe")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(value, 42);
        }

        drop(server);
        assert!(!workdir.exists());
    }

    #[tokio::test]
    #[ignore = "requires local initdb, postgres, and pg_ctl binaries"]
    async fn live_failed_postgres_stop_can_be_retried_without_losing_ownership() {
        let mut server = ManagedPostgresServer::start().await.unwrap();
        let pg_ctl_bin = server.pg_ctl_bin.clone();
        server.pg_ctl_bin = server.workdir.join("missing-pg-ctl");
        assert!(server.stop().is_err());
        assert!(server.child.as_mut().unwrap().try_wait().unwrap().is_none());
        assert!(server.data_dir.exists());
        server.pg_ctl_bin = pg_ctl_bin;
        server.stop().unwrap();
        assert!(server.child.is_none());
        assert!(!server.data_dir.join("postmaster.pid").exists());
    }
}
