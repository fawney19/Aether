#![allow(clippy::large_enum_variant)]

mod app;
mod config;
mod hardware;
mod net;
mod registration;
mod runtime;
mod setup;
mod state;
mod target_filter;
mod tunnel;
mod upstream_client;

use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser};

use config::Config;

/// Default config file name.
const DEFAULT_CONFIG: &str = "aether-proxy.toml";

/// Build the full clap command: Config args + discoverable subcommands.
///
/// `subcommand_negates_reqs` lets subcommands bypass the required Config
/// flags so that e.g. `aether-proxy setup` doesn't demand `--aether-url`.
fn build_command() -> clap::Command {
    Config::command()
        .subcommand(
            clap::Command::new("setup")
                .about("Interactive setup wizard (TUI)")
                .arg(
                    clap::Arg::new("config_path")
                        .help("Path to config file")
                        .default_value(DEFAULT_CONFIG),
                ),
        )
        .subcommand(clap::Command::new("start").about("Start the installed service"))
        .subcommand(clap::Command::new("status").about("Show service status"))
        .subcommand(clap::Command::new("logs").about("Tail service logs"))
        .subcommand(clap::Command::new("restart").about("Restart the installed service"))
        .subcommand(clap::Command::new("stop").about("Stop the installed service"))
        .subcommand(clap::Command::new("uninstall").about("Uninstall the installed service"))
        .subcommand(
            clap::Command::new("upgrade")
                .about("Self-upgrade from GitHub releases")
                .arg(clap::Arg::new("version").help("Target version (e.g. 0.2.0)")),
        )
        .subcommand_negates_reqs(true)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Failed to install rustls CryptoProvider"))?;

    // Load config file as env-var defaults (before clap parsing)
    let config_file_path =
        std::env::var("AETHER_PROXY_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let config_path = std::path::Path::new(&config_file_path);
    if config_path.exists() {
        // Migrate legacy 0.1.x config to 0.2.0 format if needed
        if let Err(e) = config::ConfigFile::migrate_legacy(config_path) {
            eprintln!("  WARNING: config migration failed: {}", e);
        }
        if let Ok(file_cfg) = config::ConfigFile::load(config_path) {
            file_cfg.inject_env();
        }
    }

    // Parse CLI (subcommands + config args in one pass)
    match build_command().try_get_matches() {
        Ok(matches) => match matches.subcommand() {
            Some(("setup", sub_m)) => {
                let path = sub_m
                    .get_one::<String>("config_path")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
                handle_setup_result(setup::run(path)?).await
            }
            Some(("start", _)) => setup::service::cmd_start(),
            Some(("status", _)) => setup::service::cmd_status(),
            Some(("logs", _)) => setup::service::cmd_logs(),
            Some(("restart", _)) => setup::service::cmd_restart(),
            Some(("stop", _)) => setup::service::cmd_stop(),
            Some(("uninstall", _)) => setup::service::cmd_uninstall(),
            Some(("upgrade", sub_m)) => {
                let version = sub_m.get_one::<String>("version").cloned();
                setup::upgrade::cmd_upgrade(version).await
            }
            Some(_) => unreachable!(),
            None => {
                // No subcommand — run the proxy with parsed config.
                let config = Config::from_arg_matches(&matches)?;
                run_proxy(config).await
            }
        },
        Err(e) => {
            if e.kind() == clap::error::ErrorKind::MissingRequiredArgument {
                eprintln!("Missing required config, launching setup wizard...\n");
                handle_setup_result(setup::run(PathBuf::from(&config_file_path))?).await
            } else {
                e.exit();
            }
        }
    }
}

/// Decide what to do after the setup wizard completes.
async fn handle_setup_result(outcome: setup::SetupOutcome) -> anyhow::Result<()> {
    match outcome {
        setup::SetupOutcome::ServiceInstalled => Ok(()),
        setup::SetupOutcome::ReadyToRun(config_path) => {
            // Reload config from the file that setup just wrote, overriding
            // any stale env vars from a previous config.
            match config::ConfigFile::load(&config_path) {
                Ok(file_cfg) => file_cfg.inject_env_override(),
                Err(e) => anyhow::bail!("failed to reload config after setup: {}", e),
            }
            // Parse from env-only (argv may still contain "setup" etc.)
            let config = Config::try_parse_from(["aether-proxy"])
                .map_err(|e| anyhow::anyhow!("config invalid after setup: {}", e))?;
            eprintln!("  Starting proxy...\n");
            run_proxy(config).await
        }
        setup::SetupOutcome::Cancelled => {
            eprintln!("  Setup cancelled.");
            Ok(())
        }
    }
}

/// Start the proxy server, checking for managed-service conflicts first.
async fn run_proxy(config: Config) -> anyhow::Result<()> {
    // Warn if a managed service is already running (would cause conflicts).
    if std::env::var_os("AETHER_PROXY_SERVICE_MANAGER").is_none()
        && std::env::var_os("INVOCATION_ID").is_none()
        && setup::service::is_service_active()
    {
        eprintln!(
            "Warning: {} service is already running.",
            setup::service::preferred_manager_name()
        );
        eprintln!("Use `./aether-proxy stop` to stop it first, or manage via subcommands:");
        eprintln!("  ./aether-proxy status / logs / restart / stop");
        std::process::exit(1);
    }

    // Resolve server list: prefer [[servers]] from TOML, fall back to CLI/env single server.
    let config_path =
        std::env::var("AETHER_PROXY_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let servers = if std::path::Path::new(&config_path).exists() {
        config::ConfigFile::load(std::path::Path::new(&config_path))
            .ok()
            .map(|f| f.effective_servers())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                vec![config::ServerEntry {
                    aether_url: config.aether_url.clone(),
                    management_token: config.management_token.clone(),
                    node_name: None,
                }]
            })
    } else {
        vec![config::ServerEntry {
            aether_url: config.aether_url.clone(),
            management_token: config.management_token.clone(),
            node_name: None,
        }]
    };

    app::run(config, servers).await
}
