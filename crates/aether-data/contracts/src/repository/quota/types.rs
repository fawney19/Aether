use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderQuotaSnapshot {
    pub provider_id: String,
    pub billing_type: String,
    pub monthly_quota_usd: Option<f64>,
    pub monthly_used_usd: f64,
    pub quota_reset_day: Option<u64>,
    pub quota_last_reset_at_unix_secs: Option<u64>,
    pub quota_expires_at_unix_secs: Option<u64>,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderQuotaUsageObservation {
    pub monthly_used_usd: f64,
    pub quota_last_reset_at_unix_secs: Option<u64>,
}

impl From<&StoredProviderQuotaSnapshot> for ProviderQuotaUsageObservation {
    fn from(snapshot: &StoredProviderQuotaSnapshot) -> Self {
        Self {
            monthly_used_usd: snapshot.monthly_used_usd,
            quota_last_reset_at_unix_secs: snapshot.quota_last_reset_at_unix_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyRemoteProviderQuotaPatch {
    pub provider_id: String,
    pub billing_type: String,
    pub monthly_quota_usd: Option<f64>,
    pub remote_monthly_used_usd: f64,
    pub remote_window_start_unix_secs: u64,
    pub remote_window_end_unix_secs: u64,
    pub quota_reset_day: Option<u64>,
    pub quota_expires_at_unix_secs: Option<u64>,
    /// Local usage state read immediately before fetching the authoritative remote
    /// snapshot. Finite remote quota is applied only if this state is unchanged;
    /// otherwise the caller retries from a fresh observation.
    pub local_usage_observation: Option<ProviderQuotaUsageObservation>,
    /// Preserve the provider's current local usage when only quota state changes.
    pub preserve_local_used_usd: bool,
}

impl ApplyRemoteProviderQuotaPatch {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "remote provider quota provider_id is empty".to_string(),
            ));
        }
        if !matches!(
            self.billing_type.as_str(),
            "monthly_quota" | "pay_as_you_go"
        ) {
            return Err(crate::DataLayerError::InvalidInput(
                "remote provider quota billing_type is unsupported".to_string(),
            ));
        }
        if self.billing_type == "monthly_quota" && self.monthly_quota_usd.is_none() {
            return Err(crate::DataLayerError::InvalidInput(
                "remote monthly quota limit is missing".to_string(),
            ));
        }
        if self.billing_type == "pay_as_you_go" && self.monthly_quota_usd.is_some() {
            return Err(crate::DataLayerError::InvalidInput(
                "remote unlimited quota must not set a monthly limit".to_string(),
            ));
        }
        if !self.remote_monthly_used_usd.is_finite()
            || self.remote_monthly_used_usd < 0.0
            || self
                .monthly_quota_usd
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            || self.local_usage_observation.as_ref().is_some_and(|value| {
                !value.monthly_used_usd.is_finite() || value.monthly_used_usd < 0.0
            })
        {
            return Err(crate::DataLayerError::InvalidInput(
                "remote provider quota values must be finite and non-negative".to_string(),
            ));
        }
        if self.preserve_local_used_usd == self.local_usage_observation.is_some() {
            return Err(crate::DataLayerError::InvalidInput(
                if self.preserve_local_used_usd {
                    "state-only remote quota must not include a local usage observation"
                } else {
                    "finite remote quota is missing its local usage observation"
                }
                .to_string(),
            ));
        }
        if self.remote_window_start_unix_secs == 0
            || self.remote_window_end_unix_secs <= self.remote_window_start_unix_secs
        {
            return Err(crate::DataLayerError::InvalidInput(
                "remote provider quota window is invalid".to_string(),
            ));
        }
        if self
            .quota_reset_day
            .is_some_and(|days| !(1..=365).contains(&days))
        {
            return Err(crate::DataLayerError::InvalidInput(
                "remote provider quota reset interval is invalid".to_string(),
            ));
        }
        Ok(())
    }

    pub fn was_applied_after_observation(&self, stored: &StoredProviderQuotaSnapshot) -> bool {
        let Some(observation) = self.local_usage_observation.as_ref() else {
            return false;
        };
        let changed_after_observation = stored.quota_last_reset_at_unix_secs
            != observation.quota_last_reset_at_unix_secs
            || stored.monthly_used_usd != observation.monthly_used_usd;
        changed_after_observation
            && stored.quota_last_reset_at_unix_secs == Some(self.remote_window_start_unix_secs)
            && stored.billing_type == self.billing_type
            && stored.monthly_quota_usd == self.monthly_quota_usd
            && stored.quota_reset_day == self.quota_reset_day
            && stored.quota_expires_at_unix_secs == self.quota_expires_at_unix_secs
            && stored.monthly_used_usd == self.remote_monthly_used_usd
    }

    pub fn usage_changed_after_observation(&self, stored: &StoredProviderQuotaSnapshot) -> bool {
        self.local_usage_observation
            .as_ref()
            .is_some_and(|observation| {
                stored.quota_last_reset_at_unix_secs != observation.quota_last_reset_at_unix_secs
                    || stored.monthly_used_usd != observation.monthly_used_usd
            })
            && !self.was_applied_after_observation(stored)
    }

    pub fn reconciled_monthly_used_usd(&self, stored: &StoredProviderQuotaSnapshot) -> f64 {
        if self.preserve_local_used_usd {
            return stored.monthly_used_usd;
        }

        self.remote_monthly_used_usd
    }

    pub fn apply_to_snapshot(&self, stored: &mut StoredProviderQuotaSnapshot) {
        stored.monthly_used_usd = self.reconciled_monthly_used_usd(stored);
        stored.billing_type.clone_from(&self.billing_type);
        stored.monthly_quota_usd = self.monthly_quota_usd;
        stored.quota_reset_day = self.quota_reset_day;
        stored.quota_last_reset_at_unix_secs = Some(self.remote_window_start_unix_secs);
        stored.quota_expires_at_unix_secs = self.quota_expires_at_unix_secs;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyRemoteProviderQuotaOutcome {
    Applied(StoredProviderQuotaSnapshot),
    StaleWindow(StoredProviderQuotaSnapshot),
    ConcurrentModification(StoredProviderQuotaSnapshot),
    ProviderNotFound,
}

impl ApplyRemoteProviderQuotaOutcome {
    /// Classify a zero-row remote quota UPDATE without mistaking an idempotent
    /// write for success. A row is already applied only when all state fields
    /// match and its usage equals the authoritative remote value.
    pub fn from_unapplied_row(
        stored: Option<StoredProviderQuotaSnapshot>,
        patch: &ApplyRemoteProviderQuotaPatch,
    ) -> Result<Self, crate::DataLayerError> {
        let Some(snapshot) = stored else {
            return Ok(Self::ProviderNotFound);
        };
        if snapshot
            .quota_last_reset_at_unix_secs
            .is_some_and(|start| start >= patch.remote_window_end_unix_secs)
        {
            return Ok(Self::StaleWindow(snapshot));
        }

        let already_applied = snapshot.quota_last_reset_at_unix_secs
            == Some(patch.remote_window_start_unix_secs)
            && snapshot.billing_type == patch.billing_type
            && snapshot.monthly_quota_usd == patch.monthly_quota_usd
            && snapshot.quota_reset_day == patch.quota_reset_day
            && snapshot.quota_expires_at_unix_secs == patch.quota_expires_at_unix_secs
            && (patch.preserve_local_used_usd
                || snapshot.monthly_used_usd == patch.remote_monthly_used_usd);
        if already_applied {
            return Ok(Self::Applied(snapshot));
        }
        Ok(Self::ConcurrentModification(snapshot))
    }
}

impl StoredProviderQuotaSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_id: String,
        billing_type: String,
        monthly_quota_usd: Option<f64>,
        monthly_used_usd: f64,
        quota_reset_day: Option<i32>,
        quota_last_reset_at_unix_secs: Option<i64>,
        quota_expires_at_unix_secs: Option<i64>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() || billing_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider quota identity is empty".to_string(),
            ));
        }
        if !monthly_used_usd.is_finite() || monthly_quota_usd.is_some_and(|v| !v.is_finite()) {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider quota value is not finite".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            billing_type,
            monthly_quota_usd,
            monthly_used_usd,
            quota_reset_day: quota_reset_day.map(|value| value as u64),
            quota_last_reset_at_unix_secs: quota_last_reset_at_unix_secs.map(|value| value as u64),
            quota_expires_at_unix_secs: quota_expires_at_unix_secs.map(|value| value as u64),
            is_active,
        })
    }
}

#[async_trait]
pub trait ProviderQuotaReadRepository: Send + Sync {
    async fn find_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, crate::DataLayerError>;

    async fn find_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderQuotaSnapshot>, crate::DataLayerError>;
}

#[async_trait]
pub trait ProviderQuotaWriteRepository: Send + Sync {
    async fn reset_due(&self, now_unix_secs: u64) -> Result<usize, crate::DataLayerError>;

    async fn apply_remote_provider_quota(
        &self,
        patch: &ApplyRemoteProviderQuotaPatch,
    ) -> Result<ApplyRemoteProviderQuotaOutcome, crate::DataLayerError>;
}

pub trait ProviderQuotaRepository:
    ProviderQuotaReadRepository + ProviderQuotaWriteRepository + Send + Sync
{
}

impl<T> ProviderQuotaRepository for T where
    T: ProviderQuotaReadRepository + ProviderQuotaWriteRepository + Send + Sync
{
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch,
        ProviderQuotaUsageObservation, StoredProviderQuotaSnapshot,
    };

    fn sample_quota(last_reset: Option<i64>) -> StoredProviderQuotaSnapshot {
        StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(100.0),
            4.0,
            Some(30),
            last_reset,
            None,
            true,
        )
        .expect("quota should build")
    }

    fn patch() -> ApplyRemoteProviderQuotaPatch {
        ApplyRemoteProviderQuotaPatch {
            provider_id: "provider-1".to_string(),
            billing_type: "monthly_quota".to_string(),
            monthly_quota_usd: Some(100.0),
            remote_monthly_used_usd: 4.0,
            remote_window_start_unix_secs: 7_000,
            remote_window_end_unix_secs: 8_000,
            quota_reset_day: Some(30),
            quota_expires_at_unix_secs: None,
            local_usage_observation: Some(ProviderQuotaUsageObservation {
                monthly_used_usd: 4.0,
                quota_last_reset_at_unix_secs: Some(7_000),
            }),
            preserve_local_used_usd: false,
        }
    }

    #[test]
    fn reconciliation_uses_remote_authority_after_an_unchanged_observation() {
        let mut stored = sample_quota(Some(7_000));
        stored.monthly_used_usd = 10.0;
        let mut patch = patch();
        patch.remote_monthly_used_usd = 8.0;
        patch.local_usage_observation = Some(ProviderQuotaUsageObservation {
            monthly_used_usd: 10.0,
            quota_last_reset_at_unix_secs: Some(7_000),
        });

        assert_eq!(patch.reconciled_monthly_used_usd(&stored), 8.0);
    }

    #[test]
    fn repeated_remote_patch_is_detected_after_its_observation() {
        let mut stored = sample_quota(Some(1_000));
        stored.monthly_used_usd = 5.0;
        let mut patch = patch();
        patch.remote_window_start_unix_secs = 7_000;
        patch.remote_window_end_unix_secs = 8_000;
        patch.remote_monthly_used_usd = 4.0;
        patch.local_usage_observation = Some(ProviderQuotaUsageObservation {
            monthly_used_usd: 5.0,
            quota_last_reset_at_unix_secs: Some(1_000),
        });

        assert!(!patch.was_applied_after_observation(&stored));
        patch.apply_to_snapshot(&mut stored);
        assert_eq!(stored.monthly_used_usd, 4.0);
        assert!(patch.was_applied_after_observation(&stored));

        patch.remote_monthly_used_usd = 5.0;
        assert!(!patch.was_applied_after_observation(&stored));
        assert!(patch.usage_changed_after_observation(&stored));

        stored.quota_last_reset_at_unix_secs = Some(7_000);
        stored.monthly_used_usd = 6.0;
        patch.local_usage_observation = Some(ProviderQuotaUsageObservation {
            monthly_used_usd: 4.0,
            quota_last_reset_at_unix_secs: Some(7_000),
        });
        assert!(patch.usage_changed_after_observation(&stored));
    }

    #[test]
    fn reconciliation_replaces_historical_local_overestimate() {
        let mut stored = sample_quota(Some(7_000));
        stored.monthly_used_usd = 10.0;
        let mut patch = patch();
        patch.remote_monthly_used_usd = 8.0;
        patch.local_usage_observation = Some(ProviderQuotaUsageObservation {
            monthly_used_usd: 10.0,
            quota_last_reset_at_unix_secs: Some(7_000),
        });

        assert_eq!(patch.reconciled_monthly_used_usd(&stored), 8.0);

        stored.monthly_used_usd = 9.0;
        assert_eq!(patch.reconciled_monthly_used_usd(&stored), 8.0);
    }

    #[test]
    fn finite_and_state_only_patches_require_opposite_observation_modes() {
        let mut finite = patch();
        finite.local_usage_observation = None;
        assert!(finite.validate().is_err());

        let mut state_only = patch();
        state_only.preserve_local_used_usd = true;
        assert!(state_only.validate().is_err());
        state_only.local_usage_observation = None;
        state_only
            .validate()
            .expect("state-only patch should validate");
    }

    #[test]
    fn unapplied_row_classifies_stale_missing_idempotent_and_concurrent() {
        assert!(matches!(
            ApplyRemoteProviderQuotaOutcome::from_unapplied_row(None, &patch())
                .expect("missing provider is an outcome"),
            ApplyRemoteProviderQuotaOutcome::ProviderNotFound
        ));
        assert!(matches!(
            ApplyRemoteProviderQuotaOutcome::from_unapplied_row(
                Some(sample_quota(Some(8_000))),
                &patch(),
            )
            .expect("newer-or-equal local window is stale"),
            ApplyRemoteProviderQuotaOutcome::StaleWindow(_)
        ));
        assert!(matches!(
            ApplyRemoteProviderQuotaOutcome::from_unapplied_row(
                Some(sample_quota(Some(7_000))),
                &patch(),
            )
            .expect("an idempotent row is already applied"),
            ApplyRemoteProviderQuotaOutcome::Applied(_)
        ));
        assert!(matches!(
            ApplyRemoteProviderQuotaOutcome::from_unapplied_row(
                Some(sample_quota(Some(1_000))),
                &patch(),
            )
            .expect("concurrent update is an outcome"),
            ApplyRemoteProviderQuotaOutcome::ConcurrentModification(_)
        ));
    }
}
