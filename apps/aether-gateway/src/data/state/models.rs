use super::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    DataLayerError, GatewayDataState, PublicCatalogModelListQuery, PublicCatalogModelSearchQuery,
    PublicGlobalModelQuery, StoredAdminGlobalModel, StoredAdminGlobalModelPage,
    StoredAdminProviderModel, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateRowsByKeyIdsQuery, StoredPoolKeyCandidateRowsQuery,
    StoredProviderActiveGlobalModel, StoredProviderModelStats, StoredPublicCatalogModel,
    StoredPublicGlobalModel, StoredPublicGlobalModelPage, StoredRequestedModelCandidateRowsQuery,
    UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};

impl GatewayDataState {
    pub(crate) async fn list_minimal_candidate_selection_rows(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "candidate_selection",
            self.database_pool_summary(),
            async {
                match &self.minimal_candidate_selection_reader {
                    Some(repository) => {
                        repository
                            .list_for_exact_api_format_and_global_model(
                                api_format,
                                global_model_name,
                            )
                            .await
                    }
                    None => Ok(Vec::new()),
                }
            },
        )
        .await
    }

    pub(crate) async fn list_minimal_candidate_selection_rows_for_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "candidate_selection",
            self.database_pool_summary(),
            async {
                match &self.minimal_candidate_selection_reader {
                    Some(repository) => {
                        repository
                            .list_for_exact_api_format_and_requested_model(
                                api_format,
                                requested_model_name,
                            )
                            .await
                    }
                    None => Ok(Vec::new()),
                }
            },
        )
        .await
    }

    pub(crate) async fn list_minimal_candidate_selection_rows_for_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "candidate_selection",
            self.database_pool_summary(),
            async {
                match &self.minimal_candidate_selection_reader {
                    Some(repository) => {
                        repository
                            .list_for_exact_api_format_and_requested_model_page(query)
                            .await
                    }
                    None => Ok(Vec::new()),
                }
            },
        )
        .await
    }

    pub(crate) async fn list_minimal_candidate_selection_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        crate::request_diagnostics::observe_db_operation(
            "candidate_selection",
            self.database_pool_summary(),
            async {
                match &self.minimal_candidate_selection_reader {
                    Some(repository) => repository.list_for_exact_api_format(api_format).await,
                    None => Ok(Vec::new()),
                }
            },
        )
        .await
    }

    pub(crate) async fn list_pool_key_candidate_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        match &self.minimal_candidate_selection_reader {
            Some(repository) => repository.list_pool_key_rows_for_group(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_pool_key_candidate_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        match &self.minimal_candidate_selection_reader {
            Some(repository) => repository.list_pool_key_rows_for_group_key_ids(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_public_global_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.list_public_models(query).await,
            None => Ok(StoredPublicGlobalModelPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn get_public_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.get_public_model_by_name(model_name).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.list_public_catalog_models(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.search_public_catalog_models(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.list_admin_provider_models(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.list_admin_global_models(query).await,
            None => Ok(StoredAdminGlobalModelPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => {
                repository
                    .get_admin_provider_model(provider_id, model_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => {
                repository
                    .list_admin_provider_available_source_models(provider_id)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => {
                repository
                    .get_admin_global_model_by_id(global_model_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.get_admin_global_model_by_name(model_name).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => {
                repository
                    .list_admin_provider_models_by_global_model_id(global_model_id)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => repository.create_admin_provider_model(record).await,
            None => Ok(None),
        };
        if result.as_ref().is_ok_and(Option::is_some) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => repository.update_admin_provider_model(record).await,
            None => Ok(None),
        };
        if result.as_ref().is_ok_and(Option::is_some) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => {
                repository
                    .delete_admin_provider_model(provider_id, model_id)
                    .await
            }
            None => Ok(false),
        };
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => repository.create_admin_global_model(record).await,
            None => Ok(None),
        };
        if result.as_ref().is_ok_and(Option::is_some) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => repository.update_admin_global_model(record).await,
            None => Ok(None),
        };
        if result.as_ref().is_ok_and(Option::is_some) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let result = match &self.global_model_writer {
            Some(repository) => repository.delete_admin_global_model(global_model_id).await,
            None => Ok(false),
        };
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.clear_billing_model_context_cache();
        }
        result
    }

    pub(crate) async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => repository.list_provider_model_stats(provider_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        match &self.global_model_reader {
            Some(repository) => {
                repository
                    .list_active_global_model_ids_by_provider_ids(provider_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }
}
