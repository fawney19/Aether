pub mod protocol;

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedTunnelDefaults {
    pub proxy_idle_timeout: Duration,
    pub ping_interval: Duration,
    pub max_streams: usize,
    pub outbound_queue_capacity: usize,
}

impl Default for EmbeddedTunnelDefaults {
    fn default() -> Self {
        Self {
            proxy_idle_timeout: Duration::ZERO,
            ping_interval: Duration::from_secs(15),
            max_streams: crate::MAX_TUNNEL_STREAMS,
            outbound_queue_capacity: 512,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EmbeddedTunnelDefaults;

    #[test]
    fn defaults_are_bounded_for_embedded_runtime() {
        let defaults = EmbeddedTunnelDefaults::default();
        assert_eq!(defaults.max_streams, 2_048);
        assert_eq!(defaults.outbound_queue_capacity, 512);
        assert!(defaults.proxy_idle_timeout.is_zero());
        assert_eq!(defaults.ping_interval.as_secs(), 15);
    }
}
