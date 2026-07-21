use crate::state::AdminSecurityBlacklistEntry;
use crate::{AppState, GatewayError};
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

const ADMIN_SECURITY_BLACKLIST_PREFIX: &str = "ip:blacklist:";
const ADMIN_SECURITY_WHITELIST_KEY: &str = "ip:whitelist";
const ADMIN_SECURITY_CACHE_TTL_MS_ENV: &str = "AETHER_GATEWAY_SECURITY_CACHE_TTL_MS";
const DEFAULT_ADMIN_SECURITY_CACHE_TTL_MS: u64 = 1_000;
const MAX_ADMIN_SECURITY_CACHE_TTL_MS: u64 = 30_000;
const ADMIN_SECURITY_WHITELIST_CACHE_KEY: &str = "rules";

static ADMIN_SECURITY_CACHE_TTL: LazyLock<Duration> = LazyLock::new(|| {
    let ttl_ms = std::env::var(ADMIN_SECURITY_CACHE_TTL_MS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_ADMIN_SECURITY_CACHE_TTL_MS)
        .min(MAX_ADMIN_SECURITY_CACHE_TTL_MS);
    Duration::from_millis(ttl_ms)
});

fn admin_security_cache_ttl() -> Duration {
    *ADMIN_SECURITY_CACHE_TTL
}

impl AppState {
    pub(crate) async fn admin_security_ip_blacklisted(
        &self,
        ip_address: IpAddr,
    ) -> Result<bool, GatewayError> {
        let cache_key = ip_address.to_string();
        let runtime_key = format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}{cache_key}");
        Ok(self
            .admin_security_blacklist_cache
            .get_or_load_once(cache_key, admin_security_cache_ttl(), || async {
                self.runtime_state
                    .kv_exists(&runtime_key)
                    .await
                    .map(Some)
                    .map_err(|err| GatewayError::Internal(err.to_string()))
            })
            .await?
            .unwrap_or(false))
    }

    pub(crate) async fn admin_security_ip_whitelisted(
        &self,
        ip_address: IpAddr,
    ) -> Result<bool, GatewayError> {
        let rules = self
            .admin_security_whitelist_cache
            .get_or_load_once(
                ADMIN_SECURITY_WHITELIST_CACHE_KEY.to_string(),
                admin_security_cache_ttl(),
                || async {
                    self.runtime_state
                        .set_members(ADMIN_SECURITY_WHITELIST_KEY)
                        .await
                        .map(Some)
                        .map_err(|err| GatewayError::Internal(err.to_string()))
                },
            )
            .await?
            .unwrap_or_default();
        Ok(rules
            .iter()
            .any(|rule| crate::handlers::shared::ip_rule_pattern_matches(rule.trim(), ip_address)))
    }

    pub(crate) async fn add_admin_security_blacklist(
        &self,
        ip_address: &str,
        reason: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<bool, GatewayError> {
        let key = format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}{ip_address}");
        self.runtime_state
            .kv_set(
                &key,
                reason.to_string(),
                ttl_seconds.map(std::time::Duration::from_secs),
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if let Ok(ip_address) = ip_address.parse::<IpAddr>() {
            self.admin_security_blacklist_cache.insert(
                ip_address.to_string(),
                Some(true),
                admin_security_cache_ttl(),
            );
        }
        Ok(true)
    }

    pub(crate) async fn remove_admin_security_blacklist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        let key = format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}{ip_address}");
        let removed = self
            .runtime_state
            .kv_delete(&key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if let Ok(ip_address) = ip_address.parse::<IpAddr>() {
            self.admin_security_blacklist_cache.insert(
                ip_address.to_string(),
                Some(false),
                admin_security_cache_ttl(),
            );
        }
        Ok(removed)
    }

    pub(crate) async fn admin_security_blacklist_stats(
        &self,
    ) -> Result<(bool, usize, Option<String>), GatewayError> {
        let total = self
            .runtime_state
            .scan_keys(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}*"), 100)
            .await
            .map(|keys| keys.len())
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((true, total, None))
    }

    pub(crate) async fn list_admin_security_blacklist(
        &self,
    ) -> Result<Vec<AdminSecurityBlacklistEntry>, GatewayError> {
        let keys = self
            .runtime_state
            .scan_keys(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}*"), 100)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let mut entries = Vec::new();
        for full_key in keys {
            let raw_key = self.runtime_state.strip_namespace(&full_key);
            let ip_address = raw_key
                .strip_prefix(ADMIN_SECURITY_BLACKLIST_PREFIX)
                .unwrap_or(raw_key)
                .to_string();
            let Some(reason) = self
                .runtime_state
                .kv_get(raw_key)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
            else {
                continue;
            };
            let ttl_seconds = self
                .runtime_state
                .kv_ttl_seconds(raw_key)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .filter(|ttl| *ttl >= 0);
            entries.push(AdminSecurityBlacklistEntry {
                ip_address,
                reason,
                ttl_seconds,
            });
        }
        entries.sort_by(|a, b| a.ip_address.cmp(&b.ip_address));
        Ok(entries)
    }

    pub(crate) async fn add_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        self.runtime_state
            .set_add(ADMIN_SECURITY_WHITELIST_KEY, ip_address)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.admin_security_whitelist_cache.clear();
        Ok(true)
    }

    pub(crate) async fn remove_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        let removed = self
            .runtime_state
            .set_remove(ADMIN_SECURITY_WHITELIST_KEY, ip_address)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        self.admin_security_whitelist_cache.clear();
        Ok(removed)
    }

    pub(crate) async fn list_admin_security_whitelist(&self) -> Result<Vec<String>, GatewayError> {
        self.runtime_state
            .set_members(ADMIN_SECURITY_WHITELIST_KEY)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
