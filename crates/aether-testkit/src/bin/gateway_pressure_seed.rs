use std::env;
use std::fs;
use std::path::PathBuf;

use aether_data::repository::auth::CreateStandaloneApiKeyRecord;
use aether_data::repository::wallet::WalletLookupKey;
use aether_data::{
    DataBackends, DataLayerConfig, DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig,
};
use aether_data_contracts::repository::global_models::{
    CreateAdminGlobalModelRecord, UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use sha2::Digest;

const DEFAULT_POSTGRES_URL: &str = "postgresql://postgres:aether@127.0.0.1:5432/aether";
const DEFAULT_OUTPUT_ENV_PATH: &str = "/tmp/aether_local_env.sh";
const DEFAULT_OUTPUT_KEY_PATH: &str = "/tmp/aether_fullchain_api_key";
const DEFAULT_OUTPUT_KEY_LIST_PATH: &str = "/tmp/aether_fullchain_api_keys";
const DEFAULT_PROVIDER_ID: &str = "provider-local-pressure-openai";
const DEFAULT_ENDPOINT_ID: &str = "endpoint-local-pressure-openai-chat";
const DEFAULT_PROVIDER_KEY_ID: &str = "provider-key-local-pressure-openai";
const DEFAULT_GLOBAL_MODEL_ID: &str = "gm-local-pressure-gpt-5-mini";
const DEFAULT_MODEL_ID: &str = "model-local-pressure-gpt-5-mini";
const DEFAULT_API_KEY_ID: &str = "api-key-local-pressure";
const DEFAULT_OPERATOR_ID: &str = "pressure-local";
const DEFAULT_MODEL: &str = "gpt-5-mini";
const DEFAULT_MOCK_UPSTREAM_BASE_URL: &str = "http://127.0.0.1:18181/v1";
const DEFAULT_GATEWAY_BASE_URL: &str = "http://127.0.0.1:8084";
const DEFAULT_API_KEY: &str = "sk-aether-local-pressure";

#[derive(Debug, Clone)]
struct Config {
    database_url: String,
    output_env_path: PathBuf,
    output_key_path: PathBuf,
    output_key_list_path: PathBuf,
    provider_id: String,
    endpoint_id: String,
    provider_key_id: String,
    global_model_id: String,
    model_id: String,
    api_key_id: String,
    operator_id: String,
    model: String,
    mock_upstream_base_url: String,
    gateway_base_url: String,
    api_key: String,
    api_key_count: usize,
    provider_api_key: String,
    postgres_min_connections: u32,
    postgres_max_connections: u32,
}

impl Config {
    fn from_env_and_args() -> Result<Self, String> {
        let mut config = Self {
            database_url: env_value("DATABASE_URL")
                .or_else(|| env_value("AETHER_DATABASE_URL"))
                .or_else(|| env_value("AETHER_GATEWAY_DATA_POSTGRES_URL"))
                .unwrap_or_else(|| DEFAULT_POSTGRES_URL.to_string()),
            output_env_path: PathBuf::from(
                env_value("OUTPUT_ENV_PATH").unwrap_or_else(|| DEFAULT_OUTPUT_ENV_PATH.to_string()),
            ),
            output_key_path: PathBuf::from(
                env_value("OUTPUT_KEY_PATH").unwrap_or_else(|| DEFAULT_OUTPUT_KEY_PATH.to_string()),
            ),
            output_key_list_path: PathBuf::from(
                env_value("OUTPUT_KEY_LIST_PATH")
                    .or_else(|| env_value("PRESSURE_API_KEY_LIST_FILE"))
                    .unwrap_or_else(|| DEFAULT_OUTPUT_KEY_LIST_PATH.to_string()),
            ),
            provider_id: env_value("PRESSURE_PROVIDER_ID")
                .unwrap_or_else(|| DEFAULT_PROVIDER_ID.to_string()),
            endpoint_id: env_value("PRESSURE_ENDPOINT_ID")
                .unwrap_or_else(|| DEFAULT_ENDPOINT_ID.to_string()),
            provider_key_id: env_value("PRESSURE_PROVIDER_KEY_ID")
                .unwrap_or_else(|| DEFAULT_PROVIDER_KEY_ID.to_string()),
            global_model_id: env_value("PRESSURE_GLOBAL_MODEL_ID")
                .unwrap_or_else(|| DEFAULT_GLOBAL_MODEL_ID.to_string()),
            model_id: env_value("PRESSURE_MODEL_ID")
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            api_key_id: env_value("PRESSURE_API_KEY_ID")
                .unwrap_or_else(|| DEFAULT_API_KEY_ID.to_string()),
            operator_id: env_value("PRESSURE_OPERATOR_ID")
                .unwrap_or_else(|| DEFAULT_OPERATOR_ID.to_string()),
            model: env_value("PRESSURE_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            mock_upstream_base_url: env_value("PRESSURE_MOCK_UPSTREAM_BASE_URL")
                .unwrap_or_else(|| DEFAULT_MOCK_UPSTREAM_BASE_URL.to_string()),
            gateway_base_url: env_value("GATEWAY_BASE_URL")
                .unwrap_or_else(|| DEFAULT_GATEWAY_BASE_URL.to_string()),
            api_key: env_value("AETHER_API_KEY").unwrap_or_else(|| DEFAULT_API_KEY.to_string()),
            api_key_count: env_value("PRESSURE_API_KEY_COUNT")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1),
            provider_api_key: env_value("PRESSURE_PROVIDER_API_KEY")
                .unwrap_or_else(|| "dummy-local-pressure-provider-key".to_string()),
            postgres_min_connections: 1,
            postgres_max_connections: 8,
        };

        let args = env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            match arg.as_str() {
                "--database-url" => config.database_url = arg_value(&args, &mut index, arg)?,
                "--output-env" => {
                    config.output_env_path = PathBuf::from(arg_value(&args, &mut index, arg)?)
                }
                "--output-key" => {
                    config.output_key_path = PathBuf::from(arg_value(&args, &mut index, arg)?)
                }
                "--output-key-list" => {
                    config.output_key_list_path = PathBuf::from(arg_value(&args, &mut index, arg)?)
                }
                "--provider-id" => config.provider_id = arg_value(&args, &mut index, arg)?,
                "--endpoint-id" => config.endpoint_id = arg_value(&args, &mut index, arg)?,
                "--provider-key-id" => config.provider_key_id = arg_value(&args, &mut index, arg)?,
                "--global-model-id" => config.global_model_id = arg_value(&args, &mut index, arg)?,
                "--model-id" => config.model_id = arg_value(&args, &mut index, arg)?,
                "--api-key-id" => config.api_key_id = arg_value(&args, &mut index, arg)?,
                "--operator-id" => config.operator_id = arg_value(&args, &mut index, arg)?,
                "--model" => config.model = arg_value(&args, &mut index, arg)?,
                "--mock-upstream-base-url" => {
                    config.mock_upstream_base_url = arg_value(&args, &mut index, arg)?
                }
                "--gateway-base-url" => {
                    config.gateway_base_url = arg_value(&args, &mut index, arg)?
                }
                "--api-key" => config.api_key = arg_value(&args, &mut index, arg)?,
                "--api-key-count" => {
                    config.api_key_count = parse_usize(&arg_value(&args, &mut index, arg)?, arg)?
                }
                "--provider-api-key" => {
                    config.provider_api_key = arg_value(&args, &mut index, arg)?
                }
                "--postgres-min-connections" => {
                    config.postgres_min_connections =
                        parse_u32(&arg_value(&args, &mut index, arg)?, arg)?
                }
                "--postgres-max-connections" => {
                    config.postgres_max_connections =
                        parse_u32(&arg_value(&args, &mut index, arg)?, arg)?
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
            index += 1;
        }

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("database_url", &self.database_url),
            ("provider_id", &self.provider_id),
            ("endpoint_id", &self.endpoint_id),
            ("provider_key_id", &self.provider_key_id),
            ("global_model_id", &self.global_model_id),
            ("model_id", &self.model_id),
            ("api_key_id", &self.api_key_id),
            ("operator_id", &self.operator_id),
            ("model", &self.model),
            ("mock_upstream_base_url", &self.mock_upstream_base_url),
            ("gateway_base_url", &self.gateway_base_url),
            ("api_key", &self.api_key),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{name} cannot be empty"));
            }
        }
        if self.postgres_min_connections > self.postgres_max_connections {
            return Err("postgres min connections cannot exceed max connections".to_string());
        }
        if self.api_key_count == 0 {
            return Err("api_key_count must be positive".to_string());
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env_and_args().map_err(|err| format!("invalid config: {err}"))?;

    let backends = DataBackends::from_config(DataLayerConfig::from_database(SqlDatabaseConfig {
        driver: DatabaseDriver::Postgres,
        url: config.database_url.clone(),
        pool: SqlPoolConfig {
            min_connections: config.postgres_min_connections,
            max_connections: config.postgres_max_connections,
            acquire_timeout_ms: 5_000,
            idle_timeout_ms: 30_000,
            max_lifetime_ms: 300_000,
            statement_cache_capacity: 128,
            require_ssl: false,
        },
    }))?;

    seed_provider_catalog(&backends, &config).await?;
    seed_models(&backends, &config).await?;
    let operator_user_id = seed_operator_user(&backends, &config).await?;
    seed_api_keys(&backends, &config, &operator_user_id).await?;
    verify_candidate_selection(&backends, &config).await?;
    write_outputs(&config)?;

    println!("gateway pressure seed complete");
    println!("provider_id={}", config.provider_id);
    println!("endpoint_id={}", config.endpoint_id);
    println!("provider_key_id={}", config.provider_key_id);
    println!("model={}", config.model);
    println!("api_key_id={}", config.api_key_id);
    println!("api_key_count={}", config.api_key_count);
    println!("env written to {}", config.output_env_path.display());
    println!("api key written to {}", config.output_key_path.display());
    println!(
        "api key list written to {}",
        config.output_key_list_path.display()
    );

    Ok(())
}

async fn seed_provider_catalog(
    backends: &DataBackends,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let reader = backends
        .read()
        .provider_catalog()
        .ok_or("provider catalog reader unavailable")?;
    let writer = backends
        .write()
        .provider_catalog()
        .ok_or("provider catalog writer unavailable")?;

    let provider = StoredProviderCatalogProvider::new(
        config.provider_id.clone(),
        "Local pressure OpenAI mock".to_string(),
        Some("http://127.0.0.1:18181".to_string()),
        "openai".to_string(),
    )?
    .with_transport_fields(
        true,
        false,
        false,
        None,
        Some(0),
        None,
        Some(120.0),
        Some(30.0),
        None,
    )
    .with_routing_fields(0)
    .with_description(Some(
        "Local OpenAI-compatible mock provider for gateway pressure tests".to_string(),
    ));

    if reader
        .list_providers_by_ids(std::slice::from_ref(&config.provider_id))
        .await?
        .is_empty()
    {
        writer.create_provider(&provider, None).await?;
    } else {
        writer.update_provider(&provider).await?;
    }

    let endpoint = StoredProviderCatalogEndpoint::new(
        config.endpoint_id.clone(),
        config.provider_id.clone(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat_completions".to_string()),
        true,
    )?
    .with_transport_fields(
        config.mock_upstream_base_url.clone(),
        None,
        None,
        Some(0),
        None,
        None,
        None,
        None,
    )?
    .with_health_score(1.0);

    if reader
        .list_endpoints_by_ids(std::slice::from_ref(&config.endpoint_id))
        .await?
        .is_empty()
    {
        writer.create_endpoint(&endpoint).await?;
    } else {
        writer.update_endpoint(&endpoint).await?;
    }

    let provider_key = StoredProviderCatalogKey::new(
        config.provider_key_id.clone(),
        config.provider_id.clone(),
        "Local pressure mock key".to_string(),
        "api_key".to_string(),
        Some(json!({"streaming": true})),
        true,
    )?
    .with_transport_fields(
        Some(json!(["openai:chat"])),
        Some(config.provider_api_key.clone()),
        None,
        None,
        None,
        Some(json!([config.model.clone()])),
        None,
        None,
        None,
    )?
    .with_rate_limit_fields(None, None, None, None, None, None, None, None, None)
    .with_health_fields(
        Some(json!({"openai:chat": {"status": "healthy"}})),
        Some(json!({"openai:chat": {"state": "closed"}})),
    );

    if reader
        .list_keys_by_ids(std::slice::from_ref(&config.provider_key_id))
        .await?
        .is_empty()
    {
        writer.create_key(&provider_key).await?;
    } else {
        writer.update_key(&provider_key).await?;
    }

    Ok(())
}

async fn seed_models(
    backends: &DataBackends,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let reader = backends
        .read()
        .global_models()
        .ok_or("global model reader unavailable")?;
    let writer = backends
        .write()
        .global_models()
        .ok_or("global model writer unavailable")?;

    let capabilities = Some(json!({
        "streaming": true,
        "chat": true
    }));
    let global_config = Some(json!({
        "model_mappings": [config.model],
        "pressure_seed": true
    }));

    if reader
        .get_admin_global_model_by_id(&config.global_model_id)
        .await?
        .is_some()
    {
        writer
            .update_admin_global_model(&UpdateAdminGlobalModelRecord::new(
                config.global_model_id.clone(),
                config.model.clone(),
                true,
                Some(0.0),
                None,
                capabilities.clone(),
                global_config.clone(),
            )?)
            .await?;
    } else {
        writer
            .create_admin_global_model(&CreateAdminGlobalModelRecord::new(
                config.global_model_id.clone(),
                config.model.clone(),
                config.model.clone(),
                true,
                Some(0.0),
                None,
                capabilities.clone(),
                global_config.clone(),
            )?)
            .await?;
    }

    let provider_model_mappings = Some(json!([
        {
            "name": config.model,
            "priority": 0,
            "api_formats": ["openai:chat"],
            "endpoint_ids": [config.endpoint_id]
        }
    ]));
    let provider_model = UpsertAdminProviderModelRecord::new(
        config.model_id.clone(),
        config.provider_id.clone(),
        config.global_model_id.clone(),
        config.model.clone(),
        provider_model_mappings,
        Some(0.0),
        None,
        Some(false),
        Some(false),
        Some(true),
        Some(false),
        Some(false),
        true,
        true,
        Some(json!({"pressure_seed": true})),
    )?;

    if reader
        .list_admin_provider_models_by_global_model_id(&config.global_model_id)
        .await?
        .iter()
        .any(|model| model.id == config.model_id)
    {
        writer.update_admin_provider_model(&provider_model).await?;
    } else {
        writer.create_admin_provider_model(&provider_model).await?;
    }

    Ok(())
}

async fn seed_api_keys(
    backends: &DataBackends,
    config: &Config,
    operator_user_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for index in 0..config.api_key_count {
        seed_api_key(backends, config, operator_user_id, index).await?;
    }
    Ok(())
}

async fn seed_api_key(
    backends: &DataBackends,
    config: &Config,
    operator_user_id: &str,
    key_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let auth_reader = backends
        .read()
        .auth_api_keys()
        .ok_or("auth api key reader unavailable")?;
    let auth_writer = backends
        .write()
        .auth_api_keys()
        .ok_or("auth api key writer unavailable")?;
    let wallet_reader = backends
        .read()
        .wallets()
        .ok_or("wallet reader unavailable")?;

    let api_key_id = pressure_api_key_id(config, key_index);
    let api_key_value = pressure_api_key_value(config, key_index);

    let existing = auth_reader
        .find_export_standalone_api_key_by_id(&api_key_id)
        .await?;
    if existing.is_none() {
        auth_writer
            .create_standalone_api_key(CreateStandaloneApiKeyRecord {
                user_id: operator_user_id.to_string(),
                api_key_id: api_key_id.clone(),
                key_hash: sha256_hex(&api_key_value),
                key_encrypted: Some(api_key_value),
                name: Some(format!("Local pressure API key {}", key_index + 1)),
                allowed_providers: Some(vec![config.provider_id.clone()]),
                allowed_api_formats: Some(vec!["openai:chat".to_string()]),
                allowed_models: Some(vec![config.model.clone()]),
                ip_rules: None,
                rate_limit: Some(0),
                concurrent_limit: None,
                force_capabilities: None,
                is_active: true,
                expires_at_unix_secs: None,
                auto_delete_on_expiry: false,
                total_requests: 0,
                total_tokens: 0,
                total_cost_usd: 0.0,
            })
            .await?;
    } else {
        auth_writer
            .update_standalone_api_key_basic(
                aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord {
                    api_key_id: api_key_id.clone(),
                    name: Some(format!("Local pressure API key {}", key_index + 1)),
                    rate_limit_present: true,
                    rate_limit: Some(0),
                    concurrent_limit_present: true,
                    concurrent_limit: None,
                    allowed_providers: Some(Some(vec![config.provider_id.clone()])),
                    allowed_api_formats: Some(Some(vec!["openai:chat".to_string()])),
                    allowed_models: Some(Some(vec![config.model.clone()])),
                    ip_rules: Some(None),
                    expires_at_present: true,
                    expires_at_unix_secs: None,
                    auto_delete_on_expiry_present: true,
                    auto_delete_on_expiry: false,
                },
            )
            .await?;
        auth_writer
            .set_standalone_api_key_active(&api_key_id, true)
            .await?;
    }

    if wallet_reader
        .find(WalletLookupKey::ApiKeyId(&api_key_id))
        .await?
        .is_none()
    {
        wallet_reader
            .initialize_auth_api_key_wallet(&api_key_id, 0.0, true)
            .await?;
    } else {
        wallet_reader
            .update_auth_api_key_wallet_limit_mode(&api_key_id, "unlimited")
            .await?;
    }

    Ok(())
}

async fn seed_operator_user(
    backends: &DataBackends,
    config: &Config,
) -> Result<String, Box<dyn std::error::Error>> {
    let user_reader = backends.read().users().ok_or("user reader unavailable")?;
    let wallet_reader = backends
        .read()
        .wallets()
        .ok_or("wallet reader unavailable")?;
    let username = format!("{}-user", config.operator_id);

    let user = match user_reader.find_user_auth_by_username(&username).await? {
        Some(user) => user_reader
            .update_local_auth_user_admin_fields(
                &user.id,
                Some("admin".to_string()),
                true,
                Some(vec![config.provider_id.clone()]),
                true,
                Some(vec!["openai:chat".to_string()]),
                true,
                Some(vec![config.model.clone()]),
                true,
                None,
                Some(true),
            )
            .await?
            .unwrap_or(user),
        None => user_reader
            .create_local_auth_user_with_settings(
                Some(format!("{}@local.pressure", config.operator_id)),
                true,
                username,
                "local-pressure-password-disabled".to_string(),
                "admin".to_string(),
                Some(vec![config.provider_id.clone()]),
                Some(vec!["openai:chat".to_string()]),
                Some(vec![config.model.clone()]),
                None,
            )
            .await?
            .ok_or("failed to create pressure operator user")?,
    };

    if wallet_reader
        .find(WalletLookupKey::UserId(&user.id))
        .await?
        .is_none()
    {
        wallet_reader
            .initialize_auth_user_wallet(&user.id, 0.0, true)
            .await?;
    } else {
        wallet_reader
            .update_auth_user_wallet_limit_mode(&user.id, "unlimited")
            .await?;
    }

    Ok(user.id)
}

async fn verify_candidate_selection(
    backends: &DataBackends,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let reader = backends
        .read()
        .minimal_candidate_selection()
        .ok_or("candidate selection reader unavailable")?;
    let rows = reader
        .list_for_exact_api_format_and_requested_model("openai:chat", &config.model)
        .await?;
    let has_pressure_row = rows.iter().any(|row| {
        row.provider_id == config.provider_id
            && row.endpoint_id == config.endpoint_id
            && row.key_id == config.provider_key_id
            && row.global_model_id == config.global_model_id
            && row.model_id == config.model_id
            && row.provider_is_active
            && row.endpoint_is_active
            && row.key_is_active
            && row.model_is_active
            && row.model_is_available
    });
    if !has_pressure_row {
        return Err(format!(
            "seeded candidate not visible for model {} and openai:chat",
            config.model
        )
        .into());
    }
    Ok(())
}

fn write_outputs(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = config.output_env_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = config.output_key_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = config.output_key_list_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&config.output_key_path, format!("{}\n", config.api_key))?;
    let key_list = (0..config.api_key_count)
        .map(|index| pressure_api_key_value(config, index))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&config.output_key_list_path, format!("{key_list}\n"))?;
    let env_content = format!(
        concat!(
            "export AETHER_API_KEY_FILE={key_path}\n",
            "export AETHER_API_KEY_LIST_FILE={key_list_path}\n",
            "export AETHER_API_KEY=$(cat {key_path})\n",
            "export GATEWAY_BASE_URL={gateway_base_url}\n",
            "export TARGET_URL={gateway_base_url}/v1/chat/completions\n",
            "export METRICS_URL={gateway_base_url}/_gateway/metrics\n",
            "export PRESSURE_MODEL={model}\n",
            "export PRESSURE_MOCK_UPSTREAM_BASE_URL={mock_upstream_base_url}\n",
            "export PRESSURE_MOCK_UPSTREAM_METRICS_URL=http://127.0.0.1:18181/metrics\n"
        ),
        key_path = shell_escape(&config.output_key_path.display().to_string()),
        key_list_path = shell_escape(&config.output_key_list_path.display().to_string()),
        gateway_base_url = shell_escape(config.gateway_base_url.trim_end_matches('/')),
        model = shell_escape(&config.model),
        mock_upstream_base_url = shell_escape(&config.mock_upstream_base_url),
    );
    fs::write(&config.output_env_path, env_content)?;

    Ok(())
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn pressure_api_key_id(config: &Config, index: usize) -> String {
    if index == 0 {
        config.api_key_id.clone()
    } else {
        format!("{}-{}", config.api_key_id, index + 1)
    }
}

fn pressure_api_key_value(config: &Config, index: usize) -> String {
    if index == 0 {
        config.api_key.clone()
    } else {
        format!("{}-{}", config.api_key, index + 1)
    }
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | ':' | '_' | '-'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn arg_value(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{name} must be an unsigned integer"))
}

fn parse_usize(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be an unsigned integer"))
}

fn print_help() {
    println!(
        "Usage: cargo run -p aether-testkit --bin gateway_pressure_seed -- [options]\n\
\n\
Options:\n\
  --database-url URL\n\
  --output-env PATH\n\
  --output-key PATH\n\
  --output-key-list PATH\n\
  --gateway-base-url URL\n\
  --mock-upstream-base-url URL\n\
  --model NAME\n\
  --api-key VALUE\n\
  --api-key-count N\n\
  --provider-api-key VALUE\n\
  --provider-id ID\n\
  --endpoint-id ID\n\
  --provider-key-id ID\n\
  --global-model-id ID\n\
  --model-id ID\n\
  --api-key-id ID\n"
    );
}
