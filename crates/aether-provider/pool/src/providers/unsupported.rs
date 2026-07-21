use crate::provider::ProviderPoolAdapter;

#[derive(Debug, Clone, Copy)]
pub struct UnsupportedQuotaProviderPoolAdapter {
    provider_type: &'static str,
    quota_refresh_unsupported_message: &'static str,
}

impl UnsupportedQuotaProviderPoolAdapter {
    pub const fn new(
        provider_type: &'static str,
        quota_refresh_unsupported_message: &'static str,
    ) -> Self {
        Self {
            provider_type,
            quota_refresh_unsupported_message,
        }
    }
}

impl ProviderPoolAdapter for UnsupportedQuotaProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        self.provider_type
    }

    fn quota_refresh_unsupported_message(&self) -> String {
        self.quota_refresh_unsupported_message.to_string()
    }
}

pub const CLAUDE_CODE_PROVIDER_POOL_ADAPTER: UnsupportedQuotaProviderPoolAdapter =
    UnsupportedQuotaProviderPoolAdapter::new(
        "claude_code",
        "Claude Code 暂不支持自动刷新额度：上游没有稳定可用的账号额度查询接口",
    );

pub const VERTEX_AI_PROVIDER_POOL_ADAPTER: UnsupportedQuotaProviderPoolAdapter =
    UnsupportedQuotaProviderPoolAdapter::new(
        "vertex_ai",
        "Vertex AI 暂不支持自动刷新额度：额度属于 Google Cloud 项目/区域配额",
    );
