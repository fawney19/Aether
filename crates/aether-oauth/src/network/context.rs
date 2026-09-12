use aether_contracts::ProxySnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthNetworkPolicy {
    DirectOnly,
    DirectOrSystemProxy,
    ProviderOperationProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRequirement {
    Optional,
    RequiredProxyNode,
    RequiredConfiguredProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuthTimeouts {
    pub connect_ms: u64,
    pub read_ms: u64,
    pub write_ms: u64,
    pub total_ms: u64,
}

impl OAuthTimeouts {
    pub const DIRECT_DEFAULT: Self = Self {
        connect_ms: 30_000,
        read_ms: 30_000,
        write_ms: 30_000,
        total_ms: 30_000,
    };

    pub const PROXY_DEFAULT: Self = Self {
        connect_ms: 60_000,
        read_ms: 60_000,
        write_ms: 60_000,
        total_ms: 60_000,
    };
}

#[derive(Clone, PartialEq)]
pub struct OAuthNetworkContext {
    pub policy: OAuthNetworkPolicy,
    pub requirement: NetworkRequirement,
    pub proxy: Option<ProxySnapshot>,
    pub timeouts: OAuthTimeouts,
}

impl std::fmt::Debug for OAuthNetworkContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthNetworkContext")
            .field("policy", &self.policy)
            .field("requirement", &self.requirement)
            .field("has_proxy", &self.proxy.is_some())
            .field("timeouts", &self.timeouts)
            .finish()
    }
}

impl OAuthNetworkContext {
    pub fn direct_identity() -> Self {
        Self {
            policy: OAuthNetworkPolicy::DirectOrSystemProxy,
            requirement: NetworkRequirement::Optional,
            proxy: None,
            timeouts: OAuthTimeouts::DIRECT_DEFAULT,
        }
    }

    pub fn provider_operation(proxy: Option<ProxySnapshot>) -> Self {
        let timeouts = if proxy.is_some() {
            OAuthTimeouts::PROXY_DEFAULT
        } else {
            OAuthTimeouts::DIRECT_DEFAULT
        };
        Self {
            policy: OAuthNetworkPolicy::ProviderOperationProxy,
            requirement: NetworkRequirement::Optional,
            proxy,
            timeouts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OAuthNetworkContext;
    use aether_contracts::ProxySnapshot;

    #[test]
    fn network_context_debug_output_does_not_expose_proxy_credentials() {
        let context = OAuthNetworkContext::provider_operation(Some(ProxySnapshot {
            url: Some("http://proxy-user:proxy-password@proxy.example:8080".to_string()),
            extra: Some(serde_json::json!({"authorization": "proxy-extra-canary"})),
            ..ProxySnapshot::default()
        }));

        let debug = format!("{context:?}");
        assert!(!debug.contains("proxy-user"));
        assert!(!debug.contains("proxy-password"));
        assert!(!debug.contains("proxy-extra-canary"));
        assert!(debug.contains("has_proxy: true"));
    }
}
