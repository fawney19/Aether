#![allow(clippy::large_enum_variant)]

mod app;
mod config;
mod egress_proxy;
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

use clap::{parser::ValueSource, CommandFactory, FromArgMatches, Parser};

use config::{Config, ServerEntry, TunnelSecurity};

/// Default config file name.
const DEFAULT_CONFIG: &str = "aether-tunnel.toml";
const OUTBOUND_PROXY_ENV: &str = "AETHER_TUNNEL_AETHER_OUTBOUND_PROXY_URL";
const LEGACY_OUTBOUND_PROXY_ENV: &str = concat!("AETHER_TUNNEL_AETHER_", "PROXY_URL");

/// Build the full clap command: Config args + discoverable subcommands.
///
/// `subcommand_negates_reqs` lets subcommands bypass the required Config
/// flags so that e.g. `aether-tunnel setup` doesn't demand `--aether-url`.
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

    promote_legacy_env_overrides();

    // Load config file as env-var defaults (before clap parsing)
    let config_file_path =
        std::env::var("AETHER_TUNNEL_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let config_path = std::path::Path::new(&config_file_path);
    if config_path.exists() {
        match config::ConfigFile::load(config_path) {
            Ok(file_cfg) => file_cfg.inject_env(),
            Err(error) => {
                eprintln!(
                    "  WARNING: failed to load config {}: {}",
                    config_path.display(),
                    error
                );
            }
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
                // No subcommand: run the tunnel with parsed config.
                let config = Config::from_arg_matches(&matches)?;
                let tunnel_security = configured_tunnel_security_from_matches(&matches, &config);
                run_tunnel(config, tunnel_security).await
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

fn promote_legacy_env_overrides() {
    if std::env::var_os(OUTBOUND_PROXY_ENV).is_none() {
        if let Some(value) = std::env::var_os(LEGACY_OUTBOUND_PROXY_ENV) {
            std::env::set_var(OUTBOUND_PROXY_ENV, value);
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
            let config = Config::try_parse_from(["aether-tunnel"])
                .map_err(|e| anyhow::anyhow!("config invalid after setup: {}", e))?;
            eprintln!("  Starting tunnel...\n");
            run_tunnel(config, None).await
        }
        setup::SetupOutcome::Cancelled => {
            eprintln!("  Setup cancelled.");
            Ok(())
        }
    }
}

fn configured_tunnel_security_from_matches(
    matches: &clap::ArgMatches,
    config: &Config,
) -> Option<TunnelSecurity> {
    matches
        .value_source("tunnel_security")
        .filter(|source| *source != ValueSource::DefaultValue)
        .map(|_| config.tunnel_security)
}

fn single_server_entry(config: &Config, tunnel_security: Option<TunnelSecurity>) -> ServerEntry {
    ServerEntry {
        aether_url: config.aether_url.clone(),
        management_token: config.management_token.clone(),
        node_name: None,
        tunnel_security,
        tunnel_encryption_key: config.tunnel_encryption_key.clone(),
    }
}

/// Start the tunnel agent, checking for managed-service conflicts first.
async fn run_tunnel(
    config: Config,
    single_server_tunnel_security: Option<TunnelSecurity>,
) -> anyhow::Result<()> {
    // Warn if a managed service is already running (would cause conflicts).
    if std::env::var_os("AETHER_TUNNEL_SERVICE_MANAGER").is_none()
        && std::env::var_os("INVOCATION_ID").is_none()
        && setup::service::is_service_active()
    {
        eprintln!(
            "Warning: {} service is already running.",
            setup::service::preferred_manager_name()
        );
        eprintln!("Use `./aether-tunnel stop` to stop it first, or manage via subcommands:");
        eprintln!("  ./aether-tunnel status / logs / restart / stop");
        std::process::exit(1);
    }

    // Resolve server list: if a config file exists, it must use [[servers]].
    // Otherwise fall back to CLI/env single-server mode.
    let config_path =
        std::env::var("AETHER_TUNNEL_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_string());
    let servers = if std::path::Path::new(&config_path).exists() {
        let file_cfg = config::ConfigFile::load(std::path::Path::new(&config_path))?;
        if file_cfg.servers.is_empty() {
            anyhow::bail!(
                "config file {} must contain at least one [[servers]] entry",
                config_path
            );
        }
        file_cfg.servers.clone()
    } else {
        vec![single_server_entry(&config, single_server_tunnel_security)]
    };

    app::run(config, servers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_config_and_security(args: &[&str]) -> (Config, Option<TunnelSecurity>) {
        let matches = build_command()
            .try_get_matches_from(args)
            .expect("arguments should parse");
        let config = Config::from_arg_matches(&matches).expect("config should parse");
        let tunnel_security = configured_tunnel_security_from_matches(&matches, &config);
        (config, tunnel_security)
    }

    #[test]
    fn single_server_entry_omits_default_tunnel_security_for_auto_inference() {
        let (config, tunnel_security) = parse_config_and_security(&[
            "aether-tunnel",
            "--aether-url",
            "http://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-encryption-key",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        ]);

        let entry = single_server_entry(&config, tunnel_security);
        assert_eq!(entry.tunnel_security, None);
        assert_eq!(
            config::effective_tunnel_security(
                &entry.aether_url,
                entry.tunnel_security,
                entry.tunnel_encryption_key.as_deref(),
            ),
            TunnelSecurity::NonTlsRequired
        );
    }

    #[test]
    fn single_server_entry_preserves_explicit_tunnel_security_off() {
        let (config, tunnel_security) = parse_config_and_security(&[
            "aether-tunnel",
            "--aether-url",
            "http://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-security",
            "off",
            "--tunnel-encryption-key",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        ]);

        let entry = single_server_entry(&config, tunnel_security);
        assert_eq!(entry.tunnel_security, Some(TunnelSecurity::Off));
        assert_eq!(
            config::effective_tunnel_security(
                &entry.aether_url,
                entry.tunnel_security,
                entry.tunnel_encryption_key.as_deref(),
            ),
            TunnelSecurity::Off
        );
    }
}
