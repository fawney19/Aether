mod admin_proxy;
mod api_keys;
mod catalog;
mod email_templates;
mod external_models;
mod normalize;
mod payloads;
mod payment_direct;
mod payment_gateway_config;
pub(crate) mod provider_pool;
mod request_utils;
mod system_config_values;
mod usage_stats;

pub(crate) use self::admin_proxy::{
    attach_admin_audit_response, build_admin_proxy_auth_required_response,
    build_unhandled_admin_proxy_response,
};
pub(crate) use self::api_keys::{
    api_key_placeholder_display, configured_api_key_prefix, generate_gateway_api_key_plaintext,
    generate_gateway_secret_plaintext, masked_gateway_api_key_display,
    normalize_optional_api_key_concurrent_limit,
};
pub(crate) use self::catalog::{
    build_admin_provider_key_response, decrypt_catalog_secret_with_fallbacks,
    default_provider_key_status_snapshot, effective_catalog_encryption_key,
    encrypt_catalog_secret_with_fallbacks, masked_catalog_api_key, parse_catalog_auth_config_json,
    provider_catalog_key_supports_format, provider_key_health_summary,
    provider_key_health_summary_at, provider_key_status_snapshot_payload,
    sync_provider_key_oauth_status_snapshot, sync_provider_key_quota_status_snapshot,
    take_secret_prefix, take_secret_suffix,
};
pub(crate) use self::email_templates::{
    admin_email_template_definition, admin_email_template_html_key,
    admin_email_template_subject_key, escape_admin_email_template_html,
    read_admin_email_template_payload, render_admin_email_template_html,
};
pub(crate) use self::external_models::OFFICIAL_EXTERNAL_MODEL_PROVIDERS;
pub(crate) use self::normalize::{
    deserialize_optional_json_patch, deserialize_optional_string_list_patch,
    ip_rule_pattern_matches, ip_rules_allow, json_ip_rules_allow, normalize_feature_settings,
    normalize_ip_rules, normalize_json_array, normalize_json_object, normalize_string_list,
    normalize_user_self_feature_settings_update, parse_json_ip_rules,
};
pub(crate) use self::payloads::{
    InternalGatewayAuthContextRequest, InternalGatewayExecuteRequest,
    InternalGatewayResolveRequest, InternalTunnelHeartbeatRequest, InternalTunnelNodeStatusRequest,
};
pub(crate) use self::payment_direct::{
    close_direct_gateway_order, create_alipay_direct_checkout, create_stripe_direct_checkout,
    create_wxpay_direct_checkout, direct_payment_client_ip, refund_direct_gateway_order,
    verify_alipay_notify_callback, verify_wxpay_notify_callback, DirectGatewayRefundResult,
    DirectPaymentCheckoutInput,
};
pub(crate) use self::payment_gateway_config::{
    payment_gateway_allow_user_refund, payment_gateway_channels_config_json,
    payment_gateway_channels_json, payment_gateway_config_json,
    payment_gateway_provider_for_payment_method, payment_gateway_refund_enabled,
    payment_gateway_secret_keys_json,
};
pub(crate) use self::request_utils::{
    admin_proxy_local_requires_buffered_body, internal_proxy_local_requires_buffered_body,
    json_string_list, local_proxy_route_requires_buffered_body,
    mark_external_models_official_providers, public_support_local_requires_buffered_body,
    query_param_bool, query_param_optional_bool, query_param_value,
    request_enables_control_execute, rust_auth_terminates_provider_credentials,
    sanitize_upstream_path_and_query, should_strip_forwarded_provider_credential_header,
    should_strip_forwarded_trusted_admin_header, strip_query_param, unix_ms_to_rfc3339,
    unix_secs_to_rfc3339,
};
pub(crate) use self::system_config_values::{
    module_available_from_env, system_config_bool, system_config_string,
};
pub(crate) use self::usage_stats::{
    admin_stats_bad_request_response, parse_bounded_u32, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};
