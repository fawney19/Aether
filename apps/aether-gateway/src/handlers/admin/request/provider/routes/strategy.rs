use crate::handlers::admin::provider::{
    shared::paths::{
        admin_provider_id_for_provider_strategy_billing,
        admin_provider_id_for_provider_strategy_quota,
        admin_provider_id_for_provider_strategy_stats, is_admin_provider_strategy_strategies_root,
    },
    strategy::{
        builders::{
            build_provider_strategy_list_response, build_provider_strategy_reset_quota_response,
            build_provider_strategy_stats_response,
            build_provider_strategy_update_billing_response, AdminProviderStrategyBillingRequest,
        },
        responses::{
            admin_provider_strategy_data_unavailable_response,
            ADMIN_PROVIDER_STRATEGY_DATA_UNAVAILABLE_DETAIL,
            ADMIN_PROVIDER_STRATEGY_STATS_DATA_UNAVAILABLE_DETAIL,
        },
    },
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::query_param_value;
use crate::handlers::admin::AdminRequestContext;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn maybe_build_admin_provider_strategy_route_response(
        &self,
        request_context: &AdminRequestContext<'_>,
        request_body: Option<&Bytes>,
    ) -> Result<Option<Response<Body>>, GatewayError> {
        let Some(decision) = request_context.decision() else {
            return Ok(None);
        };
        if decision.route_family.as_deref() != Some("provider_strategy_manage") {
            return Ok(None);
        }

        if decision.route_kind.as_deref() == Some("list_strategies")
            && request_context.method() == http::Method::GET
            && is_admin_provider_strategy_strategies_root(request_context.path())
        {
            return Ok(Some(build_provider_strategy_list_response()));
        }

        if decision.route_kind.as_deref() == Some("update_provider_billing")
            && request_context.method() == http::Method::PUT
        {
            if !self.has_provider_catalog_data_reader() || !self.has_provider_catalog_data_writer()
            {
                return Ok(Some(admin_provider_strategy_data_unavailable_response(
                    ADMIN_PROVIDER_STRATEGY_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let Some(provider_id) =
                admin_provider_id_for_provider_strategy_billing(request_context.path())
            else {
                return Ok(Some(admin_provider_strategy_provider_not_found_response()));
            };

            let Some(request_body) = request_body else {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体不能为空" })),
                    )
                        .into_response(),
                ));
            };
            let payload =
                match serde_json::from_slice::<AdminProviderStrategyBillingRequest>(request_body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(
                            (
                                http::StatusCode::BAD_REQUEST,
                                Json(json!({ "detail": "请求数据验证失败" })),
                            )
                                .into_response(),
                        ))
                    }
                };

            return Ok(Some(
                build_provider_strategy_update_billing_response(self, provider_id, payload).await?,
            ));
        }

        if decision.route_kind.as_deref() == Some("get_provider_stats")
            && request_context.method() == http::Method::GET
        {
            if !self.has_provider_catalog_data_reader() || !self.has_usage_data_reader() {
                return Ok(Some(admin_provider_strategy_data_unavailable_response(
                    ADMIN_PROVIDER_STRATEGY_STATS_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let Some(provider_id) =
                admin_provider_id_for_provider_strategy_stats(request_context.path())
            else {
                return Ok(Some(admin_provider_strategy_provider_not_found_response()));
            };

            let hours = query_param_value(request_context.query_string(), "hours")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(24);

            return Ok(Some(
                build_provider_strategy_stats_response(self, provider_id, hours).await?,
            ));
        }

        if decision.route_kind.as_deref() == Some("reset_provider_quota")
            && request_context.method() == http::Method::DELETE
        {
            if !self.has_provider_catalog_data_reader() || !self.has_provider_catalog_data_writer()
            {
                return Ok(Some(admin_provider_strategy_data_unavailable_response(
                    ADMIN_PROVIDER_STRATEGY_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let Some(provider_id) =
                admin_provider_id_for_provider_strategy_quota(request_context.path())
            else {
                return Ok(Some(admin_provider_strategy_provider_not_found_response()));
            };

            return Ok(Some(
                build_provider_strategy_reset_quota_response(self, provider_id).await?,
            ));
        }

        Ok(Some(admin_provider_strategy_dispatcher_not_found_response()))
    }
}

fn admin_provider_strategy_provider_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Provider not found" })),
    )
        .into_response()
}

fn admin_provider_strategy_dispatcher_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Provider strategy route not found" })),
    )
        .into_response()
}
