use super::{
    auth_password_policy_level, base_url_from_request, build_auth_error_response,
    build_auth_wallet_summary_payload, decrypt_catalog_secret_with_fallbacks,
    encrypt_catalog_secret_with_fallbacks, filter_eligible_model_rows_for_state, handle_auth_me,
    handle_users_me_api_key_install_session_create, query_param_optional_bool, query_param_value,
    resolve_authenticated_local_user, sanitize_public_model_config_for_user, unix_secs_to_rfc3339,
    users_me_api_key_install_sessions_path_matches, validate_auth_register_password, AppState,
    AuthenticatedLocalUserContext, GatewayPublicRequestContext, PUBLIC_CAPABILITY_DEFINITIONS,
};
use crate::admin_api::build_admin_endpoint_health_status_payload;
use crate::handlers::internal::build_management_token_payload;
use crate::handlers::shared::{
    admin_stats_bad_request_response, parse_bounded_u32, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};

const USERS_ME_AVAILABLE_MODELS_FETCH_LIMIT: usize = 1000;
const USERS_ME_CATALOG_API_FORMATS: &[&str] = &[
    "openai:chat",
    "openai:responses",
    "openai:responses:compact",
    "openai:image",
    "openai:embedding",
    "openai:rerank",
    "claude:messages",
    "gemini:generate_content",
    "gemini:embedding",
    "jina:embedding",
    "jina:rerank",
    "doubao:embedding",
    "aliyun:multimodal_embedding",
];

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
#[path = "user_me_referral.rs"]
mod user_me_referral;
use user_me_referral::*;
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
