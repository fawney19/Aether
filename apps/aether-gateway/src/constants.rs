pub(crate) const TRACE_ID_HEADER: &str = "x-trace-id";
pub(crate) const FRONTDOOR_MANIFEST_VERSION: &str = "aether.frontdoor/v1alpha1";
pub(crate) const FRONTDOOR_MANIFEST_PATH: &str = "/.well-known/aether/frontdoor.json";
pub(crate) const INTERNAL_FRONTDOOR_MANIFEST_PATH: &str = "/_gateway/frontdoor/manifest";
pub(crate) const READYZ_PATH: &str = "/readyz";
pub(crate) const FORWARDED_HOST_HEADER: &str = "x-forwarded-host";
pub(crate) const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";
pub(crate) const FORWARDED_PROTO_HEADER: &str = "x-forwarded-proto";
pub(crate) const GATEWAY_HEADER: &str = "x-aether-gateway";
pub(crate) const EXECUTION_PATH_HEADER: &str = "x-aether-execution-path";
pub(crate) const DEPENDENCY_REASON_HEADER: &str = "x-aether-dependency-reason";
pub(crate) const EXECUTION_RUNTIME_LOOP_GUARD_HEADER: &str = "x-aether-execution-loop-guard";
pub(crate) const EXECUTION_RUNTIME_LOOP_GUARD_VALUE: &str = "local-runtime";
pub(crate) const EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN: &str = "aether-execution-runtime";
pub(crate) const LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER: &str =
    "x-aether-local-execution-runtime-miss-reason";
pub(crate) const TUNNEL_AFFINITY_FORWARDED_BY_HEADER: &str =
    "x-aether-tunnel-affinity-forwarded-by";
pub(crate) const TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER: &str =
    "x-aether-tunnel-affinity-owner-instance-id";
pub(crate) const EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH: &str = "public_proxy_passthrough";
pub(crate) const EXECUTION_PATH_LOCAL_PROXY_PASSTHROUGH_REMOVED: &str =
    "local_proxy_passthrough_removed";
pub(crate) const EXECUTION_PATH_EXECUTION_RUNTIME_SYNC: &str = "execution_runtime_sync";
pub(crate) const EXECUTION_PATH_EXECUTION_RUNTIME_STREAM: &str = "execution_runtime_stream";
pub(crate) const EXECUTION_PATH_CONTROL_EXECUTE_SYNC: &str = "control_execute_sync";
pub(crate) const EXECUTION_PATH_CONTROL_EXECUTE_STREAM: &str = "control_execute_stream";
pub(crate) const EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS: &str = "local_execution_runtime_miss";
pub(crate) const EXECUTION_PATH_LOCAL_API_KEY_CONCURRENCY_LIMITED: &str =
    "local_api_key_concurrency_limited";
pub(crate) const API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS: u64 = 150;
pub(crate) const API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS: u64 = 10;
pub(crate) const EXECUTION_PATH_LOCAL_AUTH_DENIED: &str = "local_auth_denied";
pub(crate) const EXECUTION_PATH_LOCAL_RATE_LIMITED: &str = "local_rate_limited";
pub(crate) const EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND: &str = "local_route_not_found";
pub(crate) const EXECUTION_PATH_LOCAL_OVERLOADED: &str = "local_overloaded";
pub(crate) const EXECUTION_PATH_DISTRIBUTED_OVERLOADED: &str = "distributed_overloaded";
pub(crate) const EXECUTION_PATH_LOCAL_AI_PUBLIC: &str = "local_ai_public";
pub(crate) const EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED: &str =
    "local_execution_loop_detected";
pub(crate) const CONTROL_ROUTE_CLASS_HEADER: &str = "x-aether-control-route-class";
pub(crate) const CONTROL_ROUTE_FAMILY_HEADER: &str = "x-aether-control-route-family";
pub(crate) const CONTROL_ROUTE_KIND_HEADER: &str = "x-aether-control-route-kind";
pub(crate) const CONTROL_EXECUTION_RUNTIME_HEADER: &str =
    "x-aether-control-execution-runtime-candidate";
pub(crate) const CONTROL_EXECUTION_RUNTIME_CANDIDATE_KEY: &str = "execution_runtime_candidate";
pub(crate) const CONTROL_REQUEST_ID_HEADER: &str = "x-aether-control-request-id";
pub(crate) const CONTROL_CANDIDATE_ID_HEADER: &str = "x-aether-control-candidate-id";
pub(crate) const CONTROL_ENDPOINT_SIGNATURE_HEADER: &str = "x-aether-control-endpoint-signature";
pub(crate) const CONTROL_EXECUTED_HEADER: &str = "x-aether-control-executed";
pub(crate) const CONTROL_ACTION_HEADER: &str = "x-aether-control-action";
pub(crate) const CONTROL_ACTION_PROXY_PUBLIC: &str = "proxy_public";
pub(crate) const CONTROL_EXECUTE_FALLBACK_HEADER: &str = "x-aether-control-execute-fallback";
pub(crate) const TRUSTED_AUTH_USER_ID_HEADER: &str = "x-aether-auth-user-id";
pub(crate) const TRUSTED_AUTH_API_KEY_ID_HEADER: &str = "x-aether-auth-api-key-id";
pub(crate) const TRUSTED_AUTH_BALANCE_HEADER: &str = "x-aether-auth-balance-remaining";
pub(crate) const TRUSTED_AUTH_ACCESS_ALLOWED_HEADER: &str = "x-aether-auth-access-allowed";
pub(crate) const TRUSTED_ADMIN_USER_ID_HEADER: &str = "x-aether-admin-user-id";
pub(crate) const TRUSTED_ADMIN_USER_ROLE_HEADER: &str = "x-aether-admin-user-role";
pub(crate) const TRUSTED_ADMIN_SESSION_ID_HEADER: &str = "x-aether-admin-session-id";
pub(crate) const TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER: &str =
    "x-aether-admin-management-token-id";
pub(crate) const TRUSTED_RATE_LIMIT_PREFLIGHT_HEADER: &str = "x-aether-rate-limit-preflight";

pub(crate) const FRONTDOOR_REPLACEABLE_ROUTE_GROUPS: &[&str] = &["frontdoor_compat_router"];
pub(crate) const FRONTDOOR_REPLACEABLE_MIDDLEWARE_GROUPS: &[&str] = &["cors"];
pub(crate) const INTERNAL_GATEWAY_ROUTE_GROUPS: &[&str] = &["internal_gateway_router"];
pub(crate) const INTERNAL_GATEWAY_PATH_PREFIXES: &[&str] = &["/api/internal/gateway"];
pub(crate) const RUST_FRONTDOOR_OWNED_ROUTE_PATTERNS: &[&str] = &[
    FRONTDOOR_MANIFEST_PATH,
    INTERNAL_FRONTDOOR_MANIFEST_PATH,
    READYZ_PATH,
    "/_gateway/health",
    "/_gateway/metrics",
    "/_gateway/async-tasks/*",
    "/_gateway/audit/*",
    "/health",
    "/v1/health",
    "/v1/providers",
    "/v1/providers/{path...}",
    "/v1/test-connection",
    "/test-connection",
    "/api/public/site-info",
    "/api/public/providers",
    "/api/public/models",
    "/api/public/search/models",
    "/api/public/stats",
    "/api/public/global-models",
    "/api/public/health/api-formats",
    "/api/oauth/providers",
    "/api/oauth/{provider_type}/authorize",
    "/api/oauth/{provider_type}/callback",
    "/api/user/oauth/bindable-providers",
    "/api/user/oauth/links",
    "/api/user/oauth/{provider_type}/bind-token",
    "/api/user/oauth/{provider_type}/bind",
    "/api/user/oauth/{provider_type}",
    "/api/modules/auth-status",
    "/api/capabilities",
    "/api/capabilities/user-configurable",
    "/api/capabilities/model/{path...}",
    "/api/internal/gateway/{path...}",
    "/api/internal/proxy-tunnel",
    "/api/internal/tunnel/heartbeat",
    "/api/internal/tunnel/node-status",
    "/api/internal/tunnel/relay/{node_id}",
    "/v1/models",
    "/v1/models/{path...}",
    "/v1beta/models",
    "/v1beta/models/{path...}",
    "/v1/chat/completions",
    "/v1/images/generations",
    "/v1/images/edits",
    "/v1/messages",
    "/v1/messages/count_tokens",
    "/v1/responses",
    "/v1/responses/compact",
    "/v1/models/{model}:generateContent",
    "/v1/models/{model}:streamGenerateContent",
    "/v1/models/{model}:predictLongRunning",
    "/v1beta/models/{model}:generateContent",
    "/v1beta/models/{model}:streamGenerateContent",
    "/v1beta/models/{model}:predictLongRunning",
    "/v1beta/models/{model}/operations/{id}",
    "/v1beta/operations",
    "/v1beta/operations/{id}",
    "/v1/videos",
    "/v1/videos/{path...}",
    "/upload/v1beta/files",
    "/v1beta/files",
    "/v1beta/files/{path...}",
    "/",
    "/{*path}",
];
