use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;

use super::{
    plan_finite_wallet_debit, settlement_billable_cost_usd,
    settlement_billing_status_for_usage_status, validate_wallet_settlement_values,
    ReconcileUsagePolicyCostInput, ReleaseUsagePolicyRequestAdmissionInput,
    ReserveUsagePolicyCostInput, ReserveUsagePolicyCostOutcome, ReserveUsagePolicyRequestInput,
    ReserveUsagePolicyRequestOutcome, SettlementWriteRepository, StoredUsagePolicyCostReservation,
    StoredUsagePolicyRequestAdmission, StoredUsageSettlement, UsagePolicyCostReservationState,
    UsagePolicyRequestAdmissionState, UsageSettlementInput, SETTLEMENT_EPSILON_USD,
};
use crate::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use crate::DataLayerError;

#[derive(Debug)]
enum InMemorySettlementWalletStore {
    Owned(RwLock<BTreeMap<String, StoredWalletSnapshot>>),
    Shared(Arc<InMemoryWalletRepository>),
}

impl Default for InMemorySettlementWalletStore {
    fn default() -> Self {
        Self::Owned(RwLock::new(BTreeMap::new()))
    }
}

impl InMemorySettlementWalletStore {
    fn seeded<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredWalletSnapshot>,
    {
        let mut wallets_by_id = BTreeMap::new();
        for item in items {
            wallets_by_id.insert(item.id.clone(), item);
        }
        Self::Owned(RwLock::new(wallets_by_id))
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut BTreeMap<String, StoredWalletSnapshot>) -> R) -> R {
        match self {
            Self::Owned(wallets_by_id) => {
                let mut wallets = wallets_by_id.write().expect("settlement repo lock");
                f(&mut wallets)
            }
            Self::Shared(repository) => repository.with_wallets_mut(f),
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemorySettlementRepository {
    wallets: InMemorySettlementWalletStore,
    settlement_lock: Mutex<()>,
    provider_monthly_used: RwLock<BTreeMap<String, f64>>,
    settlements: RwLock<BTreeMap<String, StoredUsageSettlement>>,
    cost_reservations: RwLock<BTreeMap<String, StoredUsagePolicyCostReservation>>,
    request_admissions: RwLock<BTreeMap<String, StoredUsagePolicyRequestAdmission>>,
}

impl InMemorySettlementRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredWalletSnapshot>,
    {
        Self {
            wallets: InMemorySettlementWalletStore::seeded(items),
            settlement_lock: Mutex::new(()),
            provider_monthly_used: RwLock::new(BTreeMap::new()),
            settlements: RwLock::new(BTreeMap::new()),
            cost_reservations: RwLock::new(BTreeMap::new()),
            request_admissions: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn from_wallet_repository(wallet_repository: Arc<InMemoryWalletRepository>) -> Self {
        Self {
            wallets: InMemorySettlementWalletStore::Shared(wallet_repository),
            settlement_lock: Mutex::new(()),
            provider_monthly_used: RwLock::new(BTreeMap::new()),
            settlements: RwLock::new(BTreeMap::new()),
            cost_reservations: RwLock::new(BTreeMap::new()),
            request_admissions: RwLock::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl SettlementWriteRepository for InMemorySettlementRepository {
    async fn reserve_usage_policy_request(
        &self,
        input: ReserveUsagePolicyRequestInput,
    ) -> Result<ReserveUsagePolicyRequestOutcome, DataLayerError> {
        input.validate()?;
        let mut admissions = self
            .request_admissions
            .write()
            .expect("usage policy request admission lock");
        if let Some(existing) = admissions.get_mut(&input.event_token) {
            if existing.request_id != input.request_id || existing.subject_id != input.subject_id {
                return Ok(ReserveUsagePolicyRequestOutcome::Conflict);
            }
            if existing.admitted_at_unix_secs != input.admitted_at_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "usage policy event_token must keep its original admitted_at".to_string(),
                ));
            }
            existing.retain_until_unix_secs = existing
                .retain_until_unix_secs
                .max(input.retain_until_unix_secs);
            if existing.state == UsagePolicyRequestAdmissionState::Released {
                return Ok(ReserveUsagePolicyRequestOutcome::AlreadyReleased);
            }
            return Ok(ReserveUsagePolicyRequestOutcome::Allowed);
        }

        for (window_index, window) in input.windows.iter().enumerate() {
            let used_requests = admissions
                .values()
                .filter(|admission| {
                    admission.state == UsagePolicyRequestAdmissionState::Active
                        && admission.subject_id == input.subject_id
                        && admission.admitted_at_unix_secs >= window.starts_at_unix_secs
                        && admission.admitted_at_unix_secs < window.ends_at_unix_secs
                })
                .count() as u64;
            if used_requests >= window.limit_requests {
                return Ok(ReserveUsagePolicyRequestOutcome::Rejected {
                    window_index,
                    limit_requests: window.limit_requests,
                    used_requests,
                });
            }
        }

        admissions.insert(
            input.event_token.clone(),
            StoredUsagePolicyRequestAdmission {
                request_id: input.request_id,
                subject_id: input.subject_id,
                event_token: input.event_token,
                admitted_at_unix_secs: input.admitted_at_unix_secs,
                retain_until_unix_secs: input.retain_until_unix_secs,
                state: UsagePolicyRequestAdmissionState::Active,
                released_at_unix_secs: None,
            },
        );
        Ok(ReserveUsagePolicyRequestOutcome::Allowed)
    }

    async fn release_usage_policy_request_admission(
        &self,
        input: ReleaseUsagePolicyRequestAdmissionInput,
    ) -> Result<Option<StoredUsagePolicyRequestAdmission>, DataLayerError> {
        input.validate()?;
        let mut admissions = self
            .request_admissions
            .write()
            .expect("usage policy request admission lock");
        let Some(admission) = admissions.get_mut(&input.event_token) else {
            return Ok(None);
        };
        if admission.request_id != input.request_id || admission.subject_id != input.subject_id {
            return Ok(None);
        }
        if input.released_at_unix_secs < admission.admitted_at_unix_secs {
            return Err(DataLayerError::InvalidInput(
                "usage policy released_at must not precede admitted_at".to_string(),
            ));
        }
        if admission.state == UsagePolicyRequestAdmissionState::Active {
            admission.state = UsagePolicyRequestAdmissionState::Released;
            admission.released_at_unix_secs = Some(input.released_at_unix_secs);
        }
        Ok(Some(admission.clone()))
    }

    async fn cleanup_usage_policy_request_admissions(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        if batch_size == 0 {
            return Ok(0);
        }
        let mut admissions = self
            .request_admissions
            .write()
            .expect("usage policy request admission lock");
        let tokens = admissions
            .iter()
            .filter(|(_, admission)| admission.retain_until_unix_secs <= now_unix_secs)
            .map(|(token, _)| token.clone())
            .take(batch_size)
            .collect::<Vec<_>>();
        for token in &tokens {
            admissions.remove(token);
        }
        Ok(tokens.len())
    }

    async fn reserve_usage_policy_cost(
        &self,
        input: ReserveUsagePolicyCostInput,
    ) -> Result<ReserveUsagePolicyCostOutcome, DataLayerError> {
        input.validate()?;
        let mut reservations = self
            .cost_reservations
            .write()
            .expect("usage policy cost reservation lock");
        let existing = reservations.get(&input.reservation_token).cloned();
        if let Some(existing) = existing.as_ref() {
            if existing.request_id != input.request_id || existing.subject_id != input.subject_id {
                return Ok(ReserveUsagePolicyCostOutcome::Conflict);
            }
            if existing.state != UsagePolicyCostReservationState::Reserved {
                return Ok(ReserveUsagePolicyCostOutcome::AlreadyTerminal {
                    state: existing.state,
                });
            }
            if existing.admitted_at_unix_secs != input.admitted_at_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "usage policy reservation_token must keep its original admitted_at".to_string(),
                ));
            }
        }

        let target_reserved_cost_units = existing
            .as_ref()
            .map(|reservation| reservation.reserved_cost_units)
            .unwrap_or(0)
            .max(input.reserved_cost_units);
        let target_reservation_expires_at_unix_secs = existing
            .as_ref()
            .map(|reservation| reservation.reservation_expires_at_unix_secs)
            .unwrap_or(0)
            .max(input.reservation_expires_at_unix_secs);
        let target_retain_until_unix_secs = existing
            .as_ref()
            .map(|reservation| reservation.retain_until_unix_secs)
            .unwrap_or(0)
            .max(input.retain_until_unix_secs);
        for (window_index, window) in input.windows.iter().enumerate() {
            let used_cost_units = reservations
                .values()
                .filter(|reservation| {
                    reservation.reservation_token != input.reservation_token
                        && reservation.subject_id == input.subject_id
                        && reservation.admitted_at_unix_secs >= window.starts_at_unix_secs
                        && reservation.admitted_at_unix_secs < window.ends_at_unix_secs
                })
                .try_fold(0_u64, |used, reservation| {
                    let cost_units = match reservation.state {
                        UsagePolicyCostReservationState::Reserved
                            if reservation.reservation_expires_at_unix_secs
                                > input.admitted_at_unix_secs =>
                        {
                            reservation.reserved_cost_units
                        }
                        UsagePolicyCostReservationState::Finalized => {
                            reservation.actual_cost_units.unwrap_or(0)
                        }
                        UsagePolicyCostReservationState::Reserved
                        | UsagePolicyCostReservationState::Released => 0,
                    };
                    used.checked_add(cost_units).ok_or_else(|| {
                        DataLayerError::UnexpectedValue(
                            "usage policy cost total overflowed".to_string(),
                        )
                    })
                })?;
            if used_cost_units
                .checked_add(target_reserved_cost_units)
                .is_none_or(|total| total > window.limit_cost_units)
            {
                return Ok(ReserveUsagePolicyCostOutcome::Rejected {
                    window_index,
                    limit_cost_units: window.limit_cost_units,
                    used_cost_units,
                });
            }
        }

        let previous_reserved_cost_units = existing
            .as_ref()
            .map(|reservation| reservation.reserved_cost_units)
            .unwrap_or(0);
        reservations.insert(
            input.reservation_token.clone(),
            StoredUsagePolicyCostReservation {
                request_id: input.request_id,
                subject_id: input.subject_id,
                reservation_token: input.reservation_token,
                admitted_at_unix_secs: input.admitted_at_unix_secs,
                reserved_cost_units: target_reserved_cost_units,
                actual_cost_units: None,
                state: UsagePolicyCostReservationState::Reserved,
                reservation_expires_at_unix_secs: target_reservation_expires_at_unix_secs,
                retain_until_unix_secs: target_retain_until_unix_secs,
                finalized_at_unix_secs: None,
            },
        );
        Ok(ReserveUsagePolicyCostOutcome::Allowed {
            reserved_cost_units: target_reserved_cost_units,
            additional_reserved_cost_units: target_reserved_cost_units
                .saturating_sub(previous_reserved_cost_units),
        })
    }

    async fn reconcile_usage_policy_cost(
        &self,
        input: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
        input.validate()?;
        let mut reservations = self
            .cost_reservations
            .write()
            .expect("usage policy cost reservation lock");
        let Some(reservation) = reservations.get_mut(&input.reservation_token) else {
            return Ok(None);
        };
        if reservation.request_id != input.request_id || reservation.subject_id != input.subject_id
        {
            // The server-issued token selects the reservation. Never let mismatched audit
            // identity fields mutate a reservation belonging to another request.
            return Ok(None);
        }
        if reservation.state == UsagePolicyCostReservationState::Reserved {
            reservation.state = input.terminal_state;
            reservation.actual_cost_units = Some(input.actual_cost_units);
            reservation.finalized_at_unix_secs = Some(input.finalized_at_unix_secs);
        }
        Ok(Some(reservation.clone()))
    }

    async fn cleanup_usage_policy_cost_reservations(
        &self,
        now_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        if batch_size == 0 {
            return Ok(0);
        }
        let mut reservations = self
            .cost_reservations
            .write()
            .expect("usage policy cost reservation lock");
        let tokens = reservations
            .iter()
            .filter(|(_, reservation)| reservation.retain_until_unix_secs <= now_unix_secs)
            .map(|(token, _)| token.clone())
            .take(batch_size)
            .collect::<Vec<_>>();
        for token in &tokens {
            reservations.remove(token);
        }
        Ok(tokens.len())
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        input.validate()?;
        // Mirror the SQL backends' row-lock transaction: a repeated or concurrent pending
        // finalization for one request must observe the first committed snapshot instead of
        // debiting the wallet twice.
        let _settlement_guard = self
            .settlement_lock
            .lock()
            .expect("usage settlement transaction lock");
        if let Some(existing) = self
            .settlements
            .read()
            .expect("settlement snapshot lock")
            .get(&input.request_id)
            .cloned()
        {
            return Ok(Some(existing));
        }
        if input.billing_status != "pending" {
            return Ok(Some(StoredUsageSettlement {
                request_id: input.request_id,
                wallet_id: None,
                billing_status: input.billing_status,
                wallet_balance_before: None,
                wallet_balance_after: None,
                wallet_recharge_balance_before: None,
                wallet_recharge_balance_after: None,
                wallet_gift_balance_before: None,
                wallet_gift_balance_after: None,
                provider_monthly_used_usd: None,
                finalized_at_unix_secs: input.finalized_at_unix_secs,
            }));
        }

        let mut final_billing_status =
            settlement_billing_status_for_usage_status(&input.status).to_string();
        let billable_cost_usd = settlement_billable_cost_usd(&input);
        let provider_monthly_after = if final_billing_status == "settled" {
            input
                .provider_id
                .as_ref()
                .map(|provider_id| {
                    let quotas = self
                        .provider_monthly_used
                        .read()
                        .expect("provider quota lock");
                    let current = quotas.get(provider_id).copied().unwrap_or(0.0);
                    let next = current + input.actual_total_cost_usd;
                    if !current.is_finite() || current < 0.0 || !next.is_finite() {
                        return Err(DataLayerError::UnexpectedValue(
                            "provider monthly usage is invalid for settlement".to_string(),
                        ));
                    }
                    Ok((provider_id.clone(), next))
                })
                .transpose()?
        } else {
            None
        };
        let mut settlement = self.wallets.with_mut(|wallets| {
            let wallet_id = input
                .api_key_id
                .as_deref()
                .and_then(|api_key_id| {
                    wallets
                        .values()
                        .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id))
                        .map(|wallet| wallet.id.clone())
                })
                .or_else(|| {
                    if input.api_key_is_standalone {
                        return None;
                    }
                    input.user_id.as_deref().and_then(|user_id| {
                        wallets
                            .values()
                            .find(|wallet| wallet.user_id.as_deref() == Some(user_id))
                            .map(|wallet| wallet.id.clone())
                    })
                });
            let wallet = wallet_id
                .as_deref()
                .and_then(|wallet_id| wallets.get_mut(wallet_id));

            let mut settlement = StoredUsageSettlement {
                request_id: input.request_id.clone(),
                wallet_id: None,
                billing_status: final_billing_status.to_string(),
                wallet_balance_before: None,
                wallet_balance_after: None,
                wallet_recharge_balance_before: None,
                wallet_recharge_balance_after: None,
                wallet_gift_balance_before: None,
                wallet_gift_balance_after: None,
                provider_monthly_used_usd: None,
                finalized_at_unix_secs: input.finalized_at_unix_secs,
            };

            if let Some(wallet) = wallet {
                let before_recharge = wallet.balance;
                let before_gift = wallet.gift_balance;
                let consumed_delta = if final_billing_status == "settled" {
                    billable_cost_usd
                } else {
                    0.0
                };
                validate_wallet_settlement_values(
                    before_recharge,
                    before_gift,
                    wallet.total_consumed,
                    consumed_delta,
                )?;
                let before_total = before_recharge + before_gift;
                settlement.wallet_id = Some(wallet.id.clone());
                settlement.wallet_balance_before = Some(before_total);
                settlement.wallet_recharge_balance_before = Some(before_recharge);
                settlement.wallet_gift_balance_before = Some(before_gift);

                if final_billing_status == "settled" {
                    let total_consumed_after = wallet.total_consumed + billable_cost_usd;
                    if wallet.limit_mode.eq_ignore_ascii_case("unlimited") {
                        validate_wallet_settlement_values(
                            before_recharge,
                            before_gift,
                            total_consumed_after,
                            0.0,
                        )?;
                    } else {
                        let debit_plan = plan_finite_wallet_debit(
                            before_recharge,
                            before_gift,
                            billable_cost_usd,
                        );
                        let (after_recharge, after_gift) =
                            debit_plan.after_balances(before_recharge, before_gift);
                        validate_wallet_settlement_values(
                            after_recharge,
                            after_gift,
                            total_consumed_after,
                            0.0,
                        )?;
                        (wallet.balance, wallet.gift_balance) = (after_recharge, after_gift);
                    }
                    wallet.total_consumed = total_consumed_after;
                }

                settlement.wallet_recharge_balance_after = Some(wallet.balance);
                settlement.wallet_gift_balance_after = Some(wallet.gift_balance);
                settlement.wallet_balance_after = Some(wallet.balance + wallet.gift_balance);
            } else if final_billing_status == "settled"
                && billable_cost_usd > SETTLEMENT_EPSILON_USD
            {
                final_billing_status = "insufficient_quota".to_string();
                settlement.billing_status = final_billing_status.clone();
            }

            Ok(settlement)
        })?;

        if final_billing_status == "settled" {
            if let Some((provider_id, next)) = provider_monthly_after {
                let mut quotas = self
                    .provider_monthly_used
                    .write()
                    .expect("provider quota lock");
                quotas.insert(provider_id, next);
                settlement.provider_monthly_used_usd = Some(next);
            }
        }

        self.settlements
            .write()
            .expect("settlement snapshot lock")
            .insert(settlement.request_id.clone(), settlement.clone());

        Ok(Some(settlement))
    }
}

#[cfg(test)]
mod usage_policy_request_admission_tests {
    use super::*;
    use crate::repository::settlement::UsagePolicyRequestWindow;

    fn reserve(
        request_id: &str,
        event_token: &str,
        admitted_at: u64,
        limits: &[(u64, u64, u64)],
    ) -> ReserveUsagePolicyRequestInput {
        let windows = limits
            .iter()
            .map(|(start, end, limit)| UsagePolicyRequestWindow {
                starts_at_unix_secs: *start,
                ends_at_unix_secs: *end,
                limit_requests: *limit,
            })
            .collect::<Vec<_>>();
        ReserveUsagePolicyRequestInput {
            request_id: request_id.to_string(),
            subject_id: "user-1".to_string(),
            event_token: event_token.to_string(),
            admitted_at_unix_secs: admitted_at,
            retain_until_unix_secs: windows
                .iter()
                .map(|window| window.ends_at_unix_secs)
                .max()
                .unwrap_or(admitted_at + 1),
            windows,
        }
    }

    #[tokio::test]
    async fn any_rejected_window_prevents_the_admission_insert() {
        let repository = InMemorySettlementRepository::default();
        repository
            .reserve_usage_policy_request(reserve("request-1", "event-1", 100, &[(0, 1_000, 10)]))
            .await
            .unwrap();

        assert_eq!(
            repository
                .reserve_usage_policy_request(reserve(
                    "request-2",
                    "event-2",
                    101,
                    &[(0, 1_000, 2), (50, 200, 1)],
                ))
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::Rejected {
                window_index: 1,
                limit_requests: 1,
                used_requests: 1,
            }
        );
        assert!(!repository
            .request_admissions
            .read()
            .expect("admission lock")
            .contains_key("event-2"));
    }

    #[tokio::test]
    async fn retries_are_idempotent_and_identity_or_timestamp_conflicts_are_explicit() {
        let repository = InMemorySettlementRepository::default();
        let initial = reserve("request-1", "event-1", 100, &[(0, 1_000, 1)]);
        assert_eq!(
            repository
                .reserve_usage_policy_request(initial.clone())
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::Allowed
        );
        let mut extended = initial.clone();
        extended.retain_until_unix_secs = 2_000;
        extended.windows[0].ends_at_unix_secs = 2_000;
        assert_eq!(
            repository
                .reserve_usage_policy_request(extended)
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::Allowed
        );
        assert_eq!(
            repository
                .request_admissions
                .read()
                .expect("admission lock")
                .get("event-1")
                .expect("admission")
                .retain_until_unix_secs,
            2_000
        );

        let mut conflicting = initial.clone();
        conflicting.request_id = "other-request".to_string();
        assert_eq!(
            repository
                .reserve_usage_policy_request(conflicting)
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::Conflict
        );
        let mut changed_timestamp = initial;
        changed_timestamp.admitted_at_unix_secs = 101;
        assert!(matches!(
            repository
                .reserve_usage_policy_request(changed_timestamp)
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn released_admission_stops_counting_and_never_reactivates() {
        let repository = InMemorySettlementRepository::default();
        let initial = reserve("request-1", "event-1", 100, &[(0, 1_000, 1)]);
        repository
            .reserve_usage_policy_request(initial.clone())
            .await
            .unwrap();
        let released = repository
            .release_usage_policy_request_admission(ReleaseUsagePolicyRequestAdmissionInput {
                request_id: "request-1".to_string(),
                subject_id: "user-1".to_string(),
                event_token: "event-1".to_string(),
                released_at_unix_secs: 101,
            })
            .await
            .unwrap()
            .expect("released admission");
        assert_eq!(released.state, UsagePolicyRequestAdmissionState::Released);

        let released_again = repository
            .release_usage_policy_request_admission(ReleaseUsagePolicyRequestAdmissionInput {
                request_id: "request-1".to_string(),
                subject_id: "user-1".to_string(),
                event_token: "event-1".to_string(),
                released_at_unix_secs: 999,
            })
            .await
            .unwrap()
            .expect("released admission retry");
        assert_eq!(released_again.released_at_unix_secs, Some(101));

        let mut retry = initial;
        retry.windows[0].ends_at_unix_secs = 2_000;
        retry.retain_until_unix_secs = 2_000;
        assert_eq!(
            repository
                .reserve_usage_policy_request(retry)
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::AlreadyReleased
        );
        assert_eq!(
            repository
                .request_admissions
                .read()
                .expect("admission lock")
                .get("event-1")
                .expect("released admission")
                .retain_until_unix_secs,
            2_000
        );
        assert_eq!(
            repository
                .reserve_usage_policy_request(reserve(
                    "request-2",
                    "event-2",
                    102,
                    &[(0, 1_000, 1)],
                ))
                .await
                .unwrap(),
            ReserveUsagePolicyRequestOutcome::Allowed
        );
    }

    #[tokio::test]
    async fn cleanup_is_bounded_and_preserves_unexpired_tombstones() {
        let repository = InMemorySettlementRepository::default();
        repository
            .reserve_usage_policy_request(reserve("request-1", "event-1", 10, &[(0, 100, 10)]))
            .await
            .unwrap();
        repository
            .reserve_usage_policy_request(reserve("request-2", "event-2", 11, &[(0, 200, 10)]))
            .await
            .unwrap();
        assert_eq!(
            repository
                .cleanup_usage_policy_request_admissions(99, 10)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            repository
                .cleanup_usage_policy_request_admissions(200, 1)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            repository
                .cleanup_usage_policy_request_admissions(200, 10)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn concurrent_admissions_do_not_oversell_capacity() {
        let repository = Arc::new(InMemorySettlementRepository::default());
        let mut tasks = Vec::new();
        for index in 0..32 {
            let repository = Arc::clone(&repository);
            tasks.push(tokio::spawn(async move {
                repository
                    .reserve_usage_policy_request(reserve(
                        &format!("request-{index}"),
                        &format!("event-{index}"),
                        100,
                        &[(0, 1_000, 5)],
                    ))
                    .await
                    .unwrap()
            }));
        }
        let mut allowed = 0;
        for task in tasks {
            if task.await.unwrap() == ReserveUsagePolicyRequestOutcome::Allowed {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 5);
        assert_eq!(
            repository
                .request_admissions
                .read()
                .expect("admission lock")
                .len(),
            5
        );
    }
}

#[cfg(test)]
mod usage_policy_cost_tests {
    use super::*;
    use crate::repository::settlement::UsagePolicyCostWindow;

    fn reserve(
        request_id: &str,
        admitted_at: u64,
        reserved: u64,
        limit: u64,
    ) -> ReserveUsagePolicyCostInput {
        ReserveUsagePolicyCostInput {
            request_id: request_id.to_string(),
            subject_id: "user-1".to_string(),
            reservation_token: format!("token-{request_id}"),
            admitted_at_unix_secs: admitted_at,
            reserved_cost_units: reserved,
            reservation_expires_at_unix_secs: admitted_at + 86_400,
            retain_until_unix_secs: admitted_at + 32 * 86_400,
            windows: vec![UsagePolicyCostWindow {
                window_id: "rolling-5h".to_string(),
                starts_at_unix_secs: admitted_at.saturating_sub(18_000),
                ends_at_unix_secs: admitted_at + 1,
                limit_cost_units: limit,
            }],
        }
    }

    #[tokio::test]
    async fn cost_reservations_are_atomic_idempotent_and_reconciled() {
        let repository = InMemorySettlementRepository::default();
        assert_eq!(
            repository
                .reserve_usage_policy_cost(reserve("request-1", 20_000, 60, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed {
                reserved_cost_units: 60,
                additional_reserved_cost_units: 60,
            }
        );
        assert_eq!(
            repository
                .reserve_usage_policy_cost(reserve("request-1", 20_000, 60, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed {
                reserved_cost_units: 60,
                additional_reserved_cost_units: 0,
            }
        );
        assert!(matches!(
            repository
                .reserve_usage_policy_cost(reserve("request-2", 20_001, 50, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Rejected {
                window_index: 0,
                used_cost_units: 60,
                ..
            }
        ));

        repository
            .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
                request_id: "request-1".to_string(),
                subject_id: "user-1".to_string(),
                reservation_token: "token-request-1".to_string(),
                actual_cost_units: 30,
                terminal_state: UsagePolicyCostReservationState::Finalized,
                finalized_at_unix_secs: 20_001,
            })
            .await
            .unwrap();
        assert!(matches!(
            repository
                .reserve_usage_policy_cost(reserve("request-2", 20_002, 50, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed { .. }
        ));
    }

    #[tokio::test]
    async fn same_request_id_can_have_independent_inbound_reservation_tokens() {
        let repository = InMemorySettlementRepository::default();
        repository
            .reserve_usage_policy_cost(reserve("shared-trace", 20_000, 10, 100))
            .await
            .unwrap();

        let mut second = reserve("shared-trace", 20_000, 10, 100);
        second.reservation_token = "another-inbound-request".to_string();
        assert_eq!(
            repository.reserve_usage_policy_cost(second).await.unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed {
                reserved_cost_units: 10,
                additional_reserved_cost_units: 10,
            }
        );

        let finalized = repository
            .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
                request_id: "shared-trace".to_string(),
                subject_id: "user-1".to_string(),
                reservation_token: "another-inbound-request".to_string(),
                actual_cost_units: 8,
                terminal_state: UsagePolicyCostReservationState::Finalized,
                finalized_at_unix_secs: 20_001,
            })
            .await
            .unwrap()
            .expect("second reservation");
        assert_eq!(finalized.reservation_token, "another-inbound-request");
        assert_eq!(finalized.state, UsagePolicyCostReservationState::Finalized);

        // A forged token cannot finalize the first reservation, even though request_id matches.
        assert_eq!(
            repository
                .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
                    request_id: "shared-trace".to_string(),
                    subject_id: "user-1".to_string(),
                    reservation_token: "forged-token".to_string(),
                    actual_cost_units: 0,
                    terminal_state: UsagePolicyCostReservationState::Released,
                    finalized_at_unix_secs: 20_002,
                })
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            repository
                .reserve_usage_policy_cost(reserve("shared-trace", 20_000, 10, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed {
                reserved_cost_units: 10,
                additional_reserved_cost_units: 0,
            }
        );
    }

    #[tokio::test]
    async fn expired_and_released_reservations_stop_consuming_capacity() {
        let repository = InMemorySettlementRepository::default();
        let mut expired = reserve("expired", 10, 100, 100);
        expired.reservation_expires_at_unix_secs = 11;
        expired.retain_until_unix_secs = 100_000;
        repository.reserve_usage_policy_cost(expired).await.unwrap();
        assert!(matches!(
            repository
                .reserve_usage_policy_cost(reserve("after-expiry", 12, 100, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed { .. }
        ));

        repository
            .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
                request_id: "after-expiry".to_string(),
                subject_id: "user-1".to_string(),
                reservation_token: "token-after-expiry".to_string(),
                actual_cost_units: 0,
                terminal_state: UsagePolicyCostReservationState::Released,
                finalized_at_unix_secs: 13,
            })
            .await
            .unwrap();
        assert!(matches!(
            repository
                .reserve_usage_policy_cost(reserve("after-release", 14, 100, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Allowed { .. }
        ));
    }

    #[tokio::test]
    async fn retries_only_extend_expiry_and_retention_and_cleanup_is_bounded() {
        let repository = InMemorySettlementRepository::default();
        let mut initial = reserve("retry", 20_000, 10, 100);
        initial.reservation_expires_at_unix_secs = 30_000;
        initial.retain_until_unix_secs = 50_000;
        repository.reserve_usage_policy_cost(initial).await.unwrap();

        let mut shorter = reserve("retry", 20_000, 10, 100);
        shorter.reservation_expires_at_unix_secs = 25_000;
        shorter.retain_until_unix_secs = 45_000;
        repository.reserve_usage_policy_cost(shorter).await.unwrap();

        let stored = repository
            .cost_reservations
            .read()
            .expect("reservation lock")
            .get("token-retry")
            .cloned()
            .expect("reservation");
        assert_eq!(stored.reservation_expires_at_unix_secs, 30_000);
        assert_eq!(stored.retain_until_unix_secs, 50_000);
        assert_eq!(
            repository
                .cleanup_usage_policy_cost_reservations(49_999, 10)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            repository
                .cleanup_usage_policy_cost_reservations(50_000, 1)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn reconciliation_ignores_subject_or_token_mismatches() {
        let repository = InMemorySettlementRepository::default();
        repository
            .reserve_usage_policy_cost(reserve("protected", 20_000, 10, 100))
            .await
            .unwrap();

        let mismatched = repository
            .reconcile_usage_policy_cost(ReconcileUsagePolicyCostInput {
                request_id: "protected".to_string(),
                subject_id: "user-1".to_string(),
                reservation_token: "forged-token".to_string(),
                actual_cost_units: 0,
                terminal_state: UsagePolicyCostReservationState::Released,
                finalized_at_unix_secs: 20_001,
            })
            .await
            .unwrap();
        assert_eq!(mismatched, None);

        assert!(matches!(
            repository
                .reserve_usage_policy_cost(reserve("after-mismatch", 20_001, 91, 100))
                .await
                .unwrap(),
            ReserveUsagePolicyCostOutcome::Rejected {
                window_index: 0,
                used_cost_units: 10,
                ..
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::InMemorySettlementRepository;
    use crate::repository::settlement::{SettlementWriteRepository, UsageSettlementInput};
    use crate::repository::wallet::StoredWalletSnapshot;
    use std::sync::Arc;

    fn sample_wallet() -> StoredWalletSnapshot {
        StoredWalletSnapshot::new(
            "wallet-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            10.0,
            2.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build")
    }

    fn sample_user_wallet(wallet_id: &str, user_id: &str) -> StoredWalletSnapshot {
        StoredWalletSnapshot::new(
            wallet_id.to_string(),
            Some(user_id.to_string()),
            None,
            10.0,
            2.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build")
    }

    #[tokio::test]
    async fn settles_usage_against_wallet_and_provider_quota() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(6.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(6.0));
    }

    #[tokio::test]
    async fn normal_key_settlement_falls_back_to_user_wallet() {
        let repository =
            InMemorySettlementRepository::seed(vec![sample_user_wallet("wallet-user-1", "user-1")]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-user-wallet".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("normal-key-without-wallet".to_string()),
                api_key_is_standalone: false,
                provider_id: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.wallet_id.as_deref(), Some("wallet-user-1"));
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(6.0));
    }

    #[tokio::test]
    async fn settles_cancelled_usage_against_wallet_and_provider_quota() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-cancelled".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "cancelled".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 6.0,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(6.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(6.0));
    }

    #[tokio::test]
    async fn standalone_key_settlement_never_falls_back_to_owner_wallet() {
        let repository = InMemorySettlementRepository::seed(vec![sample_user_wallet(
            "wallet-admin-owner",
            "admin-owner",
        )]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-standalone-no-key-wallet".to_string(),
                user_id: Some("admin-owner".to_string()),
                api_key_id: Some("standalone-key-without-wallet".to_string()),
                api_key_is_standalone: true,
                provider_id: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 1.5,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.billing_status, "insufficient_quota");
        assert_eq!(settlement.wallet_id, None);
        assert_eq!(settlement.wallet_balance_before, None);
        assert_eq!(settlement.wallet_balance_after, None);
    }

    #[tokio::test]
    async fn finite_wallet_insufficient_balance_overdraws_and_settles() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-insufficient-wallet".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 15.0,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(-3.0));
        assert_eq!(settlement.wallet_recharge_balance_after, Some(-3.0));
        assert_eq!(settlement.wallet_gift_balance_after, Some(0.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(15.0));
    }

    #[tokio::test]
    async fn pending_settlement_replay_is_idempotent_in_memory() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let input = UsageSettlementInput {
            request_id: "req-pending-replay".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            api_key_is_standalone: false,
            provider_id: Some("provider-1".to_string()),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            total_cost_usd: 3.0,
            actual_total_cost_usd: 6.0,
            finalized_at_unix_secs: Some(200),
        };

        let first = repository
            .settle_usage(input.clone())
            .await
            .unwrap()
            .unwrap();
        let replay = repository.settle_usage(input).await.unwrap().unwrap();

        assert_eq!(replay, first);
        assert_eq!(replay.wallet_balance_after, Some(6.0));
        assert_eq!(replay.provider_monthly_used_usd, Some(6.0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_pending_settlement_debits_once_in_memory() {
        let repository = Arc::new(InMemorySettlementRepository::seed(vec![sample_wallet()]));
        let barrier = Arc::new(tokio::sync::Barrier::new(9));
        let input = UsageSettlementInput {
            request_id: "req-concurrent-pending".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            api_key_is_standalone: false,
            provider_id: Some("provider-1".to_string()),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            total_cost_usd: 3.0,
            actual_total_cost_usd: 6.0,
            finalized_at_unix_secs: Some(200),
        };
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            let input = input.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository.settle_usage(input).await.unwrap().unwrap()
            }));
        }
        barrier.wait().await;

        let mut observed = Vec::new();
        for task in tasks {
            observed.push(task.await.unwrap());
        }
        assert!(observed.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(observed[0].wallet_balance_after, Some(6.0));
        assert_eq!(observed[0].provider_monthly_used_usd, Some(6.0));
    }

    #[tokio::test]
    async fn corrupt_wallet_financial_values_fail_before_memory_settlement_mutation() {
        for corrupt in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut wallet = sample_wallet();
            wallet.balance = corrupt;
            let repository = InMemorySettlementRepository::seed(vec![wallet]);
            let result = repository
                .settle_usage(UsageSettlementInput {
                    request_id: "req-corrupt-wallet".to_string(),
                    user_id: Some("user-1".to_string()),
                    api_key_id: Some("key-1".to_string()),
                    api_key_is_standalone: false,
                    provider_id: Some("provider-1".to_string()),
                    status: "completed".to_string(),
                    billing_status: "pending".to_string(),
                    total_cost_usd: 1.0,
                    actual_total_cost_usd: 1.0,
                    finalized_at_unix_secs: Some(200),
                })
                .await;
            assert!(result.is_err());
            assert!(repository
                .settlements
                .read()
                .expect("settlement snapshot lock")
                .is_empty());
            assert!(repository
                .provider_monthly_used
                .read()
                .expect("provider quota lock")
                .is_empty());
        }
    }

    #[tokio::test]
    async fn returns_stored_snapshot_when_usage_is_already_finalized() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settled = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-2".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 2.0,
                actual_total_cost_usd: 1.0,
                finalized_at_unix_secs: Some(250),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        let replay = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-2".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                api_key_is_standalone: false,
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "settled".to_string(),
                total_cost_usd: 2.0,
                actual_total_cost_usd: 1.0,
                finalized_at_unix_secs: Some(250),
            })
            .await
            .expect("replay should succeed")
            .expect("snapshot should exist");

        assert_eq!(replay, settled);
    }
}
