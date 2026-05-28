use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::risk_control::{read_risk_control_runtime_config, RiskControlRuntimeConfig};
use crate::AppState;

const RISK_CONTROL_RETENTION_DISABLED_RECHECK: Duration = Duration::from_secs(5 * 60);
const RISK_CONTROL_RETENTION_STATUS_KEY: &str = "module.risk_control.retention.status";

pub(crate) fn spawn_risk_control_retention_worker(state: AppState) -> Option<JoinHandle<()>> {
    if !state.data.has_risk_control_writer() {
        return None;
    }

    Some(tokio::spawn(async move {
        loop {
            let Some(delay) = next_retention_delay(&state).await else {
                tokio::time::sleep(RISK_CONTROL_RETENTION_DISABLED_RECHECK).await;
                continue;
            };

            record_risk_control_retention_next_run(
                &state,
                current_unix_secs().saturating_add(delay.as_secs()),
            )
            .await;
            tokio::time::sleep(delay).await;

            if next_retention_delay(&state).await.is_none() {
                continue;
            }

            match run_risk_control_retention_once(&state).await {
                Ok(summary) => {
                    info!(
                        event_name = "risk_control_retention_run",
                        log_type = "ops",
                        hit_deleted = summary.hit_deleted,
                        non_hit_deleted = summary.non_hit_deleted,
                        "risk control retention sweep completed"
                    );
                }
                Err(err) => {
                    warn!(
                        event_name = "risk_control_retention_run_failed",
                        log_type = "ops",
                        error = ?err,
                        "risk control retention sweep failed"
                    );
                }
            }
        }
    }))
}

async fn next_retention_delay(state: &AppState) -> Option<Duration> {
    let config = match read_risk_control_runtime_config(state).await {
        Ok(config) => config,
        Err(err) => {
            warn!(
                event_name = "risk_control_retention_config_read_failed",
                log_type = "ops",
                error = ?err,
                "risk control retention worker failed to read config"
            );
            return None;
        }
    };
    let interval_minutes = config.retention.auto_run_interval_minutes;
    (config.enabled && interval_minutes > 0)
        .then(|| Duration::from_secs(interval_minutes.saturating_mul(60)))
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RiskControlRetentionRunSummary {
    pub(crate) hit_deleted: u64,
    pub(crate) non_hit_deleted: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RiskControlRetentionStatus {
    #[serde(default)]
    pub(crate) last_started_at_unix_secs: Option<u64>,
    #[serde(default)]
    pub(crate) last_completed_at_unix_secs: Option<u64>,
    #[serde(default)]
    pub(crate) last_success: Option<bool>,
    #[serde(default)]
    pub(crate) last_hit_deleted: u64,
    #[serde(default)]
    pub(crate) last_non_hit_deleted: u64,
    #[serde(default)]
    pub(crate) last_error: Option<String>,
    #[serde(default)]
    pub(crate) next_run_at_unix_secs: Option<u64>,
}

pub(crate) async fn read_risk_control_retention_status(
    state: &AppState,
    config: &RiskControlRuntimeConfig,
) -> Result<RiskControlRetentionStatus, crate::GatewayError> {
    let mut status = match state
        .read_system_config_json_value(RISK_CONTROL_RETENTION_STATUS_KEY)
        .await?
    {
        Some(value) => match serde_json::from_value::<RiskControlRetentionStatus>(value) {
            Ok(status) => status,
            Err(err) => {
                warn!(
                    event_name = "risk_control_retention_status_invalid",
                    log_type = "ops",
                    error = ?err,
                    "risk control retention status is invalid; falling back to defaults"
                );
                RiskControlRetentionStatus::default()
            }
        },
        None => RiskControlRetentionStatus::default(),
    };
    status.next_run_at_unix_secs = computed_next_run_at(config, &status, current_unix_secs());
    Ok(status)
}

pub(crate) async fn run_risk_control_retention_once(
    state: &AppState,
) -> Result<RiskControlRetentionRunSummary, crate::GatewayError> {
    let started_at = current_unix_secs();
    let result = async {
        let config = read_risk_control_runtime_config(state).await?;
        let now = current_unix_secs();
        let hit_before = days_cutoff(now, config.retention.hit_days);
        let non_hit_before = days_cutoff(now, config.retention.non_hit_days);
        let summary = state
            .data
            .delete_expired_risk_control_logs(hit_before, non_hit_before)
            .await
            .map_err(|err| crate::GatewayError::Internal(err.to_string()))?;
        Ok::<_, crate::GatewayError>(RiskControlRetentionRunSummary {
            hit_deleted: summary.hit_deleted,
            non_hit_deleted: summary.non_hit_deleted,
        })
    }
    .await;

    let completed_at = current_unix_secs();
    match &result {
        Ok(summary) => {
            record_risk_control_retention_status(
                state,
                RiskControlRetentionStatus {
                    last_started_at_unix_secs: Some(started_at),
                    last_completed_at_unix_secs: Some(completed_at),
                    last_success: Some(true),
                    last_hit_deleted: summary.hit_deleted,
                    last_non_hit_deleted: summary.non_hit_deleted,
                    last_error: None,
                    next_run_at_unix_secs: next_run_at_from_now(state, completed_at).await,
                },
            )
            .await;
        }
        Err(err) => {
            record_risk_control_retention_status(
                state,
                RiskControlRetentionStatus {
                    last_started_at_unix_secs: Some(started_at),
                    last_completed_at_unix_secs: Some(completed_at),
                    last_success: Some(false),
                    last_error: Some(format!("{err:?}")),
                    next_run_at_unix_secs: next_run_at_from_now(state, completed_at).await,
                    ..RiskControlRetentionStatus::default()
                },
            )
            .await;
        }
    }
    result
}

fn days_cutoff(now: u64, days: u64) -> Option<u64> {
    (days > 0).then(|| now.saturating_sub(days.saturating_mul(86_400)))
}

async fn next_run_at_from_now(state: &AppState, now: u64) -> Option<u64> {
    let config = read_risk_control_runtime_config(state).await.ok()?;
    if !config.enabled || config.retention.auto_run_interval_minutes == 0 {
        return None;
    }
    Some(
        now.saturating_add(
            config
                .retention
                .auto_run_interval_minutes
                .saturating_mul(60),
        ),
    )
}

fn computed_next_run_at(
    config: &RiskControlRuntimeConfig,
    status: &RiskControlRetentionStatus,
    now: u64,
) -> Option<u64> {
    let interval_secs = config
        .retention
        .auto_run_interval_minutes
        .saturating_mul(60);
    if !config.enabled || interval_secs == 0 {
        return None;
    }
    if let Some(next) = status.next_run_at_unix_secs.filter(|value| *value > now) {
        return Some(next);
    }
    status
        .last_completed_at_unix_secs
        .map(|value| value.saturating_add(interval_secs))
        .filter(|value| *value > now)
        .or_else(|| Some(now.saturating_add(interval_secs)))
}

async fn record_risk_control_retention_next_run(state: &AppState, next_run_at_unix_secs: u64) {
    let mut status = state
        .read_system_config_json_value(RISK_CONTROL_RETENTION_STATUS_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<RiskControlRetentionStatus>(value).ok())
        .unwrap_or_default();
    status.next_run_at_unix_secs = Some(next_run_at_unix_secs);
    record_risk_control_retention_status(state, status).await;
}

async fn record_risk_control_retention_status(
    state: &AppState,
    status: RiskControlRetentionStatus,
) {
    if let Err(err) = state
        .upsert_system_config_json_value(
            RISK_CONTROL_RETENTION_STATUS_KEY,
            &json!(status),
            Some("Risk control retention runtime status"),
        )
        .await
    {
        warn!(
            event_name = "risk_control_retention_status_write_failed",
            log_type = "ops",
            error = ?err,
            "failed to persist risk control retention status"
        );
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn retention_state(enabled: bool, interval_minutes: u64) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled().with_system_config_values_for_tests([
                    (
                        crate::risk_control::RISK_CONTROL_ENABLED_CONFIG_KEY.to_string(),
                        json!(enabled),
                    ),
                    (
                        crate::risk_control::RISK_CONTROL_CONFIG_KEY.to_string(),
                        json!({
                            "mode": "observe",
                            "retention": {
                                "auto_run_interval_minutes": interval_minutes
                            }
                        }),
                    ),
                ]),
            )
    }

    #[tokio::test]
    async fn next_retention_delay_is_none_when_module_disabled() {
        let state = retention_state(false, 60);

        assert!(next_retention_delay(&state).await.is_none());
    }

    #[tokio::test]
    async fn next_retention_delay_is_none_when_interval_zero() {
        let state = retention_state(true, 0);

        assert!(next_retention_delay(&state).await.is_none());
    }

    #[tokio::test]
    async fn next_retention_delay_uses_configured_interval() {
        let state = retention_state(true, 7);

        assert_eq!(
            next_retention_delay(&state).await,
            Some(Duration::from_secs(7 * 60))
        );
    }

    #[tokio::test]
    async fn retention_status_falls_back_when_persisted_value_is_invalid() {
        let state = retention_state(true, 10);
        state
            .upsert_system_config_json_value(
                RISK_CONTROL_RETENTION_STATUS_KEY,
                &json!("invalid-status"),
                Some("test invalid status"),
            )
            .await
            .expect("status fixture should write");
        let config = read_risk_control_runtime_config(&state)
            .await
            .expect("config should read");

        let status = read_risk_control_retention_status(&state, &config)
            .await
            .expect("invalid status should not fail status payload");

        assert_eq!(status.last_completed_at_unix_secs, None);
        assert!(status.next_run_at_unix_secs.is_some());
    }

    #[test]
    fn computed_next_run_uses_stored_future_run_or_interval() {
        let mut config = RiskControlRuntimeConfig::default();
        config.enabled = true;
        config.retention.auto_run_interval_minutes = 10;
        let status = RiskControlRetentionStatus {
            next_run_at_unix_secs: Some(1_500),
            last_completed_at_unix_secs: Some(900),
            ..RiskControlRetentionStatus::default()
        };

        assert_eq!(computed_next_run_at(&config, &status, 1_000), Some(1_500));

        let stale = RiskControlRetentionStatus {
            next_run_at_unix_secs: Some(950),
            last_completed_at_unix_secs: Some(900),
            ..RiskControlRetentionStatus::default()
        };
        assert_eq!(computed_next_run_at(&config, &stale, 1_000), Some(1_500));
    }
}
