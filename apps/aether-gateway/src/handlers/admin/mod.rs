mod announcements;
pub(super) mod auth;
mod billing;
pub(super) mod endpoint;
pub(super) mod features;
mod model;
pub(super) mod observability;
pub(super) mod provider;
mod system;
mod users;

pub(super) mod request;
pub(super) mod routes;
mod shared;

pub(crate) use self::auth::maybe_build_local_admin_security_response;
pub(crate) use self::endpoint::build_admin_endpoint_health_status_payload;
pub(crate) use self::features::maybe_build_local_admin_video_tasks_response;
pub(crate) use self::observability::{
    admin_stats_bad_request_response, list_usage_for_optional_range,
    maybe_build_local_admin_usage_response, parse_bounded_u32, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};
pub(crate) use self::provider::oauth::errors::build_internal_control_error_response;
pub(crate) use self::provider::ops::providers::actions::admin_provider_ops_local_action_response;
pub(crate) use self::provider::pool_admin::maybe_build_local_admin_pool_response;
pub(crate) use self::provider::{
    maybe_build_local_admin_provider_oauth_response, maybe_build_local_admin_providers_response,
};
pub(crate) use self::request::{
    AdminAppState, AdminRequestContext, AdminRouteRequest, AdminRouteResponse, AdminRouteResult,
};
pub(crate) use self::routes::maybe_build_local_admin_response;
