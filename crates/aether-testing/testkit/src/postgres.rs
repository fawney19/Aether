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
        std::fs::create_dir_all(&workdir)?;

        let initdb_bin = std::env::var("AETHER_INITDB_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "initdb".to_string());
        let postgres_bin = std::env::var("AETHER_POSTGRES_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "postgres".to_string());

        let init_output = Command::new(&initdb_bin)
            .arg("-D")
            .arg(&data_dir)
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

        let database_url = format!("postgres://aether@127.0.0.1:{port}/postgres");
        let mut server = Self {
            child: None,
            postgres_bin,
            port,
            workdir,
            data_dir,
            database_url,
        };
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
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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
        let _ = self.stop();
        let _ = std::fs::remove_dir_all(&self.workdir);
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
