use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn upsert_gemini_file_mapping(
        &self,
        record: aether_data::repository::gemini_file_mappings::UpsertGeminiFileMappingRecord,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .upsert_gemini_file_mapping(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn upsert_gemini_file_mapping_if_owner_matches(
        &self,
        record: aether_data::repository::gemini_file_mappings::UpsertGeminiFileMappingRecord,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .upsert_gemini_file_mapping_if_owner_matches(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_gemini_file_mappings(
        &self,
        query: &aether_data::repository::gemini_file_mappings::GeminiFileMappingListQuery,
    ) -> Result<
        aether_data::repository::gemini_file_mappings::StoredGeminiFileMappingListPage,
        GatewayError,
    > {
        self.data
            .list_gemini_file_mappings(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_gemini_file_mapping_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .find_gemini_file_mapping_by_file_name(file_name)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_active_gemini_file_mapping_for_user(
        &self,
        file_name: &str,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .find_active_gemini_file_mapping_for_user(file_name, user_id, now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_active_gemini_file_mapping_for_owner(
        &self,
        file_name: &str,
        key_id: &str,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .find_active_gemini_file_mapping_for_owner(file_name, key_id, user_id, now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::gemini_file_mappings::GeminiFileMappingStats, GatewayError>
    {
        self.data
            .summarize_gemini_file_mappings(now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_gemini_file_mapping_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_gemini_file_mapping_by_file_name(file_name)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_gemini_file_mapping_by_file_name_for_user(
        &self,
        file_name: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_gemini_file_mapping_by_file_name_for_user(file_name, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_gemini_file_mapping_by_file_name_for_owner(
        &self,
        file_name: &str,
        key_id: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_gemini_file_mapping_by_file_name_for_owner(file_name, key_id, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_gemini_file_mapping_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<
        Option<aether_data::repository::gemini_file_mappings::StoredGeminiFileMapping>,
        GatewayError,
    > {
        self.data
            .delete_gemini_file_mapping_by_id(mapping_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_expired_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<usize, GatewayError> {
        self.data
            .delete_expired_gemini_file_mappings(now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn cache_set_string_with_ttl(
        &self,
        key: &str,
        value: &str,
        ttl_seconds: u64,
    ) -> Result<(), GatewayError> {
        self.runtime_state
            .kv_set(
                key,
                value.to_string(),
                Some(std::time::Duration::from_secs(ttl_seconds)),
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn cache_delete_key(&self, key: &str) -> Result<(), GatewayError> {
        self.runtime_state
            .kv_delete(key)
            .await
            .map(|_| ())
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
