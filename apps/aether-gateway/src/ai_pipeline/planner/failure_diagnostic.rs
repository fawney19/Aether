use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateFailureDiagnosticKind {
    RequestBodyBuild,
    RequestConversion,
    BodyRules,
    HeaderRules,
    UrlBuild,
    TransportAuth,
    EnvelopeBuild,
}

impl CandidateFailureDiagnosticKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RequestBodyBuild => "request_body_build",
            Self::RequestConversion => "request_conversion",
            Self::BodyRules => "body_rules",
            Self::HeaderRules => "header_rules",
            Self::UrlBuild => "url_build",
            Self::TransportAuth => "transport_auth",
            Self::EnvelopeBuild => "envelope_build",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateFailureDiagnostic {
    pub(crate) kind: CandidateFailureDiagnosticKind,
    pub(crate) path: String,
    pub(crate) message: String,
    pub(crate) source: Option<String>,
    pub(crate) client_api_format: Option<String>,
    pub(crate) provider_api_format: Option<String>,
    pub(crate) safe_to_show: bool,
}

impl CandidateFailureDiagnostic {
    pub(crate) fn new(
        kind: CandidateFailureDiagnosticKind,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            path: path.into(),
            message: message.into(),
            source: None,
            client_api_format: None,
            provider_api_format: None,
            safe_to_show: true,
        }
    }

    pub(crate) fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub(crate) fn formats(
        mut self,
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
    ) -> Self {
        self.client_api_format = Some(client_api_format.into());
        self.provider_api_format = Some(provider_api_format.into());
        self
    }

    pub(crate) fn to_extra_data(&self) -> Value {
        let diagnostic = self.to_value();
        let mut extra_data = json!({
            "failure_diagnostic": diagnostic,
        });

        // Compatibility for current usage UI and already persisted trace readers.
        if self.kind == CandidateFailureDiagnosticKind::RequestBodyBuild {
            if let Some(object) = extra_data.as_object_mut() {
                object.insert(
                    "request_body_build_error".to_string(),
                    json!({
                        "path": self.path,
                        "message": self.message,
                        "client_api_format": self.client_api_format,
                        "provider_api_format": self.provider_api_format,
                    }),
                );
            }
        }

        extra_data
    }

    pub(crate) fn upstream_url_missing(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::UrlBuild,
            "$.endpoint",
            "无法构建上游请求地址；请检查 base_url、custom_path、API 格式和模型映射",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    pub(crate) fn header_rules_apply_failed(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::HeaderRules,
            "$.endpoint.header_rules",
            "Header 规则应用失败；请检查规则格式、条件配置，或是否试图覆盖受保护认证头",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    pub(crate) fn body_rules_apply_failed(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::BodyRules,
            "$.endpoint.body_rules",
            "Body 规则应用失败；请检查规则格式、条件配置，或规则输出是否仍是当前上游支持的请求体",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    pub(crate) fn body_rules_unsupported_for_binary_upload(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::BodyRules,
            "$.endpoint.body_rules",
            "二进制上传暂不支持本地应用 Body 规则；请移除该 Endpoint 的 Body 规则或改用 JSON 请求体",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    pub(crate) fn provider_request_body_missing(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::RequestBodyBuild,
            "$",
            "无法构建上游请求体；请检查请求体是否为支持的 JSON object，以及该任务类型必需字段是否存在且取值受支持",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    pub(crate) fn envelope_build_failed(
        client_api_format: impl Into<String>,
        provider_api_format: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self::new(
            CandidateFailureDiagnosticKind::EnvelopeBuild,
            "$",
            "无法构建上游请求封装；请检查该 Provider 的认证配置、模型映射、Endpoint Body 规则和当前请求体是否兼容",
        )
        .formats(client_api_format, provider_api_format)
        .source(source)
    }

    fn to_value(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "path": self.path,
            "message": self.message,
            "source": self.source,
            "client_api_format": self.client_api_format,
            "provider_api_format": self.provider_api_format,
            "safe_to_show": self.safe_to_show,
        })
    }
}
