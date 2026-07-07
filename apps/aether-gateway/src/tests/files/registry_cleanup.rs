use super::{AppState, Arc, InMemoryRequestCandidateRepository};
use aether_data::repository::gemini_file_mappings::{
    GeminiFileMappingReadRepository, InMemoryGeminiFileMappingRepository, StoredGeminiFileMapping,
};

#[test]
fn gateway_background_gemini_file_mapping_cleanup_deletes_expired_entries() {
    super::run_files_test(
        "gateway_background_gemini_file_mapping_cleanup_deletes_expired_entries",
        gateway_background_gemini_file_mapping_cleanup_deletes_expired_entries_impl,
    );
}

async fn gateway_background_gemini_file_mapping_cleanup_deletes_expired_entries_impl() {
    fn sample_mapping(
        id: &str,
        file_name: &str,
        expires_at_unix_secs: i64,
    ) -> StoredGeminiFileMapping {
        StoredGeminiFileMapping::new(
            id.to_string(),
            file_name.to_string(),
            "key-gemini-files-local-1".to_string(),
            1,
            expires_at_unix_secs,
        )
        .expect("mapping should build")
    }

    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let gemini_file_mapping_repository = Arc::new(InMemoryGeminiFileMappingRepository::seed(vec![
        sample_mapping("mapping-expired", "files/expired", 1),
        sample_mapping("mapping-active", "files/active", 4_102_444_800),
    ]));

    let gateway_state = AppState::new()
    .expect("gateway state should build")
    .with_data_state_for_tests(
        crate::data::GatewayDataState::with_request_candidate_and_gemini_file_mapping_repository_for_tests(
            request_candidate_repository,
            Arc::clone(&gemini_file_mapping_repository),
        ),
    );
    let background_tasks = gateway_state.spawn_background_tasks();
    assert!(!background_tasks.is_empty(), "cleanup worker should spawn");

    let expired_lookup_deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        if gemini_file_mapping_repository
            .find_by_file_name("files/expired")
            .await
            .expect("lookup should succeed")
            .is_none()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < expired_lookup_deadline,
            "cleanup worker did not delete expired mapping within 500ms"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        gemini_file_mapping_repository
            .find_by_file_name("files/expired")
            .await
            .expect("lookup should succeed")
            .is_none(),
        "expired mapping should be deleted"
    );
    assert!(
        gemini_file_mapping_repository
            .find_by_file_name("files/active")
            .await
            .expect("lookup should succeed")
            .is_some(),
        "active mapping should remain"
    );

    background_tasks.shutdown().await;
}
