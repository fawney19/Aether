use super::*;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyListQuery, StoredProviderCatalogKeyMaintenanceSummary,
    StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats,
};
use aether_data_contracts::DataLayerError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailedRead {
    Providers,
    Endpoints,
    Keys,
}

struct FailingSummaryRepository {
    inner: InMemoryProviderCatalogReadRepository,
    failed_read: FailedRead,
}

impl FailingSummaryRepository {
    fn check(&self, operation: FailedRead) -> Result<(), DataLayerError> {
        if self.failed_read == operation {
            return Err(DataLayerError::InvalidConfiguration(
                "injected summary read failure".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProviderCatalogReadRepository for FailingSummaryRepository {
    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        self.check(FailedRead::Providers)?;
        self.inner.list_providers(active_only).await
    }

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        self.check(FailedRead::Providers)?;
        self.inner.list_providers_by_ids(provider_ids).await
    }

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        self.inner.list_endpoints_by_ids(endpoint_ids).await
    }

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        self.check(FailedRead::Endpoints)?;
        self.inner
            .list_endpoints_by_provider_ids(provider_ids)
            .await
    }

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.inner.list_keys_by_ids(key_ids).await
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.inner.list_keys_by_provider_ids(provider_ids).await
    }

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        self.check(FailedRead::Keys)?;
        self.inner
            .list_key_summaries_by_provider_ids(provider_ids)
            .await
    }

    async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        self.inner
            .list_key_maintenance_summaries_by_provider_ids(provider_ids)
            .await
    }

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        self.inner.list_keys_page(query).await
    }

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        self.inner
            .list_key_stats_by_provider_ids(provider_ids)
            .await
    }
}

#[tokio::test]
async fn admin_provider_summary_health_read_errors_do_not_look_like_empty_accounts() {
    for failed_read in [
        FailedRead::Providers,
        FailedRead::Endpoints,
        FailedRead::Keys,
    ] {
        let repository = Arc::new(FailingSummaryRepository {
            inner: InMemoryProviderCatalogReadRepository::seed(
                vec![sample_provider("provider-openai", "openai", 10)],
                vec![sample_endpoint(
                    "endpoint-chat",
                    "provider-openai",
                    "openai:chat",
                    "https://api.openai.example",
                )],
                vec![
                    sample_key("key-observed", "provider-openai", "openai:chat", "test")
                        .with_health_fields(
                            Some(json!({"openai:chat": {"health_score": 1.0}})),
                            None,
                        ),
                ],
            ),
            failed_read,
        });
        let state = AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                repository,
            ));

        for uri in [
            "/api/admin/providers/summary",
            "/api/admin/providers/summary?api_format=openai%3Achat",
            "/api/admin/providers/provider-openai/summary",
        ] {
            let response =
                local_admin_providers_response(&state, http::Method::GET, uri, None).await;
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{failed_read:?}: {uri}"
            );
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("error body should read");
            let payload: serde_json::Value =
                serde_json::from_slice(&body).expect("error should parse");
            assert_eq!(payload["detail"], ADMIN_PROVIDERS_DATA_UNAVAILABLE_DETAIL);
            assert!(payload.get("items").is_none());
            assert!(payload.get("endpoint_health_details").is_none());
        }
    }
}

#[tokio::test]
async fn admin_provider_summary_health_missing_provider_remains_not_found() {
    let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![],
        vec![],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            repository,
        ));

    let response = local_admin_providers_response(
        &state,
        http::Method::GET,
        "/api/admin/providers/provider-missing/summary",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_provider_summary_health_counts_inherited_reverse_proxy_accounts() {
    for (provider_type, auth_type, api_format) in [
        ("codex", "oauth", "openai:responses"),
        ("kiro", "oauth", "claude:messages"),
        ("kiro", "bearer", "claude:messages"),
        ("gemini_cli", "oauth", "gemini:generate_content"),
        ("antigravity", "oauth", "gemini:generate_content"),
        ("vertex_ai", "service_account", "gemini:generate_content"),
        ("chatgpt_web", "oauth", "openai:chat"),
        ("chatgpt_web", "bearer", "openai:chat"),
        ("windsurf", "oauth", "openai:chat"),
    ] {
        for configured_formats in [None, Some(json!([]))] {
            for score in [None, Some(0.0), Some(0.75)] {
                let mut provider = sample_provider("provider-reverse", provider_type, 10);
                provider.provider_type = provider_type.to_string();
                let endpoint = sample_endpoint(
                    "endpoint-reverse",
                    &provider.id,
                    api_format,
                    "https://reverse.example",
                );
                let mut key = sample_key("key-reverse", &provider.id, api_format, "test");
                key.auth_type = auth_type.to_string();
                key.api_formats = configured_formats.clone();
                key.encrypted_api_key = Some("summary".to_string());
                key.encrypted_auth_config = Some("{}".to_string());
                key.health_by_format =
                    score.map(|score| json!({api_format: {"health_score": score}}));
                let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    vec![key],
                ));
                let state = AppState::new()
                    .expect("gateway should build")
                    .with_data_state_for_tests(
                        GatewayDataState::with_provider_catalog_reader_for_tests(repository),
                    );

                for uri in [
                    "/api/admin/providers/summary",
                    "/api/admin/providers/provider-reverse/summary",
                ] {
                    let response =
                        local_admin_providers_response(&state, http::Method::GET, uri, None).await;
                    assert_eq!(
                        response.status(),
                        StatusCode::OK,
                        "{provider_type}/{auth_type}: {uri}"
                    );
                    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .expect("summary body should read");
                    let payload: serde_json::Value =
                        serde_json::from_slice(&body).expect("summary should parse");
                    let summary = payload
                        .get("items")
                        .map(|items| &items[0])
                        .unwrap_or(&payload);
                    let detail = &summary["endpoint_health_details"][0];
                    assert_eq!(detail["total_keys"], 1, "{provider_type}/{auth_type}");
                    assert_eq!(detail["active_keys"], 1, "{provider_type}/{auth_type}");
                    assert_eq!(
                        detail["health_score"],
                        json!(score.unwrap_or(1.0)),
                        "{provider_type}/{auth_type}"
                    );
                    assert_eq!(summary["avg_health_score"], json!(score.unwrap_or(1.0)));
                }
            }
        }
    }
}
