use std::time::Duration;

use aether_loadtools::{
    run_http_load_probe_with_options, HttpLoadProbeConfig, HttpLoadProbeOptions,
};
use reqwest::Method;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (config, options) = parse_args(std::env::args().skip(1).collect())?;
    let result = run_http_load_probe_with_options(&config, options)
        .await
        .map_err(std::io::Error::other)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<(HttpLoadProbeConfig, HttpLoadProbeOptions), Box<dyn std::error::Error>> {
    let mut url: Option<String> = None;
    let mut warmup_url: Option<String> = None;
    let mut total_requests: Option<usize> = None;
    let mut concurrency: Option<usize> = None;
    let mut warmup_connections: usize = 0;
    let mut timeout_ms: Option<u64> = None;
    let mut connect_timeout_ms: Option<u64> = None;
    let mut client_shards: Option<usize> = None;
    let mut pool_max_idle_per_host: Option<usize> = None;
    let mut start_ramp_ms: u64 = 0;
    let mut first_body_hold_ms: u64 = 0;
    let mut method = Method::GET;
    let mut headers = std::collections::BTreeMap::new();
    let mut body: Option<Vec<u8>> = None;
    let mut response_mode = aether_loadtools::HttpLoadProbeResponseMode::HeadersOnly;
    let mut require_sse_done = false;
    let mut http1_only = false;
    let mut http2_prior_knowledge = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => url = Some(next_value(&mut iter, "--url")?),
            "--warmup-url" => warmup_url = Some(next_value(&mut iter, "--warmup-url")?),
            "--requests" => total_requests = Some(next_value(&mut iter, "--requests")?.parse()?),
            "--concurrency" => concurrency = Some(next_value(&mut iter, "--concurrency")?.parse()?),
            "--warmup-connections" => {
                warmup_connections = next_value(&mut iter, "--warmup-connections")?.parse()?
            }
            "--timeout-ms" => timeout_ms = Some(next_value(&mut iter, "--timeout-ms")?.parse()?),
            "--connect-timeout-ms" => {
                connect_timeout_ms = Some(next_value(&mut iter, "--connect-timeout-ms")?.parse()?)
            }
            "--client-shards" => {
                client_shards = Some(next_value(&mut iter, "--client-shards")?.parse()?)
            }
            "--pool-max-idle-per-host" => {
                pool_max_idle_per_host =
                    Some(next_value(&mut iter, "--pool-max-idle-per-host")?.parse()?)
            }
            "--start-ramp-ms" => {
                start_ramp_ms = next_value(&mut iter, "--start-ramp-ms")?.parse()?
            }
            "--first-body-hold-ms" => {
                first_body_hold_ms = next_value(&mut iter, "--first-body-hold-ms")?.parse()?
            }
            "--http1-only" => http1_only = true,
            "--http2-prior-knowledge" => http2_prior_knowledge = true,
            "--method" => {
                method = Method::from_bytes(next_value(&mut iter, "--method")?.as_bytes())?
            }
            "--header" | "-H" => {
                let (name, value) = parse_header_arg(&next_value(&mut iter, "--header")?)?;
                headers.insert(name, value);
            }
            "--body" => body = Some(next_value(&mut iter, "--body")?.into_bytes()),
            "--body-file" => body = Some(std::fs::read(next_value(&mut iter, "--body-file")?)?),
            "--response-mode" => {
                response_mode = parse_response_mode(&next_value(&mut iter, "--response-mode")?)?
            }
            "--require-sse-done" => require_sse_done = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                )
                .into());
            }
        }
    }

    let mut config = HttpLoadProbeConfig {
        url: url.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing required --url")
        })?,
        total_requests: total_requests.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing required --requests",
            )
        })?,
        concurrency: concurrency.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing required --concurrency",
            )
        })?,
        method,
        headers,
        body,
        response_mode,
        ..HttpLoadProbeConfig::default()
    };
    config.warmup_url = warmup_url;
    config.warmup_connections = warmup_connections;
    if let Some(timeout_ms) = timeout_ms {
        config.timeout = Duration::from_millis(timeout_ms);
    }
    config.connect_timeout = connect_timeout_ms.map(Duration::from_millis);
    if let Some(client_shards) = client_shards {
        config.client_shards = client_shards;
    }
    config.pool_max_idle_per_host = pool_max_idle_per_host;
    config.start_ramp = Duration::from_millis(start_ramp_ms);
    config.first_body_hold = Duration::from_millis(first_body_hold_ms);
    config.http1_only = http1_only;
    config.http2_prior_knowledge = http2_prior_knowledge;
    config
        .validate()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    Ok((config, HttpLoadProbeOptions { require_sse_done }))
}

fn parse_header_arg(value: &str) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (name, value) = value
        .split_once(':')
        .or_else(|| value.split_once('='))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "--header expects `Name: value` or `Name=value`",
            )
        })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--header name cannot be empty",
        )
        .into());
    }
    Ok((name.to_string(), value.trim().to_string()))
}

fn parse_response_mode(
    value: &str,
) -> Result<aether_loadtools::HttpLoadProbeResponseMode, Box<dyn std::error::Error>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "headers" | "headers-only" | "header" => {
            Ok(aether_loadtools::HttpLoadProbeResponseMode::HeadersOnly)
        }
        "first-body-byte" | "first-body" | "first-byte" | "first-chunk" => {
            Ok(aether_loadtools::HttpLoadProbeResponseMode::FirstBodyByte)
        }
        "full" | "full-body" | "body" => Ok(aether_loadtools::HttpLoadProbeResponseMode::FullBody),
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unsupported --response-mode {other}; expected headers, first-body-byte, or full"
            ),
        )
        .into()),
    }
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("missing value for {flag}"),
        )
        .into()
    })
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -p aether-loadtools --bin http_load_probe -- --url <URL> --requests <N> --concurrency <N> [--warmup-url <URL>] [--warmup-connections N] [--method GET] [--timeout-ms 30000] [--connect-timeout-ms 10000] [--client-shards 1] [--pool-max-idle-per-host N] [--start-ramp-ms 0] [--first-body-hold-ms 0] [--http1-only | --http2-prior-knowledge] [-H 'Name: value'] [--body JSON | --body-file path] [--response-mode headers|first-body-byte|full] [--require-sse-done]"
    );
}
