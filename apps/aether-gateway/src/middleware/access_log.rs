//! Compatibility facade for frontdoor access logging.

pub(crate) use aether_gateway_frontdoor::{
    access_log_middleware, sanitize_access_log_path, should_downgrade_access_log,
    GatewayRequestAcceptedAt, RequestLogEmitted,
};
