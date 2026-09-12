mod admin_proxy;
mod api_keys;
mod auth_api_key_secret;
mod catalog;
mod email_templates;
mod external_models;
mod identity_oauth_provider_secret;
mod multipart;
mod normalize;
mod payloads;
mod payment_currency;
mod payment_direct;
mod payment_gateway_config;
mod payment_gateway_secret;
mod payment_order_stripe_secret;
mod provider_catalog_credential;
mod provider_ops_credential;
pub(crate) mod provider_pool;
mod request_utils;
mod runtime_secret;
mod system_config_values;
mod usage_stats;

pub(crate) use self::admin_proxy::{
    attach_admin_audit_response, build_admin_proxy_auth_required_response,
    build_unhandled_admin_proxy_response, mark_sensitive_admin_response_no_store,
};
pub(crate) use self::api_keys::{
    api_key_placeholder_display, configured_api_key_prefix, generate_gateway_api_key_plaintext,
    generate_gateway_secret_plaintext, masked_gateway_api_key_display, masked_secret_display,
    normalize_optional_api_key_concurrent_limit,
};
pub(crate) use self::auth_api_key_secret::{
    decrypt_or_migrate_auth_api_key_secret, open_auth_api_key_secret, seal_auth_api_key_secret,
};
pub(crate) use self::catalog::{
    build_admin_provider_key_response, decrypt_catalog_secret_or_legacy_plaintext,
    decrypt_catalog_secret_with_fallbacks, default_provider_key_status_snapshot,
    effective_catalog_encryption_key, encrypt_catalog_secret_with_fallbacks,
    masked_catalog_api_key, masked_catalog_api_key_for_provider, parse_catalog_auth_config_json,
    provider_catalog_key_supports_format, provider_key_health_summary,
    provider_key_health_summary_at, provider_key_status_snapshot_payload,
    sync_provider_key_oauth_status_snapshot, sync_provider_key_quota_status_snapshot,
    take_secret_prefix, take_secret_suffix, StoredCatalogSecret,
};
pub(crate) use self::email_templates::{
    admin_email_template_definition, admin_email_template_html_key,
    admin_email_template_subject_key, escape_admin_email_template_html,
    read_admin_email_template_payload, render_admin_email_template_html,
};
pub(crate) use self::external_models::OFFICIAL_EXTERNAL_MODEL_PROVIDERS;
pub(crate) use self::identity_oauth_provider_secret::{
    decrypt_or_migrate_identity_oauth_provider_client_secret,
    identity_oauth_provider_secret_binding_matches, seal_identity_oauth_provider_client_secret,
};
pub(crate) use self::multipart::{
    find_multipart_boundary, find_multipart_boundary_after_crlf, parse_multipart_boundary,
    MAX_MULTIPART_PARTS, MAX_MULTIPART_PART_HEADER_BYTES,
};
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
pub(crate) use self::payment_currency::{
    effective_payment_exchange_rate, normalize_payment_currency, stripe_amount_to_major,
    stripe_amount_to_minor,
};
pub(crate) use self::payment_direct::{
    close_direct_gateway_checkout, close_direct_gateway_order, create_alipay_direct_checkout,
    create_stripe_direct_checkout, create_wxpay_direct_checkout, find_payment_callback_order,
    payment_callback_settlement_values, public_payment_http_client, refund_direct_gateway_order,
    verify_alipay_notify_callback, verify_wxpay_notify_callback, DirectGatewayRefundResult,
    DirectPaymentCheckoutError, DirectPaymentCheckoutInput,
};
pub(crate) use self::payment_gateway_config::{
    normalize_payment_callback_base_url, normalize_payment_https_url,
    payment_gateway_allow_user_refund, payment_gateway_channels_config_json,
    payment_gateway_channels_json, payment_gateway_config_json,
    payment_gateway_provider_for_payment_method, payment_gateway_refund_enabled,
    payment_gateway_secret_keys_json,
};
pub(crate) use self::payment_gateway_secret::{
    open_payment_gateway_secret, payment_gateway_secret_is_legacy_unbound,
    seal_payment_gateway_secret, PaymentGatewaySecretBinding, PaymentGatewaySecretProjection,
};
pub(crate) use self::payment_order_stripe_secret::{
    normalize_stripe_client_secret, open_payment_order_stripe_client_secret,
    seal_payment_order_stripe_client_secret, PaymentOrderStripeSecretBinding,
    PaymentOrderStripeSecretProjection, STRIPE_CLIENT_SECRET_ENCRYPTED_KEY,
};
pub(crate) use self::provider_catalog_credential::{
    open_provider_catalog_credential, seal_provider_catalog_credential,
    ProviderCatalogCredentialField, ProviderCatalogCredentialProjection,
};
pub(crate) use self::provider_ops_credential::{
    canonicalize_provider_ops_base_url, open_provider_ops_credential,
    provider_ops_credential_binding_from_config, provider_ops_credential_field_is_secret,
    provider_ops_outbound_policy_digest, resolve_provider_ops_same_origin_url,
    seal_provider_ops_credential, ProviderOpsCanonicalDestination, ProviderOpsCredentialBinding,
    ProviderOpsCredentialProjection, PROVIDER_OPS_PERSISTENT_SECRET_FIELDS,
    PROVIDER_OPS_TRANSIENT_METADATA_FIELDS, PROVIDER_OPS_TRANSIENT_SECRET_FIELDS,
};
pub(crate) use self::request_utils::{
    admin_proxy_local_requires_buffered_body, internal_proxy_local_requires_buffered_body,
    json_string_list, local_proxy_route_requires_buffered_body,
    mark_external_models_official_providers, public_support_local_requires_buffered_body,
    query_param_bool, query_param_optional_bool, query_param_value,
    request_enables_control_execute, rust_auth_terminates_provider_credentials,
    sanitize_upstream_path_and_query, security_log_url_origin,
    should_strip_forwarded_provider_credential_header, should_strip_forwarded_trusted_admin_header,
    strip_query_param, unix_ms_to_rfc3339, unix_secs_to_rfc3339,
};
pub(crate) use self::runtime_secret::{
    open_runtime_secret_payload, open_runtime_secret_payload_with_encryption_key,
    runtime_secret_payload_is_sealed, seal_runtime_secret_payload,
    seal_runtime_secret_payload_with_encryption_key,
};
pub(crate) use self::system_config_values::{
    bark_device_key_binding, canonical_bark_server_url, decrypt_or_migrate_bark_device_key,
    decrypt_or_migrate_ldap_bind_password, decrypt_or_migrate_smtp_password,
    decrypt_or_migrate_system_config_secret, decrypt_system_config_secret, encrypt_bark_device_key,
    encrypt_ldap_bind_password, encrypt_smtp_password, encrypt_system_config_secret,
    ldap_attribute_description_is_valid, ldap_bind_password_binding_matches,
    ldap_distinguished_name_is_valid, ldap_module_config_is_valid, ldap_search_filter_is_valid,
    module_available_from_env, normalize_ldap_transport_server_url, smtp_password_binding,
    system_config_bool, system_config_string, BarkDeviceKeyBinding, SmtpPasswordBinding,
};
pub(crate) use self::usage_stats::{
    admin_stats_bad_request_response, parse_bounded_u32, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};
