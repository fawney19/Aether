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

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthNetworkContext {
    pub policy: OAuthNetworkPolicy,
    pub requirement: NetworkRequirement,
    pub proxy: Option<ProxySnapshot>,
    pub timeouts: OAuthTimeouts,
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
