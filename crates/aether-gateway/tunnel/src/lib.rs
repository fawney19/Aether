//! Shared tunnel request-lifecycle contracts.
//!
//! The executable tunnel agent and the embedded gateway tunnel both depend on
//! this crate. Network runtimes, persistence, and HTTP handlers remain in
//! their respective adapter crates.

pub mod admission;
pub mod embedded;
pub mod hub;
pub mod protocol;
pub mod relay;

pub use admission::{TunnelAdmissionClass, TunnelAdmissionPolicy, TunnelAdmissionRequest};
pub use embedded::EmbeddedTunnelDefaults;
pub use hub::{
    resolve_proxy_max_streams, resolve_proxy_node_name, resolve_proxy_protocol_version,
    MAX_TUNNEL_STREAMS,
};
pub use relay::{
    is_tunnel_heartbeat_path, is_tunnel_node_status_path, TunnelAttachmentRecord,
    DEFAULT_OWNER_RELAY_BODY_LIMIT_BYTES, DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES, PROXY_TUNNEL_PATH,
    TUNNEL_HEARTBEAT_PATH, TUNNEL_NODE_STATUS_PATH, TUNNEL_RELAY_PATH_PATTERN, TUNNEL_ROUTE_FAMILY,
};
