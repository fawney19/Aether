use async_trait::async_trait;

use crate::repository::billing::MAX_USAGE_POLICY_TOTAL_RULES;

const MAX_USAGE_POLICY_LEDGER_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsagePolicyRequestWindow {
    pub starts_at_unix_secs: u64,
    pub ends_at_unix_secs: u64,
    pub limit_requests: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReserveUsagePolicyRequestInput {
    pub request_id: String,
    pub subject_id: String,
    pub event_token: String,
    pub admitted_at_unix_secs: u64,
    /// Exclusive timestamp after which this admission cannot affect any supplied window and its
    /// idempotency tombstone may be deleted safely.
    pub retain_until_unix_secs: u64,
    pub windows: Vec<UsagePolicyRequestWindow>,
}

impl ReserveUsagePolicyRequestInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_bounded_id(&self.request_id, "usage policy request_id")?;
        validate_bounded_id(&self.subject_id, "usage policy subject_id")?;
        validate_bounded_id(&self.event_token, "usage policy event_token")?;
        validate_database_u64(self.admitted_at_unix_secs, "usage policy admitted_at")?;
        validate_database_u64(self.retain_until_unix_secs, "usage policy retain_until")?;
        if self.windows.is_empty() || self.windows.len() > MAX_USAGE_POLICY_TOTAL_RULES {
            return Err(crate::DataLayerError::InvalidInput(format!(
                "usage policy request admission requires 1 to {MAX_USAGE_POLICY_TOTAL_RULES} windows"
            )));
        }
        for (index, window) in self.windows.iter().enumerate() {
            validate_database_u64(
                window.starts_at_unix_secs,
                &format!("usage policy request window {index} start"),
            )?;
            validate_database_u64(
                window.ends_at_unix_secs,
                &format!("usage policy request window {index} end"),
            )?;
            validate_database_u64(
                window.limit_requests,
                &format!("usage policy request window {index} limit"),
            )?;
            if window.limit_requests == 0
                || window.starts_at_unix_secs >= window.ends_at_unix_secs
                || self.admitted_at_unix_secs < window.starts_at_unix_secs
                || self.admitted_at_unix_secs >= window.ends_at_unix_secs
            {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "usage policy request window {index} does not contain admission or has invalid bounds"
                )));
            }
            if self.retain_until_unix_secs < window.ends_at_unix_secs {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "usage policy request retain_until precedes window {index} end"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicyRequestAdmissionState {
    Active,
    Released,
}

impl UsagePolicyRequestAdmissionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReserveUsagePolicyRequestOutcome {
    Allowed,
    Rejected {
        window_index: usize,
        limit_requests: u64,
        used_requests: u64,
    },
    AlreadyReleased,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseUsagePolicyRequestAdmissionInput {
    pub request_id: String,
    pub subject_id: String,
    pub event_token: String,
    pub released_at_unix_secs: u64,
}

impl ReleaseUsagePolicyRequestAdmissionInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_bounded_id(&self.request_id, "usage policy request_id")?;
        validate_bounded_id(&self.subject_id, "usage policy subject_id")?;
        validate_bounded_id(&self.event_token, "usage policy event_token")?;
        validate_database_u64(self.released_at_unix_secs, "usage policy released_at")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsagePolicyRequestAdmission {
    pub request_id: String,
    pub subject_id: String,
    pub event_token: String,
    pub admitted_at_unix_secs: u64,
    pub retain_until_unix_secs: u64,
    pub state: UsagePolicyRequestAdmissionState,
    pub released_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsagePolicyCostWindow {
    pub window_id: String,
    pub starts_at_unix_secs: u64,
    pub ends_at_unix_secs: u64,
    pub limit_cost_units: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReserveUsagePolicyCostInput {
    pub request_id: String,
    pub subject_id: String,
    pub reservation_token: String,
    pub admitted_at_unix_secs: u64,
    pub reserved_cost_units: u64,
    pub reservation_expires_at_unix_secs: u64,
    /// Exclusive timestamp after which this reservation can no longer affect any future window
    /// and its idempotency tombstone may be deleted safely.
    pub retain_until_unix_secs: u64,
    pub windows: Vec<UsagePolicyCostWindow>,
}

impl ReserveUsagePolicyCostInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_bounded_id(&self.request_id, "usage policy request_id")?;
        validate_bounded_id(&self.subject_id, "usage policy subject_id")?;
        validate_bounded_id(&self.reservation_token, "usage policy reservation_token")?;
        validate_cost_units(self.reserved_cost_units, "reserved_cost_units")?;
        if self.reservation_expires_at_unix_secs <= self.admitted_at_unix_secs {
            return Err(crate::DataLayerError::InvalidInput(
                "usage policy reservation must expire after admission".to_string(),
            ));
        }
        if self.retain_until_unix_secs < self.reservation_expires_at_unix_secs {
            return Err(crate::DataLayerError::InvalidInput(
                "usage policy retain_until must not precede reservation expiry".to_string(),
            ));
        }
        if self.windows.is_empty() || self.windows.len() > MAX_USAGE_POLICY_TOTAL_RULES {
            return Err(crate::DataLayerError::InvalidInput(format!(
                "usage policy reservation requires 1 to {MAX_USAGE_POLICY_TOTAL_RULES} windows"
            )));
        }
        for (index, window) in self.windows.iter().enumerate() {
            validate_non_empty_id(
                &window.window_id,
                &format!("usage policy window {index} id"),
            )?;
            validate_cost_units(
                window.limit_cost_units,
                &format!("usage policy window {index} limit_cost_units"),
            )?;
            if window.limit_cost_units == 0
                || window.starts_at_unix_secs >= window.ends_at_unix_secs
                || self.admitted_at_unix_secs < window.starts_at_unix_secs
                || self.admitted_at_unix_secs >= window.ends_at_unix_secs
            {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "usage policy window {index} does not contain admission or has invalid bounds"
                )));
            }
            if self.windows[..index]
                .iter()
                .any(|previous| previous.window_id == window.window_id)
            {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "usage policy window {index} duplicates window_id {}",
                    window.window_id
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicyCostReservationState {
    Reserved,
    Finalized,
    Released,
}

impl UsagePolicyCostReservationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Finalized => "finalized",
            Self::Released => "released",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "reserved" => Some(Self::Reserved),
            "finalized" => Some(Self::Finalized),
            "released" => Some(Self::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReserveUsagePolicyCostOutcome {
    Allowed {
        reserved_cost_units: u64,
        additional_reserved_cost_units: u64,
    },
    Rejected {
        window_index: usize,
        limit_cost_units: u64,
        used_cost_units: u64,
    },
    AlreadyTerminal {
        state: UsagePolicyCostReservationState,
    },
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconcileUsagePolicyCostInput {
    pub request_id: String,
    pub subject_id: String,
    pub reservation_token: String,
    pub actual_cost_units: u64,
    pub terminal_state: UsagePolicyCostReservationState,
    pub finalized_at_unix_secs: u64,
}

impl ReconcileUsagePolicyCostInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_bounded_id(&self.request_id, "usage policy request_id")?;
        validate_bounded_id(&self.subject_id, "usage policy subject_id")?;
        validate_bounded_id(&self.reservation_token, "usage policy reservation_token")?;
        validate_cost_units(self.actual_cost_units, "actual_cost_units")?;
        match self.terminal_state {
            UsagePolicyCostReservationState::Reserved => Err(crate::DataLayerError::InvalidInput(
                "usage policy reconciliation requires a terminal state".to_string(),
            )),
            UsagePolicyCostReservationState::Released if self.actual_cost_units != 0 => {
                Err(crate::DataLayerError::InvalidInput(
                    "released usage policy reservations must have zero actual cost".to_string(),
                ))
            }
            UsagePolicyCostReservationState::Finalized
            | UsagePolicyCostReservationState::Released => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsagePolicyCostReservation {
    pub request_id: String,
    pub subject_id: String,
    pub reservation_token: String,
    pub admitted_at_unix_secs: u64,
    pub reserved_cost_units: u64,
    pub actual_cost_units: Option<u64>,
    pub state: UsagePolicyCostReservationState,
    pub reservation_expires_at_unix_secs: u64,
    pub retain_until_unix_secs: u64,
    pub finalized_at_unix_secs: Option<u64>,
}

fn validate_non_empty_id(value: &str, field: &str) -> Result<(), crate::DataLayerError> {
    if value.trim().is_empty() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_bounded_id(value: &str, field: &str) -> Result<(), crate::DataLayerError> {
    validate_non_empty_id(value, field)?;
    if value.len() > MAX_USAGE_POLICY_LEDGER_ID_BYTES {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} exceeds {MAX_USAGE_POLICY_LEDGER_ID_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_database_u64(value: u64, field: &str) -> Result<(), crate::DataLayerError> {
    if value > i64::MAX as u64 {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} exceeds the database integer range"
        )));
    }
    Ok(())
}

fn validate_cost_units(value: u64, field: &str) -> Result<(), crate::DataLayerError> {
    if value > i64::MAX as u64 {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} exceeds the database integer range"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageSettlementInput {
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub api_key_is_standalone: bool,
    pub provider_id: Option<String>,
    pub status: String,
    pub billing_status: String,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub finalized_at_unix_secs: Option<u64>,
}

impl UsageSettlementInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.request_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement request_id cannot be empty".to_string(),
            ));
        }
        if self.status.trim().is_empty() || self.billing_status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement status cannot be empty".to_string(),
            ));
        }
        if !self.total_cost_usd.is_finite() || !self.actual_total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement cost must be finite".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageSettlement {
    pub request_id: String,
    pub wallet_id: Option<String>,
    pub billing_status: String,
    pub wallet_balance_before: Option<f64>,
    pub wallet_balance_after: Option<f64>,
    pub wallet_recharge_balance_before: Option<f64>,
    pub wallet_recharge_balance_after: Option<f64>,
    pub wallet_gift_balance_before: Option<f64>,
    pub wallet_gift_balance_after: Option<f64>,
    pub provider_monthly_used_usd: Option<f64>,
    pub finalized_at_unix_secs: Option<u64>,
}

#[async_trait]
pub trait SettlementWriteRepository: Send + Sync {
    async fn reserve_usage_policy_request(
        &self,
        input: ReserveUsagePolicyRequestInput,
    ) -> Result<ReserveUsagePolicyRequestOutcome, crate::DataLayerError>;

    async fn release_usage_policy_request_admission(
        &self,
        input: ReleaseUsagePolicyRequestAdmissionInput,
    ) -> Result<Option<StoredUsagePolicyRequestAdmission>, crate::DataLayerError>;

    async fn cleanup_usage_policy_request_admissions(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, crate::DataLayerError> {
        let _ = (now_unix_secs, batch_size);
        Ok(0)
    }

    async fn reserve_usage_policy_cost(
        &self,
        input: ReserveUsagePolicyCostInput,
    ) -> Result<ReserveUsagePolicyCostOutcome, crate::DataLayerError>;

    async fn reconcile_usage_policy_cost(
        &self,
        input: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, crate::DataLayerError>;

    async fn cleanup_usage_policy_cost_reservations(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, crate::DataLayerError> {
        let _ = (now_unix_secs, batch_size);
        Ok(0)
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, crate::DataLayerError>;
}

pub trait SettlementRepository: SettlementWriteRepository + Send + Sync {}

impl<T> SettlementRepository for T where T: SettlementWriteRepository + Send + Sync {}

pub const SETTLEMENT_EPSILON_USD: f64 = 0.000_000_01;

#[derive(Debug, Clone, Copy)]
pub struct WalletDebitPlan {
    pub recharge_deduction: f64,
    pub gift_deduction: f64,
    pub recharge_overdraft: f64,
}

impl WalletDebitPlan {
    pub fn after_balances(self, recharge_balance: f64, gift_balance: f64) -> (f64, f64) {
        (
            recharge_balance - self.recharge_deduction - self.recharge_overdraft,
            gift_balance - self.gift_deduction,
        )
    }
}

pub fn finite_wallet_available_usd(recharge_balance: f64, gift_balance: f64) -> f64 {
    recharge_balance.max(0.0) + gift_balance.max(0.0)
}

/// Reject corrupted or overflowing financial values before usage settlement mutates a wallet.
/// Recharge balances may legitimately be negative after an admitted request settles, but gift
/// balances and cumulative consumption may not be negative. Derived totals must remain finite so
/// persisted `NaN`/infinity values cannot turn a finite wallet into an implicit unlimited wallet.
pub fn validate_wallet_settlement_values(
    recharge_balance: f64,
    gift_balance: f64,
    total_consumed: f64,
    additional_consumed: f64,
) -> Result<(), crate::DataLayerError> {
    let balance_total = recharge_balance + gift_balance;
    let consumed_after = total_consumed + additional_consumed;
    if !recharge_balance.is_finite()
        || !gift_balance.is_finite()
        || gift_balance < 0.0
        || !balance_total.is_finite()
        || !total_consumed.is_finite()
        || total_consumed < 0.0
        || !additional_consumed.is_finite()
        || additional_consumed < 0.0
        || !consumed_after.is_finite()
    {
        return Err(crate::DataLayerError::UnexpectedValue(
            "wallet financial state is invalid for usage settlement".to_string(),
        ));
    }
    Ok(())
}

pub fn plan_finite_wallet_debit(
    recharge_balance: f64,
    gift_balance: f64,
    requested_usd: f64,
) -> WalletDebitPlan {
    let requested_usd = requested_usd.max(0.0);
    let recharge_deduction = recharge_balance.max(0.0).min(requested_usd);
    let after_recharge_remaining = (requested_usd - recharge_deduction).max(0.0);
    let gift_deduction = gift_balance.max(0.0).min(after_recharge_remaining);
    let recharge_overdraft = (after_recharge_remaining - gift_deduction).max(0.0);
    WalletDebitPlan {
        recharge_deduction,
        gift_deduction,
        recharge_overdraft,
    }
}

pub fn settlement_billing_status_for_usage_status(status: &str) -> &'static str {
    match status {
        "completed" | "cancelled" => "settled",
        _ => "void",
    }
}

pub fn settlement_billable_cost_usd(input: &UsageSettlementInput) -> f64 {
    input.actual_total_cost_usd.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::{
        validate_wallet_settlement_values, ReconcileUsagePolicyCostInput,
        ReserveUsagePolicyCostInput, ReserveUsagePolicyRequestInput,
        UsagePolicyCostReservationState, UsagePolicyCostWindow, UsagePolicyRequestWindow,
        UsageSettlementInput,
    };

    #[test]
    fn rejects_invalid_settlement_input() {
        let input = UsageSettlementInput {
            request_id: "".to_string(),
            user_id: None,
            api_key_id: None,
            api_key_is_standalone: false,
            provider_id: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            total_cost_usd: 0.1,
            actual_total_cost_usd: 0.1,
            finalized_at_unix_secs: None,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn wallet_settlement_values_reject_corruption_and_overflow() {
        assert!(validate_wallet_settlement_values(-3.0, 0.0, 12.0, 1.0).is_ok());
        for (recharge, gift, consumed, additional) in [
            (f64::NAN, 0.0, 0.0, 1.0),
            (f64::INFINITY, 0.0, 0.0, 1.0),
            (0.0, f64::NAN, 0.0, 1.0),
            (0.0, -0.01, 0.0, 1.0),
            (0.0, 0.0, f64::INFINITY, 1.0),
            (0.0, 0.0, -0.01, 1.0),
            (0.0, 0.0, 0.0, f64::NAN),
            (f64::MAX, f64::MAX, 0.0, 1.0),
            (0.0, 0.0, f64::MAX, f64::MAX),
        ] {
            assert!(
                validate_wallet_settlement_values(recharge, gift, consumed, additional).is_err()
            );
        }
    }

    #[test]
    fn validates_request_admission_window_and_retention_bounds() {
        let valid = ReserveUsagePolicyRequestInput {
            request_id: "request-1".to_string(),
            subject_id: "user-1".to_string(),
            event_token: "event-1".to_string(),
            admitted_at_unix_secs: 100,
            retain_until_unix_secs: 200,
            windows: vec![UsagePolicyRequestWindow {
                starts_at_unix_secs: 50,
                ends_at_unix_secs: 200,
                limit_requests: 10,
            }],
        };
        assert!(valid.validate().is_ok());

        let mut invalid_retention = valid.clone();
        invalid_retention.retain_until_unix_secs = 199;
        assert!(invalid_retention.validate().is_err());

        let mut invalid_window = valid;
        invalid_window.windows[0].starts_at_unix_secs = 101;
        assert!(invalid_window.validate().is_err());
    }

    #[test]
    fn usage_policy_cost_ids_match_sql_column_bounds() {
        let bounded = "x".repeat(128);
        let reserve = ReserveUsagePolicyCostInput {
            request_id: bounded.clone(),
            subject_id: bounded.clone(),
            reservation_token: bounded.clone(),
            admitted_at_unix_secs: 100,
            reserved_cost_units: 1,
            reservation_expires_at_unix_secs: 150,
            retain_until_unix_secs: 200,
            windows: vec![UsagePolicyCostWindow {
                window_id: "window-1".to_string(),
                starts_at_unix_secs: 50,
                ends_at_unix_secs: 200,
                limit_cost_units: 10,
            }],
        };
        assert!(reserve.validate().is_ok());

        for field in ["request_id", "subject_id", "reservation_token"] {
            let mut too_long = reserve.clone();
            match field {
                "request_id" => too_long.request_id.push('x'),
                "subject_id" => too_long.subject_id.push('x'),
                "reservation_token" => too_long.reservation_token.push('x'),
                _ => unreachable!(),
            }
            assert!(too_long.validate().is_err(), "{field} must be bounded");
        }

        let reconcile = ReconcileUsagePolicyCostInput {
            request_id: bounded.clone(),
            subject_id: bounded.clone(),
            reservation_token: bounded,
            actual_cost_units: 1,
            terminal_state: UsagePolicyCostReservationState::Finalized,
            finalized_at_unix_secs: 200,
        };
        assert!(reconcile.validate().is_ok());
        for field in ["request_id", "subject_id", "reservation_token"] {
            let mut too_long = reconcile.clone();
            match field {
                "request_id" => too_long.request_id.push('x'),
                "subject_id" => too_long.subject_id.push('x'),
                "reservation_token" => too_long.reservation_token.push('x'),
                _ => unreachable!(),
            }
            assert!(too_long.validate().is_err(), "{field} must be bounded");
        }
    }
}
