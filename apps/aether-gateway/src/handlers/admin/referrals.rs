use crate::data::state::{ReferralRelationshipListQuery, ReferralRewardListQuery};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Default, Deserialize)]
struct ReferralAdminMutationRequest {
    note: Option<String>,
}

pub(crate) async fn maybe_build_local_admin_referrals_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    let request_context = request.request_context();
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("referrals_manage") {
        return Ok(None);
    }
    let response = match decision.route_kind.as_deref() {
        Some("referrals_overview") => {
            match request.state().app().referral_admin_overview().await? {
                Some(value) => Json(value).into_response(),
                None => build_admin_referrals_unavailable_response(),
            }
        }
        Some("referral_reward_detail") => {
            if !request.state().app().has_referral_data_backend() {
                build_admin_referrals_unavailable_response()
            } else if let Some(id) = reward_id_from_path(request_context.path(), "") {
                match request.state().app().referral_reward_detail(&id).await? {
                    Some(detail) => Json(detail).into_response(),
                    None => (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail":"Referral reward not found"})),
                    )
                        .into_response(),
                }
            } else {
                admin_referrals_bad_request("Reward ID invalid")
            }
        }
        Some("list_referrals") => {
            build_admin_referrals_list_response(&request.state(), &request_context).await?
        }
        Some("list_referral_rewards") => {
            build_admin_referral_rewards_list_response(&request.state(), &request_context).await?
        }
        Some("retry_referral_reward") => {
            build_admin_referral_reward_retry_response(
                &request.state(),
                &request_context,
                request.request_body(),
            )
            .await?
        }
        Some("void_referral_reward") => {
            build_admin_referral_reward_void_response(
                &request.state(),
                &request_context,
                request.request_body(),
            )
            .await?
        }
        _ => build_admin_referrals_unavailable_response(),
    };
    Ok(Some(response))
}

fn admin_referrals_bad_request(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn build_admin_referrals_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": "Admin referral data unavailable" })),
    )
        .into_response()
}

fn parse_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => value
            .parse::<usize>()
            .map(|value| value.clamp(1, 200))
            .map_err(|_| "limit 必须是正整数".to_string()),
        None => Ok(50),
    }
}

fn parse_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset 必须是非负整数".to_string()),
        None => Ok(0),
    }
}

fn parse_optional_bool(query: Option<&str>, key: &str) -> Result<Option<bool>, String> {
    let Some(value) = query_param_value(query, key) else {
        return Ok(None);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(Some(true)),
        "false" | "0" | "no" => Ok(Some(false)),
        _ => Err(format!("{key} 必须是布尔值")),
    }
}

fn operator_id(
    request_context: &crate::handlers::admin::request::AdminRequestContext<'_>,
) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
}

fn reward_id_from_path(path: &str, suffix: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let rest = trimmed.strip_prefix("/api/admin/referral-rewards/")?;
    let id = rest.strip_suffix(suffix)?.trim_end_matches('/');
    (!id.is_empty()).then_some(id.to_string())
}

fn parse_mutation_note(body: Option<&axum::body::Bytes>) -> Result<Option<String>, String> {
    let Some(body) = body.filter(|body| !body.is_empty()) else {
        return Ok(None);
    };
    let payload = serde_json::from_slice::<ReferralAdminMutationRequest>(body)
        .map_err(|_| "请求数据验证失败".to_string())?;
    Ok(payload
        .note
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

async fn build_admin_referrals_list_response(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    request_context: &crate::handlers::admin::request::AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let offset = match parse_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let first_paid = match parse_optional_bool(query, "first_paid") {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let Some((items, total, stats)) = state
        .app()
        .list_admin_referral_relationships(ReferralRelationshipListQuery {
            inviter: query_param_value(query, "inviter"),
            invitee: query_param_value(query, "invitee"),
            invite_code: query_param_value(query, "invite_code"),
            first_paid,
            limit,
            offset,
        })
        .await?
    else {
        return Ok(build_admin_referrals_unavailable_response());
    };
    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
        "stats": stats,
    }))
    .into_response())
}

async fn build_admin_referral_rewards_list_response(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    request_context: &crate::handlers::admin::request::AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let offset = match parse_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let pending_reversal = match parse_optional_bool(query, "pending_reversal") {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    let Some((items, total, stats)) = state
        .app()
        .list_admin_referral_rewards(ReferralRewardListQuery {
            order_id: query_param_value(query, "order_id"),
            order_no: query_param_value(query, "order_no"),
            inviter: query_param_value(query, "inviter"),
            invitee: query_param_value(query, "invitee"),
            referral_id: query_param_value(query, "referral_id"),
            trigger_point: query_param_value(query, "trigger_point"),
            pending_reversal,
            reward_type: query_param_value(query, "reward_type"),
            status: query_param_value(query, "status"),
            limit,
            offset,
        })
        .await?
    else {
        return Ok(build_admin_referrals_unavailable_response());
    };
    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
        "stats": stats,
    }))
    .into_response())
}

async fn build_admin_referral_reward_retry_response(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    request_context: &crate::handlers::admin::request::AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(reward_id) = reward_id_from_path(request_context.path(), "/retry") else {
        return Ok(admin_referrals_bad_request("返利记录不存在"));
    };
    let note = match parse_mutation_note(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    match state
        .app()
        .retry_referral_reward(
            &reward_id,
            operator_id(request_context).as_deref(),
            note.as_deref(),
        )
        .await?
    {
        Some(reward) => Ok(attach_admin_audit_response(
            Json(json!({ "reward": reward })).into_response(),
            "admin_referral_reward_retry",
            "retry_referral_reward",
            "referral_reward",
            &reward_id,
        )),
        None => Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "Referral reward not found" })),
        )
            .into_response()),
    }
}

async fn build_admin_referral_reward_void_response(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    request_context: &crate::handlers::admin::request::AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(reward_id) = reward_id_from_path(request_context.path(), "/void") else {
        return Ok(admin_referrals_bad_request("返利记录不存在"));
    };
    let note = match parse_mutation_note(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_referrals_bad_request(detail)),
    };
    match state
        .app()
        .void_referral_reward(
            &reward_id,
            operator_id(request_context).as_deref(),
            note.as_deref(),
        )
        .await?
    {
        Some(reward) => Ok(attach_admin_audit_response(
            Json(json!({ "reward": reward })).into_response(),
            "admin_referral_reward_void",
            "void_referral_reward",
            "referral_reward",
            &reward_id,
        )),
        None => Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "Referral reward not found" })),
        )
            .into_response()),
    }
}
