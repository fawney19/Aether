use super::{
    build_admin_users_bad_request_response, build_admin_users_permission_denied_response,
    build_admin_users_read_only_response, build_admin_users_wallet_permission_denied_response,
    disabled_user_policy_detail, disabled_user_policy_field,
    management_token_may_adjust_admin_wallet_balance,
    management_token_may_administer_user_accounts, normalize_admin_user_role,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use aether_data::repository::wallet::{
    AdminUserWalletBalanceBatchUserOutcome, PrepareAdminUserWalletBalanceBatchOutcome,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
struct AdminUserSelectionFilters {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
    #[serde(default)]
    group_id: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct AdminUserSelectionRequest {
    user_ids: Vec<String>,
    group_ids: Vec<String>,
    filters: Option<AdminUserSelectionFilters>,
    filters_scope_present: bool,
}

#[derive(Debug)]
struct AdminUserBatchActionRequest {
    selection: AdminUserSelectionRequest,
    action: String,
    payload: Option<Value>,
    idempotency_key: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RawAdminUserBatchActionRequest {
    selection: Value,
    action: String,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct NormalizedAdminUserSelectionFilters {
    search: Option<String>,
    role: Option<String>,
    is_active: Option<bool>,
    group_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AdminUserSelectionItem {
    user_id: String,
    username: String,
    email: Option<String>,
    role: String,
    is_active: bool,
    matched_by: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
struct AdminUserSelectionWarning {
    #[serde(rename = "type")]
    warning_type: String,
    group_id: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Default)]
struct ResolvedAdminUserSelection {
    items: Vec<AdminUserSelectionItem>,
    missing_user_ids: Vec<String>,
    warnings: Vec<AdminUserSelectionWarning>,
}

#[derive(Debug, Clone, Default)]
struct AdminUserBatchMutation {
    role: Option<String>,
    is_active: Option<bool>,
    unlimited: Option<bool>,
    wallet_balance_adjustment: Option<AdminUserWalletBalanceAdjustment>,
    modified_fields: Vec<&'static str>,
}

impl AdminUserBatchMutation {
    fn has_auth_user_fields(&self) -> bool {
        self.role.is_some() || self.is_active.is_some()
    }
}

#[derive(Debug, Clone, Copy)]
struct AdminUserWalletBalanceAdjustment {
    operation: AdminUserWalletBalanceOperation,
    amount: f64,
}

#[derive(Debug, Clone, Copy)]
enum AdminUserWalletBalanceOperation {
    Add,
    Deduct,
}

enum AdminBatchWalletBalanceAdjustmentError {
    WalletLookup,
    BalanceAdjustment,
}

enum AdminBatchWalletLimitModeError {
    WalletLookup,
    Mutation,
}

pub(in super::super) async fn build_admin_resolve_user_selection_response(
    state: &AdminAppState<'_>,
    _request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let selection = match parse_resolve_selection_request(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
    };
    let resolved = match resolve_admin_user_selection(state, selection).await {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
    };

    Ok(Json(json!({
        "total": resolved.items.len(),
        "items": resolved.items,
        "warnings": resolved.warnings,
    }))
    .into_response())
}

pub(in super::super) async fn build_admin_user_batch_action_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let request = match parse_batch_action_request(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
    };
    let mutation = match parse_batch_mutation(&request.action, request.payload.clone()) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
    };
    if mutation.wallet_balance_adjustment.is_some() {
        if !management_token_may_adjust_admin_wallet_balance(request_context) {
            return Ok(build_admin_users_wallet_permission_denied_response(
                request_context,
            ));
        }
        if !state.has_auth_wallet_write_capability() {
            return Ok(build_admin_users_read_only_response(
                "当前为只读模式，无法批量调整用户钱包余额",
            ));
        }
        return build_admin_user_wallet_balance_batch_response(
            state,
            request_context,
            request,
            mutation,
        )
        .await;
    }
    let resolved = match resolve_admin_user_selection(state, request.selection).await {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
    };
    let mutates_admin_account = mutation
        .role
        .as_deref()
        .is_some_and(crate::roles::can_access_admin_console)
        || (mutation.has_auth_user_fields()
            && resolved
                .items
                .iter()
                .any(|item| crate::roles::can_access_admin_console(&item.role)));
    if mutates_admin_account && !management_token_may_administer_user_accounts(request_context) {
        return Ok(build_admin_users_permission_denied_response(
            request_context,
        ));
    }
    if mutation.has_auth_user_fields() && !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法批量更新用户",
        ));
    }
    if mutation.unlimited.is_some() && !state.has_auth_wallet_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法批量更新用户钱包",
        ));
    }
    let active_admin_demotions = count_active_admin_demotions(&mutation, &resolved.items);
    let active_admin_count = if active_admin_demotions > 0 {
        state.count_active_admin_users().await?
    } else {
        0
    };
    let current_admin_user_id = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.as_str());

    let mut success = 0usize;
    let mut failures = resolved
        .missing_user_ids
        .iter()
        .map(|user_id| json!({ "user_id": user_id, "reason": "用户不存在或已删除" }))
        .collect::<Vec<_>>();
    let mut completed_user_ids = Vec::new();
    let mut uncertain_user_ids = Vec::new();
    let mut unprocessed_user_ids = Vec::new();
    let mut interrupted = false;

    for (item_index, item) in resolved.items.iter().enumerate() {
        let user = match state.find_user_auth_by_id(&item.user_id).await {
            Ok(user) => user,
            Err(_) => {
                record_batch_action_interruption(
                    &resolved.items,
                    item_index,
                    false,
                    "读取用户状态失败，批次已中止，该用户未执行",
                    &mut failures,
                    &mut uncertain_user_ids,
                    &mut unprocessed_user_ids,
                );
                interrupted = true;
                break;
            }
        };
        if user.is_none() {
            failures.push(json!({
                "user_id": item.user_id,
                "reason": "用户不存在或已删除",
            }));
            continue;
        }

        if let Some(reason) = batch_role_demotion_failure_reason(
            &mutation,
            item,
            active_admin_count,
            active_admin_demotions,
            current_admin_user_id,
        ) {
            failures.push(json!({
                "user_id": item.user_id,
                "reason": reason,
            }));
            continue;
        }

        if let Some(unlimited) = mutation.unlimited {
            match apply_batch_user_wallet_limit_mode(state, &item.user_id, unlimited).await {
                Ok(true) => {}
                Ok(false) => {
                    failures.push(json!({
                        "user_id": item.user_id,
                        "reason": "用户钱包不可用",
                    }));
                    continue;
                }
                Err(AdminBatchWalletLimitModeError::WalletLookup) => {
                    record_batch_action_interruption(
                        &resolved.items,
                        item_index,
                        false,
                        "读取用户钱包失败，批次已中止，该用户未执行",
                        &mut failures,
                        &mut uncertain_user_ids,
                        &mut unprocessed_user_ids,
                    );
                    interrupted = true;
                    break;
                }
                Err(AdminBatchWalletLimitModeError::Mutation) => {
                    record_batch_action_interruption(
                        &resolved.items,
                        item_index,
                        true,
                        "用户钱包更新结果未确认，批次已中止，请核对钱包后再重试",
                        &mut failures,
                        &mut uncertain_user_ids,
                        &mut unprocessed_user_ids,
                    );
                    interrupted = true;
                    break;
                }
            }
        }

        if let Some(adjustment) = mutation.wallet_balance_adjustment {
            match apply_batch_user_wallet_balance_adjustment(
                state,
                &item.user_id,
                adjustment,
                current_admin_user_id,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    failures.push(json!({
                        "user_id": item.user_id,
                        "reason": "用户钱包不可用",
                    }));
                    continue;
                }
                Err(AdminBatchWalletBalanceAdjustmentError::WalletLookup) => {
                    record_batch_action_interruption(
                        &resolved.items,
                        item_index,
                        false,
                        "读取用户钱包失败，批次已中止，该用户未执行",
                        &mut failures,
                        &mut uncertain_user_ids,
                        &mut unprocessed_user_ids,
                    );
                    interrupted = true;
                    break;
                }
                Err(AdminBatchWalletBalanceAdjustmentError::BalanceAdjustment) => {
                    record_batch_action_interruption(
                        &resolved.items,
                        item_index,
                        true,
                        "余额调整结果未确认，批次已中止，请核对钱包后再重试",
                        &mut failures,
                        &mut uncertain_user_ids,
                        &mut unprocessed_user_ids,
                    );
                    interrupted = true;
                    break;
                }
            }
        }

        if mutation.has_auth_user_fields() {
            let updated_user = match state
                .update_local_auth_user_admin_fields(
                    &item.user_id,
                    mutation.role.clone(),
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    mutation.is_active,
                )
                .await
            {
                Ok(user) => user,
                Err(_) => {
                    record_batch_action_interruption(
                        &resolved.items,
                        item_index,
                        true,
                        "用户更新结果未确认，批次已中止，请核对后再重试",
                        &mut failures,
                        &mut uncertain_user_ids,
                        &mut unprocessed_user_ids,
                    );
                    interrupted = true;
                    break;
                }
            };
            if updated_user.is_none() {
                failures.push(json!({
                    "user_id": item.user_id,
                    "reason": "用户不存在或已删除",
                }));
                continue;
            }
        }

        success += 1;
        completed_user_ids.push(item.user_id.clone());
    }

    let failed = failures.len();
    let total = success + failed;
    let mut response_payload = json!({
        "total": total,
        "success": success,
        "failed": failed,
        "failures": failures,
        "warnings": resolved.warnings,
        "action": request.action.trim().to_ascii_lowercase(),
        "modified_fields": mutation.modified_fields,
        "interrupted": interrupted,
    });
    if interrupted {
        response_payload["completed_user_ids"] = json!(completed_user_ids);
        response_payload["uncertain_user_ids"] = json!(uncertain_user_ids);
        response_payload["unprocessed_user_ids"] = json!(unprocessed_user_ids);
    }
    let response = Json(response_payload).into_response();

    Ok(attach_admin_audit_response(
        response,
        "admin_users_batch_action_executed",
        "batch_update_users",
        "user_batch",
        "users",
    ))
}

fn record_batch_action_interruption(
    items: &[AdminUserSelectionItem],
    item_index: usize,
    current_result_uncertain: bool,
    reason: &str,
    failures: &mut Vec<Value>,
    uncertain_user_ids: &mut Vec<String>,
    unprocessed_user_ids: &mut Vec<String>,
) {
    let current_item = &items[item_index];
    failures.push(json!({
        "user_id": current_item.user_id,
        "reason": reason,
    }));
    if current_result_uncertain {
        uncertain_user_ids.push(current_item.user_id.clone());
    } else {
        unprocessed_user_ids.push(current_item.user_id.clone());
    }

    for item in items.iter().skip(item_index + 1) {
        failures.push(json!({
            "user_id": item.user_id,
            "reason": "因前序错误未执行",
        }));
        unprocessed_user_ids.push(item.user_id.clone());
    }
}

async fn build_admin_user_wallet_balance_batch_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request: AdminUserBatchActionRequest,
    mutation: AdminUserBatchMutation,
) -> Result<Response<Body>, GatewayError> {
    let Some(idempotency_key) = request
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        })
    else {
        return Ok(build_admin_user_batch_bad_request_response(
            "余额批量操作必须提供有效的 idempotency_key".to_string(),
        ));
    };
    let Some(admin_user_id) = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
    else {
        return Ok(build_admin_users_permission_denied_response(
            request_context,
        ));
    };
    let action = request.action.trim().to_ascii_lowercase();
    let fingerprint_payload = json!({
        "selection": &request.selection,
        "action": &action,
        "payload": &request.payload,
    });
    let encoded = serde_json::to_vec(&fingerprint_payload)
        .map_err(|error| GatewayError::Internal(error.to_string()))?;
    let request_fingerprint = format!("{:x}", Sha256::digest(encoded));

    let existing = state
        .get_admin_user_wallet_balance_batch(&admin_user_id, idempotency_key, &request_fingerprint)
        .await?;
    let batch = match existing {
        Some(PrepareAdminUserWalletBalanceBatchOutcome::Conflict) => {
            return Ok(build_admin_user_batch_idempotency_conflict_response());
        }
        Some(PrepareAdminUserWalletBalanceBatchOutcome::Ready(batch)) => batch,
        None => {
            let resolved =
                match resolve_admin_user_selection(state, request.selection.clone()).await {
                    Ok(value) => value,
                    Err(detail) => return Ok(build_admin_user_batch_bad_request_response(detail)),
                };
            let warnings = serde_json::to_value(&resolved.warnings)
                .ok()
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default();
            let prepared = state
                .prepare_admin_user_wallet_balance_batch(
                    aether_data::repository::wallet::PrepareAdminUserWalletBalanceBatchInput {
                        admin_user_id: admin_user_id.clone(),
                        idempotency_key: idempotency_key.to_string(),
                        request_fingerprint: request_fingerprint.clone(),
                        target_user_ids: resolved
                            .items
                            .iter()
                            .map(|item| item.user_id.clone())
                            .collect(),
                        missing_user_ids: resolved.missing_user_ids,
                        warnings,
                    },
                )
                .await?;
            match prepared {
                PrepareAdminUserWalletBalanceBatchOutcome::Conflict => {
                    return Ok(build_admin_user_batch_idempotency_conflict_response());
                }
                PrepareAdminUserWalletBalanceBatchOutcome::Ready(batch) => batch,
            }
        }
    };
    let warnings: Vec<AdminUserSelectionWarning> =
        serde_json::from_value(Value::Array(batch.warnings.clone()))
            .map_err(|error| GatewayError::Internal(error.to_string()))?;
    let resolved = ResolvedAdminUserSelection {
        items: batch
            .target_user_ids
            .iter()
            .map(|user_id| AdminUserSelectionItem {
                user_id: user_id.clone(),
                username: String::new(),
                email: None,
                role: "user".to_string(),
                is_active: true,
                matched_by: Vec::new(),
            })
            .collect(),
        missing_user_ids: batch.missing_user_ids.clone(),
        warnings,
    };
    let adjustment = mutation
        .wallet_balance_adjustment
        .expect("wallet balance action should have an adjustment");
    let signed_amount = match adjustment.operation {
        AdminUserWalletBalanceOperation::Add => adjustment.amount,
        AdminUserWalletBalanceOperation::Deduct => -adjustment.amount,
    };

    let mut outcomes = batch.user_outcomes;
    let mut completed_user_ids = outcomes
        .iter()
        .filter_map(|(user_id, outcome)| {
            matches!(outcome, AdminUserWalletBalanceBatchUserOutcome::Succeeded)
                .then_some(user_id.clone())
        })
        .collect::<Vec<_>>();
    let mut success = completed_user_ids.len();
    let mut failures = resolved
        .missing_user_ids
        .iter()
        .map(|user_id| json!({ "user_id": user_id, "reason": "用户不存在或已删除" }))
        .collect::<Vec<_>>();
    for (user_id, outcome) in &outcomes {
        if let AdminUserWalletBalanceBatchUserOutcome::Failed(reason) = outcome {
            failures.push(json!({ "user_id": user_id, "reason": reason }));
        }
    }
    let mut uncertain_user_ids = Vec::new();
    let mut unprocessed_user_ids = Vec::new();
    let mut interrupted = false;

    for (item_index, item) in resolved.items.iter().enumerate() {
        if outcomes.contains_key(&item.user_id) {
            continue;
        }
        match state.find_user_auth_by_id(&item.user_id).await {
            Err(_) => {
                failures.push(json!({
                    "user_id": item.user_id,
                    "reason": "读取用户状态失败，批次已中止，该用户未执行",
                }));
                unprocessed_user_ids.push(item.user_id.clone());
                interrupted = true;
            }
            Ok(None) => {
                let outcome = match state
                    .record_admin_user_wallet_balance_batch_failure(
                        &admin_user_id,
                        idempotency_key,
                        &item.user_id,
                        "用户不存在或已删除",
                    )
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        failures.push(json!({
                            "user_id": item.user_id,
                            "reason": "记录用户状态失败，批次已中止，该用户未执行",
                        }));
                        unprocessed_user_ids.push(item.user_id.clone());
                        interrupted = true;
                        append_wallet_batch_unprocessed_suffix(
                            &resolved.items,
                            item_index + 1,
                            &outcomes,
                            &mut failures,
                            &mut unprocessed_user_ids,
                        );
                        break;
                    }
                };
                outcomes.insert(item.user_id.clone(), outcome.clone());
                match outcome {
                    AdminUserWalletBalanceBatchUserOutcome::Succeeded => {
                        completed_user_ids.push(item.user_id.clone());
                        success += 1;
                    }
                    AdminUserWalletBalanceBatchUserOutcome::Failed(reason) => {
                        failures.push(json!({ "user_id": item.user_id, "reason": reason }));
                    }
                }
            }
            Ok(Some(_)) => {
                let wallet = match state
                    .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
                        &item.user_id,
                    ))
                    .await
                {
                    Ok(Some(wallet)) => wallet,
                    Ok(None) => {
                        let outcome = match state
                            .record_admin_user_wallet_balance_batch_failure(
                                &admin_user_id,
                                idempotency_key,
                                &item.user_id,
                                "用户钱包不可用",
                            )
                            .await
                        {
                            Ok(outcome) => outcome,
                            Err(_) => {
                                failures.push(json!({
                                    "user_id": item.user_id,
                                    "reason": "记录用户钱包状态失败，批次已中止，该用户未执行",
                                }));
                                unprocessed_user_ids.push(item.user_id.clone());
                                interrupted = true;
                                append_wallet_batch_unprocessed_suffix(
                                    &resolved.items,
                                    item_index + 1,
                                    &outcomes,
                                    &mut failures,
                                    &mut unprocessed_user_ids,
                                );
                                break;
                            }
                        };
                        outcomes.insert(item.user_id.clone(), outcome.clone());
                        match outcome {
                            AdminUserWalletBalanceBatchUserOutcome::Succeeded => {
                                completed_user_ids.push(item.user_id.clone());
                                success += 1;
                            }
                            AdminUserWalletBalanceBatchUserOutcome::Failed(reason) => {
                                failures.push(json!({ "user_id": item.user_id, "reason": reason }));
                            }
                        }
                        continue;
                    }
                    Err(_) => {
                        failures.push(json!({
                            "user_id": item.user_id,
                            "reason": "读取用户钱包失败，批次已中止，该用户未执行",
                        }));
                        unprocessed_user_ids.push(item.user_id.clone());
                        interrupted = true;
                        append_wallet_batch_unprocessed_suffix(
                            &resolved.items,
                            item_index + 1,
                            &outcomes,
                            &mut failures,
                            &mut unprocessed_user_ids,
                        );
                        break;
                    }
                };
                let result = state
                    .adjust_admin_user_wallet_balance_batch_user(
                        aether_data::repository::wallet::AdjustWalletBalanceInBatchInput {
                            admin_user_id: admin_user_id.clone(),
                            idempotency_key: idempotency_key.to_string(),
                            user_id: item.user_id.clone(),
                            adjustment: aether_data::repository::wallet::AdjustWalletBalanceInput {
                                wallet_id: wallet.id,
                                amount_usd: signed_amount,
                                balance_type: "recharge".to_string(),
                                operator_id: Some(admin_user_id.clone()),
                                description: Some("管理员批量调整用户余额".to_string()),
                                clamp_deduction_to_available_balance: true,
                                batch_context: None,
                            },
                        },
                    )
                    .await;
                match result {
                    Ok(AdminUserWalletBalanceBatchUserOutcome::Succeeded) => {
                        outcomes.insert(
                            item.user_id.clone(),
                            AdminUserWalletBalanceBatchUserOutcome::Succeeded,
                        );
                        completed_user_ids.push(item.user_id.clone());
                        success += 1;
                    }
                    Ok(AdminUserWalletBalanceBatchUserOutcome::Failed(reason)) => {
                        outcomes.insert(
                            item.user_id.clone(),
                            AdminUserWalletBalanceBatchUserOutcome::Failed(reason.clone()),
                        );
                        failures.push(json!({ "user_id": item.user_id, "reason": reason }));
                    }
                    Err(_) => {
                        failures.push(json!({
                            "user_id": item.user_id,
                            "reason": "余额调整结果未确认，批次已中止，请使用同一批次重试以核对结果",
                        }));
                        uncertain_user_ids.push(item.user_id.clone());
                        interrupted = true;
                        append_wallet_batch_unprocessed_suffix(
                            &resolved.items,
                            item_index + 1,
                            &outcomes,
                            &mut failures,
                            &mut unprocessed_user_ids,
                        );
                        break;
                    }
                }
            }
        }
        if interrupted {
            for pending in resolved.items.iter().skip(item_index + 1) {
                if outcomes.contains_key(&pending.user_id) {
                    continue;
                }
                failures.push(json!({
                    "user_id": pending.user_id,
                    "reason": "因前序错误未执行",
                }));
                unprocessed_user_ids.push(pending.user_id.clone());
            }
            break;
        }
    }

    let failed = failures.len();
    let total = success + failed;
    let mut response_payload = json!({
        "total": total,
        "success": success,
        "failed": failed,
        "failures": failures,
        "warnings": resolved.warnings,
        "action": action,
        "modified_fields": mutation.modified_fields,
        "interrupted": interrupted,
    });
    if interrupted {
        response_payload["completed_user_ids"] = json!(completed_user_ids);
        response_payload["uncertain_user_ids"] = json!(uncertain_user_ids);
        response_payload["unprocessed_user_ids"] = json!(unprocessed_user_ids);
    }
    let response = Json(response_payload).into_response();
    Ok(attach_admin_audit_response(
        response,
        "admin_users_batch_action_executed",
        "batch_update_users",
        "user_batch",
        "users",
    ))
}

fn append_wallet_batch_unprocessed_suffix(
    items: &[AdminUserSelectionItem],
    start_index: usize,
    outcomes: &BTreeMap<String, AdminUserWalletBalanceBatchUserOutcome>,
    failures: &mut Vec<Value>,
    unprocessed_user_ids: &mut Vec<String>,
) {
    for pending in items.iter().skip(start_index) {
        if outcomes.contains_key(&pending.user_id) {
            continue;
        }
        failures.push(json!({
            "user_id": pending.user_id,
            "reason": "因前序错误未执行",
        }));
        unprocessed_user_ids.push(pending.user_id.clone());
    }
}

fn build_admin_user_batch_idempotency_conflict_response() -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({
            "detail": "idempotency_key was already used with a different request",
            "error_code": "idempotency_key_conflict",
        })),
    )
        .into_response()
}

fn parse_resolve_selection_request(
    request_body: Option<&Bytes>,
) -> Result<AdminUserSelectionRequest, String> {
    match request_body {
        None => Ok(AdminUserSelectionRequest::default()),
        Some(body) if body.is_empty() => Ok(AdminUserSelectionRequest::default()),
        Some(body) => {
            let value = serde_json::from_slice::<Value>(body)
                .map_err(|_| "Invalid JSON request body".to_string())?;
            parse_selection_request_value(value)
        }
    }
}

fn parse_batch_action_request(
    request_body: Option<&Bytes>,
) -> Result<AdminUserBatchActionRequest, String> {
    match request_body {
        Some(body) if !body.is_empty() => {
            let raw = serde_json::from_slice::<RawAdminUserBatchActionRequest>(body)
                .map_err(|_| "Invalid JSON request body".to_string())?;
            Ok(AdminUserBatchActionRequest {
                selection: parse_selection_request_value(raw.selection)?,
                action: raw.action,
                payload: raw.payload,
                idempotency_key: raw.idempotency_key,
            })
        }
        _ => Err("Invalid JSON request body".to_string()),
    }
}

fn parse_selection_request_value(value: Value) -> Result<AdminUserSelectionRequest, String> {
    let Value::Object(map) = value else {
        return Err("selection 必须是对象".to_string());
    };

    let user_ids = match map.get("user_ids") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
            .map_err(|_| "user_ids 必须是字符串数组".to_string())?,
    };
    let group_ids = match map.get("group_ids") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone())
            .map_err(|_| "group_ids 必须是字符串数组".to_string())?,
    };

    let (filters_scope_present, filters) = match map.get("filters") {
        Some(Value::Object(_)) => {
            let filters = serde_json::from_value::<AdminUserSelectionFilters>(
                map.get("filters").cloned().unwrap_or(Value::Null),
            )
            .map_err(|_| "filters 参数不合法".to_string())?;
            (true, Some(filters))
        }
        None | Some(Value::Null) => (false, None),
        Some(_) => return Err("filters 必须是对象".to_string()),
    };

    Ok(AdminUserSelectionRequest {
        user_ids,
        group_ids,
        filters,
        filters_scope_present,
    })
}

async fn resolve_admin_user_selection(
    state: &AdminAppState<'_>,
    selection: AdminUserSelectionRequest,
) -> Result<ResolvedAdminUserSelection, String> {
    let filters = normalize_selection_filters(selection.filters)?;
    let explicit_user_ids = normalize_user_ids(selection.user_ids);
    let explicit_group_ids = normalize_user_ids(selection.group_ids);
    if explicit_user_ids.is_empty()
        && explicit_group_ids.is_empty()
        && !selection.filters_scope_present
    {
        return Err("至少需要选择一个用户、用户组或明确提供筛选条件".to_string());
    }
    let should_resolve_filters = selection.filters_scope_present;
    let mut items_by_id = BTreeMap::new();
    let mut missing_user_ids = Vec::new();
    let mut warnings = Vec::new();

    if !explicit_group_ids.is_empty() {
        let groups = state
            .list_user_groups_by_ids(&explicit_group_ids)
            .await
            .map_err(|_| "用户分组数据不可用".to_string())?;
        let found_group_ids = groups
            .iter()
            .map(|group| group.id.clone())
            .collect::<BTreeSet<_>>();
        let missing_group_ids = explicit_group_ids
            .iter()
            .filter(|group_id| !found_group_ids.contains(*group_id))
            .cloned()
            .collect::<Vec<_>>();
        if !missing_group_ids.is_empty() {
            return Err(format!("用户分组不存在: {}", missing_group_ids.join(", ")));
        }
    }

    if !explicit_user_ids.is_empty() {
        let users = state
            .resolve_auth_user_summaries_by_ids(&explicit_user_ids)
            .await
            .map_err(|_| "用户数据不可用".to_string())?;
        for user_id in explicit_user_ids {
            match users.get(&user_id).filter(|user| !user.is_deleted) {
                Some(user) => {
                    insert_or_update_selection_item(
                        &mut items_by_id,
                        user.id.clone(),
                        user.username.clone(),
                        user.email.clone(),
                        user.role.clone(),
                        user.is_active,
                        "direct".to_string(),
                    );
                }
                None => missing_user_ids.push(user_id),
            }
        }
    }

    for group_id in &explicit_group_ids {
        let members = state
            .list_user_group_members(group_id)
            .await
            .map_err(|_| "用户分组成员数据不可用".to_string())?;
        let mut matched_count = 0usize;
        for member in members.into_iter().filter(|member| !member.is_deleted) {
            matched_count += 1;
            insert_or_update_selection_item(
                &mut items_by_id,
                member.user_id,
                member.username,
                member.email,
                member.role,
                member.is_active,
                format!("group:{group_id}"),
            );
        }
        if matched_count == 0 {
            warnings.push(AdminUserSelectionWarning {
                warning_type: "empty_group".to_string(),
                group_id: Some(group_id.clone()),
                message: "分组内没有可操作用户".to_string(),
            });
        }
    }

    if should_resolve_filters {
        let users = if filters.as_ref().is_some_and(|filters| {
            filters.search.is_some()
                || filters.role.is_some()
                || filters.is_active.is_some()
                || filters.group_id.is_some()
        }) {
            state
                .list_export_users_page(&aether_data::repository::users::UserExportListQuery {
                    skip: 0,
                    limit: 100_000,
                    role: filters.as_ref().and_then(|filters| filters.role.clone()),
                    is_active: filters.as_ref().and_then(|filters| filters.is_active),
                    search: filters.as_ref().and_then(|filters| filters.search.clone()),
                    group_id: filters
                        .as_ref()
                        .and_then(|filters| filters.group_id.clone()),
                    ..Default::default()
                })
                .await
                .map_err(|_| "用户数据不可用".to_string())?
        } else {
            state
                .list_export_users()
                .await
                .map_err(|_| "用户数据不可用".to_string())?
        };
        for user in users
            .into_iter()
            .filter(|user| admin_user_matches_filters(user, filters.as_ref()))
        {
            insert_or_update_selection_item(
                &mut items_by_id,
                user.id,
                user.username,
                user.email,
                user.role,
                user.is_active,
                "filter".to_string(),
            );
        }
    }

    let mut items = items_by_id.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.username
            .to_ascii_lowercase()
            .cmp(&right.username.to_ascii_lowercase())
            .then_with(|| left.user_id.cmp(&right.user_id))
    });

    Ok(ResolvedAdminUserSelection {
        items,
        missing_user_ids,
        warnings,
    })
}

fn insert_or_update_selection_item(
    items_by_id: &mut BTreeMap<String, AdminUserSelectionItem>,
    user_id: String,
    username: String,
    email: Option<String>,
    role: String,
    is_active: bool,
    matched_by: String,
) {
    match items_by_id.get_mut(&user_id) {
        Some(item) => {
            if !item.matched_by.iter().any(|value| value == &matched_by) {
                item.matched_by.push(matched_by);
            }
        }
        None => {
            items_by_id.insert(
                user_id.clone(),
                AdminUserSelectionItem {
                    user_id,
                    username,
                    email,
                    role,
                    is_active,
                    matched_by: vec![matched_by],
                },
            );
        }
    }
}

fn normalize_selection_filters(
    filters: Option<AdminUserSelectionFilters>,
) -> Result<Option<NormalizedAdminUserSelectionFilters>, String> {
    let Some(filters) = filters else {
        return Ok(None);
    };
    let search = filters
        .search
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let role = match filters
        .role
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && value != "all")
    {
        Some(role) if crate::roles::normalize_assignable_user_role(&role).is_some() => Some(role),
        Some(_) => return Err("role 参数不合法".to_string()),
        None => None,
    };

    Ok(Some(NormalizedAdminUserSelectionFilters {
        search,
        role,
        is_active: filters.is_active,
        group_id: filters
            .group_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }))
}

fn admin_user_matches_filters(
    user: &aether_data::repository::users::StoredUserExportRow,
    filters: Option<&NormalizedAdminUserSelectionFilters>,
) -> bool {
    let Some(filters) = filters else {
        return true;
    };
    if filters
        .role
        .as_deref()
        .is_some_and(|role| !user.role.eq_ignore_ascii_case(role))
    {
        return false;
    }
    if filters
        .is_active
        .is_some_and(|is_active| user.is_active != is_active)
    {
        return false;
    }
    if let Some(search) = filters.search.as_deref() {
        let searchable_text = format!(
            "{} {}",
            user.username,
            user.email.as_deref().unwrap_or_default()
        )
        .to_ascii_lowercase();
        let keywords = search
            .to_ascii_lowercase()
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !keywords
            .iter()
            .all(|keyword| searchable_text.contains(keyword))
        {
            return false;
        }
    }
    true
}

fn normalize_user_ids(user_ids: Vec<String>) -> Vec<String> {
    user_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parse_batch_mutation(
    action: &str,
    payload: Option<Value>,
) -> Result<AdminUserBatchMutation, String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "enable" => Ok(AdminUserBatchMutation {
            is_active: Some(true),
            modified_fields: vec!["is_active"],
            ..AdminUserBatchMutation::default()
        }),
        "disable" => Ok(AdminUserBatchMutation {
            is_active: Some(false),
            modified_fields: vec!["is_active"],
            ..AdminUserBatchMutation::default()
        }),
        "update_access_control" => parse_access_control_mutation(payload),
        "update_role" => parse_role_mutation(payload),
        "adjust_wallet_balance" => parse_wallet_balance_adjustment_mutation(payload),
        _ => Err("不支持的批量操作".to_string()),
    }
}

fn parse_wallet_balance_adjustment_mutation(
    payload: Option<Value>,
) -> Result<AdminUserBatchMutation, String> {
    let Some(Value::Object(payload)) = payload else {
        return Err("payload 必须是对象".to_string());
    };
    let operation = match payload.get("operation").and_then(Value::as_str) {
        Some("add") => AdminUserWalletBalanceOperation::Add,
        Some("deduct") => AdminUserWalletBalanceOperation::Deduct,
        _ => return Err("operation 必须为 add 或 deduct".to_string()),
    };
    let amount = payload
        .get("amount")
        .and_then(Value::as_f64)
        .ok_or_else(|| "amount 必须为大于 0 的有限数字".to_string())?;
    if !amount.is_finite() || amount <= 0.0 {
        return Err("amount 必须为大于 0 的有限数字".to_string());
    }

    Ok(AdminUserBatchMutation {
        wallet_balance_adjustment: Some(AdminUserWalletBalanceAdjustment { operation, amount }),
        modified_fields: vec!["wallet_balance"],
        ..AdminUserBatchMutation::default()
    })
}

fn parse_role_mutation(payload: Option<Value>) -> Result<AdminUserBatchMutation, String> {
    let Some(Value::Object(payload)) = payload else {
        return Err("payload 必须是对象".to_string());
    };
    let Some(value) = payload.get("role") else {
        return Err("role 参数不能为空".to_string());
    };
    let Some(role) = value.as_str() else {
        return Err("role 参数不合法".to_string());
    };
    let role = role.trim();
    if role.is_empty() {
        return Err("role 参数不能为空".to_string());
    }
    Ok(AdminUserBatchMutation {
        role: Some(normalize_admin_user_role(Some(role))?),
        modified_fields: vec!["role"],
        ..AdminUserBatchMutation::default()
    })
}

fn parse_access_control_mutation(payload: Option<Value>) -> Result<AdminUserBatchMutation, String> {
    let Some(Value::Object(payload)) = payload else {
        return Err("payload 必须是对象".to_string());
    };
    let mut mutation = AdminUserBatchMutation::default();

    if let Some(field) = disabled_user_policy_field(&payload) {
        return Err(disabled_user_policy_detail(field));
    }
    if let Some(value) = payload.get("unlimited") {
        mutation.unlimited = Some(parse_unlimited(value)?);
        mutation.modified_fields.push("unlimited");
    }

    if mutation.modified_fields.is_empty() {
        return Err("至少需要选择一个要修改的访问控制字段".to_string());
    }

    Ok(mutation)
}

fn parse_unlimited(value: &Value) -> Result<bool, String> {
    serde_json::from_value::<bool>(value.clone()).map_err(|_| "unlimited 必须是布尔值".to_string())
}

fn count_active_admin_demotions(
    mutation: &AdminUserBatchMutation,
    items: &[AdminUserSelectionItem],
) -> usize {
    if mutation
        .role
        .as_deref()
        .is_none_or(crate::roles::is_full_admin_role)
    {
        return 0;
    }
    items
        .iter()
        .filter(|item| item.is_active && item.role.eq_ignore_ascii_case("admin"))
        .count()
}

fn batch_role_demotion_failure_reason(
    mutation: &AdminUserBatchMutation,
    item: &AdminUserSelectionItem,
    active_admin_count: u64,
    active_admin_demotions: usize,
    current_admin_user_id: Option<&str>,
) -> Option<&'static str> {
    if mutation
        .role
        .as_deref()
        .is_none_or(crate::roles::is_full_admin_role)
        || !item.is_active
        || !item.role.eq_ignore_ascii_case("admin")
    {
        return None;
    }
    if current_admin_user_id.is_some_and(|user_id| user_id == item.user_id) {
        return Some("不能降级当前管理员账户");
    }
    if active_admin_count <= active_admin_demotions as u64 {
        return Some("不能降级最后一个管理员账户");
    }
    None
}

async fn apply_batch_user_wallet_limit_mode(
    state: &AdminAppState<'_>,
    user_id: &str,
    unlimited: bool,
) -> Result<bool, AdminBatchWalletLimitModeError> {
    let desired_limit_mode = if unlimited { "unlimited" } else { "finite" };
    let wallet = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            user_id,
        ))
        .await
        .map_err(|_| AdminBatchWalletLimitModeError::WalletLookup)?;
    match wallet {
        Some(wallet) => {
            if wallet.limit_mode.eq_ignore_ascii_case(desired_limit_mode) {
                return Ok(true);
            }
            Ok(state
                .update_auth_user_wallet_limit_mode(user_id, desired_limit_mode)
                .await
                .map_err(|_| AdminBatchWalletLimitModeError::Mutation)?
                .is_some())
        }
        None => Ok(state
            .initialize_auth_user_wallet(user_id, 0.0, unlimited)
            .await
            .map_err(|_| AdminBatchWalletLimitModeError::Mutation)?
            .is_some()),
    }
}

async fn apply_batch_user_wallet_balance_adjustment(
    state: &AdminAppState<'_>,
    user_id: &str,
    adjustment: AdminUserWalletBalanceAdjustment,
    operator_id: Option<&str>,
) -> Result<bool, AdminBatchWalletBalanceAdjustmentError> {
    // Resolve only the wallet ID; the repository clamps the deduction under its row lock.
    let Some(wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            user_id,
        ))
        .await
        .map_err(|_| AdminBatchWalletBalanceAdjustmentError::WalletLookup)?
    else {
        return Ok(false);
    };
    let amount = match adjustment.operation {
        AdminUserWalletBalanceOperation::Add => adjustment.amount,
        AdminUserWalletBalanceOperation::Deduct => -adjustment.amount,
    };

    state
        .admin_adjust_wallet_balance(
            &wallet.id,
            amount,
            "recharge",
            operator_id,
            Some("管理员批量调整用户余额"),
            true,
        )
        .await
        .map_err(|_| AdminBatchWalletBalanceAdjustmentError::BalanceAdjustment)
        .map(|result| result.is_some())
}

fn build_admin_user_batch_bad_request_response(detail: String) -> Response<Body> {
    if detail.as_str() == "缺少 user_id" {
        return build_admin_users_bad_request_response("缺少 user_id");
    }
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}
