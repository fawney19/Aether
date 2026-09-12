use aether_data_contracts::repository::usage::{
    UsageCleanupExecutionMode, UsageCleanupSummary, UsageCleanupTargets, UsageCleanupWindow,
};
use aether_data_contracts::DataLayerError;
use chrono::Utc;

use crate::data::GatewayDataState;

use super::{
    system_config_bool, usage_cleanup_settings, usage_cleanup_window,
    usage_cleanup_window_with_override,
};

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct ManualUsageCleanupPreview {
    pub detail_cutoff: chrono::DateTime<Utc>,
    pub compressed_cutoff: chrono::DateTime<Utc>,
    pub header_cutoff: chrono::DateTime<Utc>,
    pub log_cutoff: chrono::DateTime<Utc>,
    pub mode: ManualUsageCleanupMode,
    pub targets: UsageCleanupTargets,
    pub requested_older_than_days: Option<u32>,
    pub detail_count: u64,
    pub compressed_count: u64,
    pub header_count: u64,
    pub log_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManualUsageCleanupMode {
    Policy,
    OlderThanDays,
    BeforeNow,
}

impl ManualUsageCleanupMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::OlderThanDays => "older_than_days",
            Self::BeforeNow => "before_now",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManualUsageCleanupOptions {
    pub(crate) mode: ManualUsageCleanupMode,
    pub(crate) requested_older_than_days: Option<u32>,
    pub(crate) targets: UsageCleanupTargets,
}

impl ManualUsageCleanupOptions {
    pub(crate) const fn policy() -> Self {
        Self {
            mode: ManualUsageCleanupMode::Policy,
            requested_older_than_days: None,
            targets: UsageCleanupTargets::all_policy_targets(),
        }
    }
}

pub(super) async fn perform_usage_cleanup_once(
    data: &GatewayDataState,
) -> Result<UsageCleanupSummary, DataLayerError> {
    perform_usage_cleanup_once_with_override(data, None, true).await
}

pub(super) async fn perform_automatic_usage_cleanup_once(
    data: &GatewayDataState,
) -> Result<UsageCleanupSummary, DataLayerError> {
    let mut summary = perform_usage_cleanup_once(data).await?;
    cleanup_usage_policy_ledgers(data, &mut summary).await?;
    Ok(summary)
}

pub(super) async fn perform_usage_cleanup_once_with_override(
    data: &GatewayDataState,
    override_older_than: Option<chrono::Duration>,
    respect_auto_enabled: bool,
) -> Result<UsageCleanupSummary, DataLayerError> {
    let options = ManualUsageCleanupOptions {
        mode: if override_older_than.is_some() {
            ManualUsageCleanupMode::OlderThanDays
        } else {
            ManualUsageCleanupMode::Policy
        },
        requested_older_than_days: None,
        targets: UsageCleanupTargets::all_policy_targets(),
    };
    perform_usage_cleanup_once_with_options(
        data,
        options,
        override_older_than,
        respect_auto_enabled,
    )
    .await
}

pub(super) async fn perform_manual_usage_cleanup_once(
    data: &GatewayDataState,
    options: ManualUsageCleanupOptions,
) -> Result<UsageCleanupSummary, DataLayerError> {
    let override_duration = options
        .requested_older_than_days
        .map(|days| chrono::Duration::days(i64::from(days)));
    perform_usage_cleanup_once_with_options(data, options, override_duration, false).await
}

async fn perform_usage_cleanup_once_with_options(
    data: &GatewayDataState,
    options: ManualUsageCleanupOptions,
    override_older_than: Option<chrono::Duration>,
    respect_auto_enabled: bool,
) -> Result<UsageCleanupSummary, DataLayerError> {
    if !data.has_usage_writer() {
        return Ok(UsageCleanupSummary::default());
    }
    if respect_auto_enabled
        && override_older_than.is_none()
        && !system_config_bool(data, "enable_auto_cleanup", true).await?
    {
        return Ok(UsageCleanupSummary::default());
    }

    let window = compute_usage_cleanup_window(data, options.mode, override_older_than).await?;
    let settings = usage_cleanup_settings(data).await?;
    data.cleanup_usage(
        &window,
        settings.batch_size,
        settings.auto_delete_expired_keys,
        options.targets,
        cleanup_execution_mode(options.mode),
    )
    .await
}

async fn cleanup_usage_policy_ledgers(
    data: &GatewayDataState,
    summary: &mut UsageCleanupSummary,
) -> Result<(), DataLayerError> {
    if !data.has_settlement_writer() {
        return Ok(());
    }

    let settings = usage_cleanup_settings(data).await?;
    let now_unix_secs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
    let cleanup_batch_size = settings.batch_size.max(1);
    // Bound each maintenance run while still draining faster than one day's normal growth.
    for _ in 0..32 {
        let deleted = data
            .cleanup_usage_policy_cost_reservations(now_unix_secs, cleanup_batch_size)
            .await?;
        summary.cost_reservations_deleted =
            summary.cost_reservations_deleted.saturating_add(deleted);
        if deleted < cleanup_batch_size {
            break;
        }
    }
    for _ in 0..32 {
        let deleted = data
            .cleanup_usage_policy_request_admissions(now_unix_secs, cleanup_batch_size)
            .await?;
        summary.request_admissions_deleted =
            summary.request_admissions_deleted.saturating_add(deleted);
        if deleted < cleanup_batch_size {
            break;
        }
    }
    Ok(())
}

pub(crate) async fn preview_manual_usage_cleanup(
    data: &GatewayDataState,
    options: ManualUsageCleanupOptions,
) -> Result<ManualUsageCleanupPreview, DataLayerError> {
    let override_duration = options
        .requested_older_than_days
        .map(|days| chrono::Duration::days(i64::from(days)));
    let window = compute_usage_cleanup_window(data, options.mode, override_duration).await?;
    let counts = data
        .preview_usage_cleanup(
            &window,
            options.targets,
            cleanup_execution_mode(options.mode),
        )
        .await?;
    Ok(ManualUsageCleanupPreview {
        detail_cutoff: window.detail_cutoff,
        compressed_cutoff: window.compressed_cutoff,
        header_cutoff: window.header_cutoff,
        log_cutoff: window.log_cutoff,
        mode: options.mode,
        targets: options.targets,
        requested_older_than_days: options.requested_older_than_days,
        detail_count: counts.detail,
        compressed_count: counts.compressed,
        header_count: counts.header,
        log_count: counts.log,
    })
}

async fn compute_usage_cleanup_window(
    data: &GatewayDataState,
    mode: ManualUsageCleanupMode,
    override_older_than: Option<chrono::Duration>,
) -> Result<UsageCleanupWindow, DataLayerError> {
    let settings = usage_cleanup_settings(data).await?;
    Ok(usage_cleanup_window_for_mode(
        Utc::now(),
        settings,
        mode,
        override_older_than,
    ))
}

pub(super) fn usage_cleanup_window_for_mode(
    now_utc: chrono::DateTime<Utc>,
    settings: super::UsageCleanupSettings,
    mode: ManualUsageCleanupMode,
    override_older_than: Option<chrono::Duration>,
) -> UsageCleanupWindow {
    match mode {
        ManualUsageCleanupMode::Policy => usage_cleanup_window(now_utc, settings),
        ManualUsageCleanupMode::OlderThanDays => {
            usage_cleanup_window_with_override(now_utc, settings, override_older_than)
        }
        ManualUsageCleanupMode::BeforeNow => UsageCleanupWindow {
            detail_cutoff: now_utc,
            compressed_cutoff: now_utc,
            header_cutoff: now_utc,
            log_cutoff: now_utc,
        },
    }
}

fn cleanup_execution_mode(mode: ManualUsageCleanupMode) -> UsageCleanupExecutionMode {
    match mode {
        ManualUsageCleanupMode::BeforeNow => UsageCleanupExecutionMode::BeforeNowBodyFields,
        ManualUsageCleanupMode::Policy | ManualUsageCleanupMode::OlderThanDays => {
            UsageCleanupExecutionMode::Policy
        }
    }
}
