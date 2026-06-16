use serde_json::Value;
use std::collections::BTreeSet;

const ACCOUNT_BLOCK_REASON_KEYWORDS: &[&str] = &[
    "suspended",
    "banned",
    "account_block",
    "account blocked",
    "account_forbidden",
    "forbidden",
    "封禁",
    "封号",
    "被封",
    "账户已封禁",
    "账号异常",
    "account has been disabled",
    "account disabled",
    "account has been deactivated",
    "account_deactivated",
    "account deactivated",
    "organization has been disabled",
    "organization_disabled",
    "deactivated_workspace",
    "deactivated",
    "访问被禁止",
    "账户访问被禁止",
    "访问受限",
    "账户访问受限",
    "oauth_token_invalid",
    "oauth_token_expired",
    "token_invalidated",
    "session expired",
    "authentication token has been invalidated",
    "token has been invalidated",
    "codex token 无效或已过期",
    "validation_required",
    "verify your account",
    "需要验证",
    "验证账号",
    "验证身份",
];

const AUTO_REMOVABLE_ACCOUNT_STATE_CODES: &[&str] = &[
    "account_banned",
    "account_suspended",
    "account_disabled",
    "account_quarantined",
    "workspace_deactivated",
    "account_forbidden",
    "oauth_token_invalid",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolAccountState {
    pub blocked: bool,
    pub code: Option<String>,
    pub label: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub recoverable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatusSnapshot {
    pub code: String,
    pub label: Option<String>,
    pub reason: Option<String>,
    pub blocked: bool,
    pub source: Option<String>,
    pub recoverable: bool,
}

impl Default for AccountStatusSnapshot {
    fn default() -> Self {
        Self {
            code: "ok".to_string(),
            label: None,
            reason: None,
            blocked: false,
            source: None,
            recoverable: false,
        }
    }
}

fn clean_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().is_some_and(|value| value != 0),
        Some(Value::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y"
        ),
        _ => false,
    }
}

fn extract_reason(source: &serde_json::Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| source.get(*field).and_then(Value::as_str))
        .and_then(|value| clean_text(Some(value)))
}

fn looks_like_workspace_deactivated(reason: Option<&str>) -> bool {
    clean_text(reason)
        .is_some_and(|value| value.to_ascii_lowercase().contains("deactivated_workspace"))
}

fn looks_like_account_verification(reason: &str) -> bool {
    let lowered = reason.to_ascii_lowercase();
    [
        "validation_required",
        "verify your account",
        "需要验证",
        "验证账号",
        "验证身份",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
}

pub fn oauth_token_reason_is_expired(reason: &str) -> bool {
    let lowered = reason.trim().to_ascii_lowercase();
    !lowered.is_empty()
        && [
            "oauth_token_expired",
            "session has expired",
            "session expired",
            "access token expired",
            "expired access token",
            "token expired",
            "token has expired",
            "security token included in the request is expired",
            "已过期",
            "过期",
        ]
        .iter()
        .any(|keyword| lowered.contains(keyword))
}

pub fn oauth_token_reason_is_hard_invalid(reason: &str) -> bool {
    let lowered = reason.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return false;
    }

    if [
        "oauth_token_invalid",
        "token_invalidated",
        "authentication token has been invalidated",
        "token has been invalidated",
        "token invalidated",
        "invalidated",
        "revoked",
        "已撤销",
        "被撤销",
        "撤销",
        "已作废",
        "作废",
        "已失效",
        "token 失效",
        "令牌失效",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
    {
        return true;
    }

    (lowered.contains("token 无效") || lowered.contains("令牌无效"))
        && !oauth_token_reason_is_expired(reason)
}

pub fn oauth_token_account_status_parts(reason: &str) -> (&'static str, &'static str) {
    if oauth_token_reason_is_hard_invalid(reason) {
        ("oauth_token_invalid", "Token 失效")
    } else {
        ("oauth_token_expired", "Token 过期")
    }
}

pub fn oauth_token_snapshot_status_parts(reason: &str) -> (&'static str, &'static str) {
    if oauth_token_reason_is_hard_invalid(reason) {
        ("invalid", "已失效")
    } else {
        ("expired", "已过期")
    }
}

fn classify_block_reason(reason: &str) -> (&'static str, &'static str) {
    let lowered = reason.to_ascii_lowercase();
    if oauth_token_reason_is_hard_invalid(reason) || oauth_token_reason_is_expired(reason) {
        return oauth_token_account_status_parts(reason);
    }
    if [
        "authentication token has been invalidated",
        "token has been invalidated",
        "codex token 无效或已过期",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
    {
        return ("oauth_token_invalid", "Token 失效");
    }
    if looks_like_account_verification(reason) {
        return ("account_verification", "需要验证");
    }
    if lowered.contains("deactivated_workspace") {
        return ("workspace_deactivated", "工作区停用");
    }
    if [
        "account has been disabled",
        "account disabled",
        "account has been deactivated",
        "account_deactivated",
        "account deactivated",
        "organization has been disabled",
        "organization_disabled",
        "访问被禁止",
        "账户访问被禁止",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
    {
        return ("account_disabled", "账号停用");
    }
    if ["account_forbidden", "forbidden", "访问受限", "账户访问受限"]
        .iter()
        .any(|keyword| lowered.contains(keyword))
    {
        return ("account_forbidden", "访问受限");
    }
    if [
        "suspended",
        "banned",
        "account_block",
        "account blocked",
        "封禁",
        "封号",
        "被封",
        "账户已封禁",
        "账号异常",
    ]
    .iter()
    .any(|keyword| lowered.contains(keyword))
    {
        return ("account_suspended", "账号封禁");
    }
    ("account_blocked", "账号异常")
}

fn parse_tagged_reason_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    let tag = trimmed.get(1..end)?.trim();
    if tag.is_empty() || !tag.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') {
        return None;
    }
    let detail = trimmed
        .get(end + 1..)
        .unwrap_or_default()
        .trim()
        .to_string();
    Some((tag.to_string(), detail))
}

fn extract_tagged_reason_sections(reason: &str) -> Vec<(String, String)> {
    let mut sections = Vec::<(String, String)>::new();
    let mut current_tag = None::<String>;
    for line in reason.lines() {
        if let Some((tag, detail)) = parse_tagged_reason_line(line) {
            current_tag = Some(tag.clone());
            if sections.iter().all(|(existing, _)| existing != &tag) {
                sections.push((tag, detail));
            }
            continue;
        }
        let continuation = line.trim();
        if continuation.is_empty() {
            continue;
        }
        let Some(tag) = current_tag.as_ref() else {
            continue;
        };
        let Some((_, detail)) = sections.iter_mut().find(|(existing, _)| existing == tag) else {
            continue;
        };
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(continuation);
    }
    sections
}

fn tagged_reason(reason: &str, tag: &str) -> Option<String> {
    extract_tagged_reason_sections(reason)
        .into_iter()
        .find_map(|(candidate, detail)| (candidate == tag).then_some(detail))
        .and_then(|detail| clean_text(Some(detail.as_str())).or(Some(detail)))
}

fn metadata_sources<'a>(
    provider_type: Option<&str>,
    upstream_metadata: Option<&'a Value>,
) -> Vec<&'a serde_json::Map<String, Value>> {
    let mut sources = Vec::new();
    let Some(root) = upstream_metadata.and_then(Value::as_object) else {
        return sources;
    };

    let normalized_provider_type = provider_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    if let Some(provider_type) = normalized_provider_type.as_deref() {
        if let Some(bucket) = root.get(provider_type).and_then(Value::as_object) {
            sources.push(bucket);
        }
        sources.push(root);
        return sources;
    }

    let mut seen = BTreeSet::new();
    for value in root.values() {
        let Some(object) = value.as_object() else {
            continue;
        };
        let pointer = object as *const serde_json::Map<String, Value> as usize;
        if seen.insert(pointer) {
            sources.push(object);
        }
    }
    sources.push(root);
    sources
}

fn resolve_from_metadata(
    provider_type: Option<&str>,
    upstream_metadata: Option<&Value>,
) -> Option<PoolAccountState> {
    for source in metadata_sources(provider_type, upstream_metadata) {
        if json_bool(source.get("is_banned")) || json_bool(source.get("banned")) {
            let reason = extract_reason(
                source,
                &["ban_reason", "forbidden_reason", "reason", "message"],
            )
            .unwrap_or_else(|| "账号已封禁".to_string());
            return Some(PoolAccountState {
                blocked: true,
                code: Some("account_banned".to_string()),
                label: Some("账号封禁".to_string()),
                reason: Some(reason),
                source: Some("metadata".to_string()),
                recoverable: false,
            });
        }
        if json_bool(source.get("is_quarantined")) || json_bool(source.get("quarantined")) {
            let reason = extract_reason(source, &["quarantine_reason", "reason", "message"])
                .unwrap_or_else(|| "账号处于隔离状态".to_string());
            return Some(PoolAccountState {
                blocked: true,
                code: Some("account_quarantined".to_string()),
                label: Some("账号隔离".to_string()),
                reason: Some(reason),
                source: Some("metadata".to_string()),
                recoverable: false,
            });
        }
        if json_bool(source.get("is_forbidden")) || json_bool(source.get("account_disabled")) {
            let reason = extract_reason(
                source,
                &["forbidden_reason", "ban_reason", "reason", "message"],
            );
            if looks_like_workspace_deactivated(reason.as_deref()) {
                return Some(PoolAccountState {
                    blocked: true,
                    code: Some("workspace_deactivated".to_string()),
                    label: Some("工作区停用".to_string()),
                    reason: Some(reason.unwrap_or_else(|| "工作区已停用".to_string())),
                    source: Some("metadata".to_string()),
                    recoverable: false,
                });
            }
            return Some(PoolAccountState {
                blocked: true,
                code: Some("account_forbidden".to_string()),
                label: Some("访问受限".to_string()),
                reason: Some(reason.unwrap_or_else(|| "账号访问受限".to_string())),
                source: Some("metadata".to_string()),
                recoverable: false,
            });
        }
    }
    None
}

fn resolve_from_oauth_invalid_reason(reason: Option<&str>) -> Option<PoolAccountState> {
    let text = clean_text(reason)?;

    if let Some(cleaned) = tagged_reason(&text, "ACCOUNT_BLOCK") {
        let reason = if cleaned.is_empty() {
            "账号异常".to_string()
        } else {
            cleaned
        };
        let (code, label) = classify_block_reason(&reason);
        return Some(PoolAccountState {
            blocked: true,
            code: Some(code.to_string()),
            label: Some(label.to_string()),
            reason: Some(reason),
            source: Some("oauth_invalid".to_string()),
            recoverable: false,
        });
    }
    if let Some(cleaned) = tagged_reason(&text, "OAUTH_EXPIRED") {
        let reason = if cleaned.is_empty() {
            "OAuth Token 已过期".to_string()
        } else {
            cleaned
        };
        let (code, label) = oauth_token_account_status_parts(&reason);
        return Some(PoolAccountState {
            blocked: true,
            code: Some(code.to_string()),
            label: Some(label.to_string()),
            reason: Some(reason),
            source: Some("oauth_invalid".to_string()),
            recoverable: code == "oauth_token_expired",
        });
    }
    if let Some(cleaned) = tagged_reason(&text, "REQUEST_FAILED") {
        let reason = if cleaned.is_empty() {
            "账号状态检查失败".to_string()
        } else {
            cleaned
        };
        return Some(PoolAccountState {
            blocked: false,
            code: Some("oauth_request_failed".to_string()),
            label: Some("请求失败".to_string()),
            reason: Some(reason),
            source: Some("oauth_request".to_string()),
            recoverable: true,
        });
    }
    if tagged_reason(&text, "REFRESH_FAILED").is_some() {
        return None;
    }
    if text.starts_with('[') {
        return None;
    }

    let lowered = text.to_ascii_lowercase();
    if ACCOUNT_BLOCK_REASON_KEYWORDS
        .iter()
        .any(|keyword| lowered.contains(keyword))
    {
        let (code, label) = classify_block_reason(&text);
        return Some(PoolAccountState {
            blocked: true,
            code: Some(code.to_string()),
            label: Some(label.to_string()),
            reason: Some(text),
            source: Some("oauth_invalid".to_string()),
            recoverable: false,
        });
    }
    None
}

pub fn resolve_pool_account_state(
    provider_type: Option<&str>,
    upstream_metadata: Option<&Value>,
    oauth_invalid_reason: Option<&str>,
) -> PoolAccountState {
    resolve_from_metadata(provider_type, upstream_metadata)
        .or_else(|| resolve_from_oauth_invalid_reason(oauth_invalid_reason))
        .unwrap_or_default()
}

pub fn resolve_account_status_snapshot(
    provider_type: Option<&str>,
    upstream_metadata: Option<&Value>,
    oauth_invalid_reason: Option<&str>,
) -> AccountStatusSnapshot {
    if let Some(metadata_state) = resolve_from_metadata(provider_type, upstream_metadata) {
        return AccountStatusSnapshot {
            code: metadata_state
                .code
                .unwrap_or_else(|| "account_blocked".to_string()),
            label: metadata_state.label,
            reason: metadata_state.reason,
            blocked: metadata_state.blocked,
            source: metadata_state.source,
            recoverable: metadata_state.recoverable,
        };
    }

    let Some(text) = clean_text(oauth_invalid_reason) else {
        return AccountStatusSnapshot::default();
    };

    if let Some(cleaned) = tagged_reason(&text, "ACCOUNT_BLOCK") {
        let reason = if cleaned.is_empty() {
            "账号异常".to_string()
        } else {
            cleaned
        };
        let (code, label) = classify_block_reason(&reason);
        return AccountStatusSnapshot {
            code: code.to_string(),
            label: Some(label.to_string()),
            reason: Some(reason),
            blocked: true,
            source: Some("oauth_invalid".to_string()),
            recoverable: false,
        };
    }

    if let Some(cleaned) = tagged_reason(&text, "OAUTH_EXPIRED") {
        let reason = if cleaned.is_empty() {
            "OAuth Token 已过期".to_string()
        } else {
            cleaned
        };
        let (code, label) = oauth_token_account_status_parts(&reason);
        return AccountStatusSnapshot {
            code: code.to_string(),
            label: Some(label.to_string()),
            reason: Some(reason),
            blocked: true,
            source: Some("oauth_invalid".to_string()),
            recoverable: code == "oauth_token_expired",
        };
    }

    if tagged_reason(&text, "REFRESH_FAILED").is_some() {
        return AccountStatusSnapshot::default();
    }

    if text.starts_with('[') {
        return AccountStatusSnapshot::default();
    }

    let lowered = text.to_ascii_lowercase();
    if ACCOUNT_BLOCK_REASON_KEYWORDS
        .iter()
        .any(|keyword| lowered.contains(keyword))
    {
        let (code, label) = classify_block_reason(&text);
        return AccountStatusSnapshot {
            code: code.to_string(),
            label: Some(label.to_string()),
            reason: Some(text),
            blocked: true,
            source: Some("oauth_invalid".to_string()),
            recoverable: false,
        };
    }

    AccountStatusSnapshot::default()
}

pub fn should_auto_remove_account_state(state: &PoolAccountState) -> bool {
    state.blocked
        && !state.recoverable
        && state.code.as_deref().is_some_and(|code| {
            AUTO_REMOVABLE_ACCOUNT_STATE_CODES
                .iter()
                .any(|candidate| code.eq_ignore_ascii_case(candidate))
        })
}

pub fn account_state_indicates_known_ban(state: &PoolAccountState) -> bool {
    if !state.blocked {
        return false;
    }
    if should_auto_remove_account_state(state) {
        return true;
    }
    if state
        .code
        .as_deref()
        .is_some_and(|code| matches!(code, "account_verification" | "account_blocked"))
    {
        return true;
    }
    state.code.as_deref().is_some_and(reason_indicates_ban)
        || state.reason.as_deref().is_some_and(reason_indicates_ban)
}

fn reason_indicates_ban(reason: &str) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && [
            "banned",
            "forbidden",
            "blocked",
            "suspend",
            "deactivated",
            "disabled",
            "verification",
            "workspace",
            "受限",
            "封",
            "禁",
        ]
        .iter()
        .any(|hint| normalized.contains(hint))
}

#[cfg(test)]
mod tests {
    use super::{
        account_state_indicates_known_ban, resolve_account_status_snapshot,
        resolve_pool_account_state, should_auto_remove_account_state,
    };
    use serde_json::json;

    #[test]
    fn resolves_workspace_deactivated_from_metadata() {
        let state = resolve_pool_account_state(
            Some("codex"),
            Some(&json!({
                "codex": {
                    "account_disabled": true,
                    "reason": "deactivated_workspace"
                }
            })),
            None,
        );

        assert!(state.blocked);
        assert_eq!(state.code.as_deref(), Some("workspace_deactivated"));
        assert_eq!(state.label.as_deref(), Some("工作区停用"));
        assert!(should_auto_remove_account_state(&state));
        assert!(account_state_indicates_known_ban(&state));
    }

    #[test]
    fn ignores_refresh_failed_as_pool_account_block() {
        let state = resolve_pool_account_state(
            Some("codex"),
            None,
            Some("[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已失效"),
        );

        assert!(!state.blocked);
        assert_eq!(state.code.as_deref(), None);
        assert_eq!(state.label.as_deref(), None);
        assert!(!should_auto_remove_account_state(&state));
    }

    #[test]
    fn resolves_windsurf_banned_and_quarantined_metadata_aliases() {
        let banned = resolve_pool_account_state(
            Some("windsurf"),
            Some(&json!({
                "windsurf": {
                    "banned": true,
                    "reason": "forbidden"
                }
            })),
            None,
        );
        assert!(banned.blocked);
        assert_eq!(banned.code.as_deref(), Some("account_banned"));

        let quarantined = resolve_pool_account_state(
            Some("windsurf"),
            Some(&json!({
                "windsurf": {
                    "quarantined": true
                }
            })),
            None,
        );
        assert!(quarantined.blocked);
        assert_eq!(quarantined.code.as_deref(), Some("account_quarantined"));
        assert!(should_auto_remove_account_state(&quarantined));
    }

    #[test]
    fn account_snapshot_ignores_refresh_failed_as_account_block() {
        let snapshot = resolve_account_status_snapshot(
            Some("codex"),
            None,
            Some("[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已失效"),
        );

        assert_eq!(snapshot.code, "ok");
        assert_eq!(snapshot.label.as_deref(), None);
        assert!(!snapshot.blocked);
        assert!(!snapshot.recoverable);
    }

    #[test]
    fn account_snapshot_marks_oauth_invalidated_as_token_invalid() {
        let snapshot = resolve_account_status_snapshot(
            Some("codex"),
            None,
            Some("[OAUTH_EXPIRED] Your authentication token has been invalidated. Please try signing in again."),
        );

        assert_eq!(snapshot.code, "oauth_token_invalid");
        assert_eq!(snapshot.label.as_deref(), Some("Token 失效"));
        assert!(snapshot.blocked);
        assert!(!snapshot.recoverable);
    }

    #[test]
    fn account_snapshot_marks_oauth_expired_as_token_expired() {
        let snapshot = resolve_account_status_snapshot(
            Some("codex"),
            None,
            Some("[OAUTH_EXPIRED] session expired"),
        );

        assert_eq!(snapshot.code, "oauth_token_expired");
        assert_eq!(snapshot.label.as_deref(), Some("Token 过期"));
        assert!(snapshot.blocked);
        assert!(snapshot.recoverable);
    }

    #[test]
    fn oauth_invalidated_state_is_auto_removed() {
        let state = resolve_pool_account_state(
            Some("codex"),
            None,
            Some("[OAUTH_EXPIRED] token invalidated"),
        );

        assert!(state.blocked);
        assert_eq!(state.code.as_deref(), Some("oauth_token_invalid"));
        assert!(should_auto_remove_account_state(&state));
    }

    #[test]
    fn oauth_expired_state_is_not_auto_removed() {
        let state = resolve_pool_account_state(
            Some("codex"),
            None,
            Some("[OAUTH_EXPIRED] session expired"),
        );

        assert!(state.blocked);
        assert_eq!(state.code.as_deref(), Some("oauth_token_expired"));
        assert!(!should_auto_remove_account_state(&state));
    }

    #[test]
    fn account_snapshot_detects_account_block_and_verification() {
        let snapshot = resolve_account_status_snapshot(
            Some("codex"),
            None,
            Some("[ACCOUNT_BLOCK] verify your account before continuing"),
        );

        assert_eq!(snapshot.code, "account_verification");
        assert_eq!(snapshot.label.as_deref(), Some("需要验证"));
        assert!(snapshot.blocked);
    }

    #[test]
    fn verification_state_is_not_auto_removed() {
        let state = resolve_pool_account_state(
            Some("codex"),
            None,
            Some("[ACCOUNT_BLOCK] verify your account before continuing"),
        );

        assert!(state.blocked);
        assert_eq!(state.code.as_deref(), Some("account_verification"));
        assert!(!should_auto_remove_account_state(&state));
        assert!(account_state_indicates_known_ban(&state));
    }
}
