use serde_json::Value;

#[derive(Debug, Clone)]
pub(super) struct AdminProviderOAuthBatchImportProgress {
    pub total: usize,
    pub processed: usize,
    pub success: usize,
    pub failed: usize,
    pub created_count: usize,
    pub replaced_count: usize,
    pub latest_result: Option<Value>,
}

#[async_trait::async_trait]
pub(super) trait AdminProviderOAuthBatchProgressReporter: Send {
    async fn report(&mut self, progress: AdminProviderOAuthBatchImportProgress);
}

pub(super) async fn maybe_report_admin_provider_oauth_batch_import_progress(
    reporter: &mut Option<&mut dyn AdminProviderOAuthBatchProgressReporter>,
    total: usize,
    success: usize,
    failed: usize,
    results: &[Value],
) {
    let Some(reporter) = reporter.as_deref_mut() else {
        return;
    };
    let replaced_count = results
        .iter()
        .filter(|item| {
            item.get("status").and_then(Value::as_str) == Some("success")
                && item.get("replaced").and_then(Value::as_bool) == Some(true)
        })
        .count();
    reporter
        .report(AdminProviderOAuthBatchImportProgress {
            total,
            processed: success.saturating_add(failed).min(total),
            success,
            failed,
            created_count: success.saturating_sub(replaced_count),
            replaced_count,
            latest_result: results.last().cloned(),
        })
        .await;
}
