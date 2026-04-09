use crate::state::AdminSecurityBlacklistEntry;
use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn add_admin_security_blacklist(
        &self,
        ip_address: &str,
        reason: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<bool, GatewayError> {
        const ADMIN_SECURITY_BLACKLIST_PREFIX: &str = "ip:blacklist:";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            let key = runner
                .keyspace()
                .key(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}{ip_address}"));
            let result = if let Some(ttl_seconds) = ttl_seconds {
                redis::cmd("SETEX")
                    .arg(&key)
                    .arg(ttl_seconds)
                    .arg(reason)
                    .query_async::<String>(&mut connection)
                    .await
            } else {
                redis::cmd("SET")
                    .arg(&key)
                    .arg(reason)
                    .query_async::<String>(&mut connection)
                    .await
            };
            return Ok(result.is_ok());
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_blacklist_store.as_ref() {
            store
                .lock()
                .expect("admin security blacklist store should lock")
                .insert(ip_address.to_string(), reason.to_string());
            return Ok(true);
        }

        Ok(false)
    }

    pub(crate) async fn remove_admin_security_blacklist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        const ADMIN_SECURITY_BLACKLIST_PREFIX: &str = "ip:blacklist:";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            let key = runner
                .keyspace()
                .key(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}{ip_address}"));
            let deleted = match redis::cmd("DEL")
                .arg(&key)
                .query_async::<i64>(&mut connection)
                .await
            {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            return Ok(deleted > 0);
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_blacklist_store.as_ref() {
            let removed = store
                .lock()
                .expect("admin security blacklist store should lock")
                .remove(ip_address)
                .is_some();
            return Ok(removed);
        }

        Ok(false)
    }

    pub(crate) async fn admin_security_blacklist_stats(
        &self,
    ) -> Result<(bool, usize, Option<String>), GatewayError> {
        const ADMIN_SECURITY_BLACKLIST_PREFIX: &str = "ip:blacklist:";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok((false, 0, Some("Redis 不可用".to_string()))),
            };
            let pattern = runner
                .keyspace()
                .key(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}*"));
            let mut cursor = 0u64;
            let mut total = 0usize;
            loop {
                let (next_cursor, keys) = match redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(100)
                    .query_async::<(u64, Vec<String>)>(&mut connection)
                    .await
                {
                    Ok(value) => value,
                    Err(err) => return Ok((false, 0, Some(err.to_string()))),
                };
                total += keys.len();
                if next_cursor == 0 {
                    break;
                }
                cursor = next_cursor;
            }
            return Ok((true, total, None));
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_blacklist_store.as_ref() {
            let total = store
                .lock()
                .expect("admin security blacklist store should lock")
                .len();
            return Ok((true, total, None));
        }

        Ok((false, 0, Some("Redis 不可用".to_string())))
    }

    pub(crate) async fn list_admin_security_blacklist(
        &self,
    ) -> Result<Vec<AdminSecurityBlacklistEntry>, GatewayError> {
        const ADMIN_SECURITY_BLACKLIST_PREFIX: &str = "ip:blacklist:";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(Vec::new()),
            };
            let pattern = runner
                .keyspace()
                .key(&format!("{ADMIN_SECURITY_BLACKLIST_PREFIX}*"));
            let prefix = runner.keyspace().key(ADMIN_SECURITY_BLACKLIST_PREFIX);
            let mut cursor = 0u64;
            let mut entries = Vec::new();
            loop {
                let (next_cursor, keys) = match redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(100)
                    .query_async::<(u64, Vec<String>)>(&mut connection)
                    .await
                {
                    Ok(value) => value,
                    Err(_) => break,
                };
                for full_key in keys {
                    let ip_address = full_key
                        .strip_prefix(prefix.as_str())
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| full_key.clone());
                    let reason: Result<String, _> = redis::cmd("GET")
                        .arg(&full_key)
                        .query_async(&mut connection)
                        .await;
                    let reason = match reason {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let ttl = match redis::cmd("TTL")
                        .arg(&full_key)
                        .query_async::<i64>(&mut connection)
                        .await
                    {
                        Ok(value) if value >= 0 => Some(value),
                        _ => None,
                    };
                    entries.push(AdminSecurityBlacklistEntry {
                        ip_address,
                        reason,
                        ttl_seconds: ttl,
                    });
                }
                if next_cursor == 0 {
                    break;
                }
                cursor = next_cursor;
            }
            entries.sort_by(|a, b| a.ip_address.cmp(&b.ip_address));
            return Ok(entries);
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_blacklist_store.as_ref() {
            let mut entries = store
                .lock()
                .expect("admin security blacklist store should lock")
                .iter()
                .map(|(ip, reason)| AdminSecurityBlacklistEntry {
                    ip_address: ip.clone(),
                    reason: reason.clone(),
                    ttl_seconds: None,
                })
                .collect::<Vec<_>>();
            entries.sort_by(|a, b| a.ip_address.cmp(&b.ip_address));
            return Ok(entries);
        }

        Ok(Vec::new())
    }

    pub(crate) async fn add_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        const ADMIN_SECURITY_WHITELIST_KEY: &str = "ip:whitelist";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            let key = runner.keyspace().key(ADMIN_SECURITY_WHITELIST_KEY);
            let added = match redis::cmd("SADD")
                .arg(&key)
                .arg(ip_address)
                .query_async::<i64>(&mut connection)
                .await
            {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            return Ok(added >= 0);
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_whitelist_store.as_ref() {
            store
                .lock()
                .expect("admin security whitelist store should lock")
                .insert(ip_address.to_string());
            return Ok(true);
        }

        Ok(false)
    }

    pub(crate) async fn remove_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        const ADMIN_SECURITY_WHITELIST_KEY: &str = "ip:whitelist";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            let key = runner.keyspace().key(ADMIN_SECURITY_WHITELIST_KEY);
            let removed = match redis::cmd("SREM")
                .arg(&key)
                .arg(ip_address)
                .query_async::<i64>(&mut connection)
                .await
            {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            return Ok(removed > 0);
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_whitelist_store.as_ref() {
            let removed = store
                .lock()
                .expect("admin security whitelist store should lock")
                .remove(ip_address);
            return Ok(removed);
        }

        Ok(false)
    }

    pub(crate) async fn list_admin_security_whitelist(&self) -> Result<Vec<String>, GatewayError> {
        const ADMIN_SECURITY_WHITELIST_KEY: &str = "ip:whitelist";

        if let Some(runner) = self.redis_kv_runner() {
            let mut connection = match runner.client().get_multiplexed_async_connection().await {
                Ok(value) => value,
                Err(_) => return Ok(Vec::new()),
            };
            let key = runner.keyspace().key(ADMIN_SECURITY_WHITELIST_KEY);
            let mut whitelist = match redis::cmd("SMEMBERS")
                .arg(&key)
                .query_async::<Vec<String>>(&mut connection)
                .await
            {
                Ok(value) => value,
                Err(_) => return Ok(Vec::new()),
            };
            whitelist.sort();
            return Ok(whitelist);
        }

        #[cfg(test)]
        if let Some(store) = self.admin_security_whitelist_store.as_ref() {
            return Ok(store
                .lock()
                .expect("admin security whitelist store should lock")
                .iter()
                .cloned()
                .collect());
        }

        Ok(Vec::new())
    }
}
