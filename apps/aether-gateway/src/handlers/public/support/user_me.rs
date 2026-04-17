use super::{
    auth_password_policy_level, build_auth_error_response, build_auth_wallet_summary_payload,
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks, handle_auth_me,
    query_param_optional_bool, query_param_value, resolve_authenticated_local_user,
    unix_secs_to_rfc3339, validate_auth_register_password, AppState, AuthenticatedLocalUserContext,
    GatewayPublicRequestContext, PUBLIC_CAPABILITY_DEFINITIONS,
};
use crate::admin_api::build_admin_endpoint_health_status_payload;
use crate::handlers::internal::build_management_token_payload;
use crate::handlers::shared::{
    admin_stats_bad_request_response, parse_bounded_u32, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};

const USERS_ME_AVAILABLE_MODELS_FETCH_LIMIT: usize = 1000;

#[path = "user_me_management_tokens.rs"]
mod user_me_management_tokens;
use user_me_management_tokens::*;

#[path = "user_me_api_keys.rs"]
mod user_me_api_keys;
use user_me_api_keys::*;
#[path = "user_me_usage.rs"]
mod user_me_usage;
use user_me_usage::*;
#[path = "user_me_catalog.rs"]
mod user_me_catalog;
use user_me_catalog::*;
#[path = "user_me_preferences.rs"]
mod user_me_preferences;
use user_me_preferences::*;
#[path = "user_me_profile.rs"]
mod user_me_profile;
use user_me_profile::*;
#[path = "user_me_sessions.rs"]
mod user_me_sessions;
use user_me_sessions::*;
#[path = "user_me_shared.rs"]
mod user_me_shared;
use user_me_shared::*;
#[path = "user_me_routes.rs"]
mod user_me_routes;
use user_me_routes::*;

pub(super) use self::user_me_routes::maybe_build_local_users_me_response;
