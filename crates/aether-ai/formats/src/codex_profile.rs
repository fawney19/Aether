use std::sync::{LazyLock, OnceLock, RwLock};

static OS_INFO: LazyLock<os_info::Info> = LazyLock::new(os_info::get);

/// 当前支持的 Codex 客户端类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexClientKind {
    Cli,
    Desktop,
}

/// Codex 上游请求使用的客户端画像。
///
/// 画像由网关后台任务更新，格式转换层只读取不可变快照，避免在请求路径执行网络操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexClientProfile {
    pub client_kind: CodexClientKind,
    pub codex_version: String,
    pub originator: String,
    pub user_agent: String,
}

impl CodexClientProfile {
    /// 从稳定版本号创建 CLI 画像；版本校验由发布检查器负责，构造器只拒绝明显非法值。
    pub fn cli(version: &str) -> Result<Self, &'static str> {
        let version = version.trim();
        if version.is_empty()
            || version.len() > 64
            || !version.bytes().all(|byte| (32..=126).contains(&byte))
        {
            return Err("invalid Codex CLI version");
        }
        let originator = "codex_cli_rs".to_owned();
        Ok(Self {
            client_kind: CodexClientKind::Cli,
            codex_version: version.to_owned(),
            // 按 CLI 格式使用当前网关的公开平台信息。无客户端终端时使用官方
            // unknown 标识，不复制调用方终端后缀、安装标识或个人身份。
            user_agent: format!(
                "{}/{} ({} {}; {}) unknown",
                originator,
                version,
                OS_INFO.os_type(),
                OS_INFO.version(),
                OS_INFO.architecture().unwrap_or(std::env::consts::ARCH),
            )
            .chars()
            .map(|ch| if matches!(ch, ' '..='~') { ch } else { '_' })
            .collect(),
            originator,
        })
    }
}

impl Default for CodexClientProfile {
    fn default() -> Self {
        // 最新已核验稳定版本；后台版本刷新继续作为版本真源。
        Self::cli("0.159.3").expect("built-in Codex CLI profile must be valid")
    }
}

static ACTIVE_PROFILE: OnceLock<RwLock<CodexClientProfile>> = OnceLock::new();

fn active_profile() -> &'static RwLock<CodexClientProfile> {
    ACTIVE_PROFILE.get_or_init(|| RwLock::new(CodexClientProfile::default()))
}

/// 返回当前画像的独立快照，调用方不会持有全局锁。
pub fn codex_client_profile() -> CodexClientProfile {
    active_profile()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 原子替换当前画像，并返回替换前的画像。
pub fn set_codex_client_profile(profile: CodexClientProfile) -> CodexClientProfile {
    let mut current = active_profile()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::replace(&mut *current, profile)
}

/// 发布一份新的 CLI 画像。
pub fn set_codex_cli_version(version: &str) -> Result<CodexClientProfile, &'static str> {
    let profile = CodexClientProfile::cli(version)?;
    Ok(set_codex_client_profile(profile))
}

/// 返回当前画像的 Codex Core 版本。
pub fn codex_client_version() -> String {
    codex_client_profile().codex_version
}

/// 返回当前画像的 User-Agent。
pub fn codex_client_user_agent() -> String {
    codex_client_profile().user_agent
}

/// 返回当前画像的 originator。
pub fn codex_client_originator() -> String {
    codex_client_profile().originator
}

#[cfg(test)]
mod tests {
    use super::{CodexClientKind, CodexClientProfile};

    #[test]
    fn cli_profile_derives_wire_identity_from_version() {
        let profile = CodexClientProfile::cli("0.200.1").expect("valid version");
        assert_eq!(profile.client_kind, CodexClientKind::Cli);
        assert_eq!(profile.originator, "codex_cli_rs");
        assert!(profile.user_agent.starts_with("codex_cli_rs/0.200.1 ("));
        assert!(profile.user_agent.ends_with(") unknown"));
        assert!(profile.user_agent.contains(std::env::consts::ARCH));
    }

    #[test]
    fn cli_profile_rejects_empty_or_control_values() {
        assert!(CodexClientProfile::cli("").is_err());
        assert!(CodexClientProfile::cli("0.1.0\nspoof").is_err());
    }
}
