use super::*;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn read_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<AdminGatewayProviderTransportSnapshot>, GatewayError> {
        self.app
            .read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
            .await
    }

    pub(crate) async fn resolve_local_oauth_request_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<crate::provider_transport::LocalResolvedOAuthRequestAuth>, GatewayError>
    {
        self.app.resolve_local_oauth_request_auth(transport).await
    }

    pub(crate) async fn resolve_local_oauth_header_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<(String, String)>, GatewayError> {
        Ok(
            match self.resolve_local_oauth_request_auth(transport).await? {
                Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name,
                    value,
                }) => Some((name, value)),
                _ => None,
            },
        )
    }

    pub(crate) async fn resolve_local_oauth_kiro_request_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<AdminKiroRequestAuth>, GatewayError> {
        Ok(
            match self.resolve_local_oauth_request_auth(transport).await? {
                Some(crate::provider_transport::LocalResolvedOAuthRequestAuth::Kiro(auth)) => {
                    Some(auth)
                }
                _ => None,
            },
        )
    }

    pub(crate) fn resolve_local_antigravity_identity_headers(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<(String, BTreeMap<String, String>)> {
        match crate::provider_transport::antigravity::resolve_local_antigravity_request_auth(
            transport,
        ) {
            crate::provider_transport::antigravity::AntigravityRequestAuthSupport::Supported(
                auth,
            ) => Some((
                auth.project_id.clone(),
                crate::provider_transport::antigravity::build_antigravity_static_identity_headers(
                    &auth,
                ),
            )),
            crate::provider_transport::antigravity::AntigravityRequestAuthSupport::Unsupported(
                _,
            ) => None,
        }
    }

    pub(crate) async fn resolve_transport_proxy_snapshot_with_tunnel_affinity(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<aether_contracts::ProxySnapshot> {
        crate::provider_transport::resolve_transport_proxy_snapshot_with_tunnel_affinity(
            self.app, transport,
        )
        .await
    }

    pub(crate) fn fixed_provider_template(
        &self,
        provider_type: &str,
    ) -> Option<(&'static str, &'static [&'static str])> {
        crate::provider_transport::provider_types::fixed_provider_template(provider_type)
    }

    pub(crate) fn provider_type_is_fixed(&self, provider_type: &str) -> bool {
        crate::provider_transport::provider_types::provider_type_is_fixed(provider_type)
    }

    pub(crate) fn provider_type_enables_format_conversion_by_default(
        &self,
        provider_type: &str,
    ) -> bool {
        crate::provider_transport::provider_types::provider_type_enables_format_conversion_by_default(
            provider_type,
        )
    }

    pub(crate) fn supports_local_gemini_transport_with_network(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
        api_format: &str,
    ) -> bool {
        crate::provider_transport::policy::supports_local_gemini_transport_with_network(
            transport, api_format,
        )
    }

    pub(crate) fn resolve_local_gemini_auth(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<(String, String)> {
        crate::provider_transport::auth::resolve_local_gemini_auth(transport)
    }

    pub(crate) fn build_passthrough_headers_with_auth(
        &self,
        headers: &axum::http::HeaderMap,
        auth_header: &str,
        auth_value: &str,
        extra_headers: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        crate::provider_transport::auth::build_passthrough_headers_with_auth(
            headers,
            auth_header,
            auth_value,
            extra_headers,
        )
    }

    pub(crate) fn apply_local_header_rules(
        &self,
        headers: &mut BTreeMap<String, String>,
        rules: Option<&serde_json::Value>,
        protected_keys: &[&str],
        body: &serde_json::Value,
        original_body: Option<&serde_json::Value>,
    ) -> bool {
        crate::provider_transport::apply_local_header_rules(
            headers,
            rules,
            protected_keys,
            body,
            original_body,
        )
    }

    pub(crate) fn build_gemini_files_passthrough_url(
        &self,
        upstream_base_url: &str,
        path: &str,
        query: Option<&str>,
    ) -> Option<String> {
        crate::provider_transport::url::build_gemini_files_passthrough_url(
            upstream_base_url,
            path,
            query,
        )
    }

    pub(crate) fn resolve_transport_tls_profile(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<String> {
        crate::provider_transport::resolve_transport_tls_profile(transport)
    }

    pub(crate) fn resolve_transport_execution_timeouts(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Option<aether_contracts::ExecutionTimeouts> {
        crate::provider_transport::resolve_transport_execution_timeouts(transport)
    }

    pub(crate) fn build_passthrough_path_url(
        &self,
        upstream_base_url: &str,
        path: &str,
        query: Option<&str>,
        blocked_keys: &[&str],
    ) -> Option<String> {
        crate::provider_transport::url::build_passthrough_path_url(
            upstream_base_url,
            path,
            query,
            blocked_keys,
        )
    }

    pub(crate) fn build_claude_messages_url(
        &self,
        upstream_base_url: &str,
        query: Option<&str>,
    ) -> String {
        crate::provider_transport::url::build_claude_messages_url(upstream_base_url, query)
    }

    pub(crate) fn build_gemini_content_url(
        &self,
        upstream_base_url: &str,
        model: &str,
        stream: bool,
        query: Option<&str>,
    ) -> Option<String> {
        crate::provider_transport::url::build_gemini_content_url(
            upstream_base_url,
            model,
            stream,
            query,
        )
    }

    pub(crate) fn build_openai_chat_url(
        &self,
        upstream_base_url: &str,
        query: Option<&str>,
    ) -> String {
        crate::provider_transport::url::build_openai_chat_url(upstream_base_url, query)
    }
}
