use std::path::PathBuf;
use std::sync::Arc;

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::video_tasks::StoredVideoTask;
use serde_json::{Map, Value};

use crate::{
    extract_gemini_short_id_from_cancel_path, extract_gemini_short_id_from_path,
    extract_openai_task_id_from_cancel_path, extract_openai_task_id_from_content_path,
    extract_openai_task_id_from_path, extract_openai_task_id_from_remix_path,
    resolve_local_video_registry_mutation, FileVideoTaskStore, InMemoryVideoTaskStore,
    LocalVideoTaskContentAction, LocalVideoTaskFollowUpPlan, LocalVideoTaskProjectionTarget,
    LocalVideoTaskReadRefreshPlan, LocalVideoTaskReadResponse, LocalVideoTaskSnapshot,
    LocalVideoTaskSuccessPlan, VideoTaskStore, VideoTaskTruthSourceMode,
};

#[derive(Debug)]
pub struct VideoTaskService {
    truth_source_mode: VideoTaskTruthSourceMode,
    store: Arc<dyn VideoTaskStore>,
}

impl VideoTaskService {
    pub fn new(mode: VideoTaskTruthSourceMode) -> Self {
        Self::with_store(mode, Arc::new(InMemoryVideoTaskStore::default()))
    }

    pub fn with_file_store(
        mode: VideoTaskTruthSourceMode,
        path: impl Into<PathBuf>,
        encryption_key: impl Into<String>,
    ) -> std::io::Result<Self> {
        Ok(Self::with_store(
            mode,
            Arc::new(FileVideoTaskStore::new(path, encryption_key)?),
        ))
    }

    fn with_store(mode: VideoTaskTruthSourceMode, store: Arc<dyn VideoTaskStore>) -> Self {
        Self {
            truth_source_mode: mode,
            store,
        }
    }

    pub fn with_truth_source_mode(&self, mode: VideoTaskTruthSourceMode) -> Self {
        Self {
            truth_source_mode: mode,
            store: self.store.clone(),
        }
    }

    pub fn is_rust_authoritative(&self) -> bool {
        self.truth_source_mode == VideoTaskTruthSourceMode::RustAuthoritative
    }

    pub fn truth_source_mode(&self) -> VideoTaskTruthSourceMode {
        self.truth_source_mode
    }

    pub fn prepare_sync_success(
        &self,
        report_kind: &str,
        provider_body: &Map<String, Value>,
        report_context: &Map<String, Value>,
        plan: &ExecutionPlan,
    ) -> Option<LocalVideoTaskSuccessPlan> {
        self.truth_source_mode.prepare_sync_success(
            report_kind,
            provider_body,
            report_context,
            plan,
        )
    }

    pub fn record_snapshot(&self, snapshot: LocalVideoTaskSnapshot) {
        self.store.insert(snapshot);
    }

    pub fn hydrate_from_stored_task(&self, task: &StoredVideoTask) -> bool {
        let Some(snapshot) = LocalVideoTaskSnapshot::from_stored_task(task) else {
            return false;
        };
        self.store.insert(snapshot);
        true
    }

    pub fn apply_finalize_mutation(&self, request_path: &str, report_kind: &str) {
        let Some(mutation) = resolve_local_video_registry_mutation(
            self.truth_source_mode,
            request_path,
            report_kind,
        ) else {
            return;
        };
        self.store.apply_mutation(mutation);
    }

    pub fn read_response(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Option<LocalVideoTaskReadResponse> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        match route_family {
            Some("openai") => extract_openai_task_id_from_path(request_path)
                .and_then(|task_id| self.store.read_openai(task_id)),
            Some("gemini") => extract_gemini_short_id_from_path(request_path)
                .and_then(|short_id| self.store.read_gemini(short_id)),
            _ => None,
        }
    }

    pub fn read_response_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> Option<LocalVideoTaskReadResponse> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        let snapshot = self.snapshot_for_route(route_family, request_path)?;
        snapshot
            .belongs_to_user(user_id)
            .then(|| snapshot.read_response())
    }

    pub fn snapshot_for_route(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Option<LocalVideoTaskSnapshot> {
        match route_family {
            Some("openai") => extract_openai_task_id_from_path(request_path)
                .or_else(|| extract_openai_task_id_from_cancel_path(request_path))
                .or_else(|| extract_openai_task_id_from_remix_path(request_path))
                .or_else(|| extract_openai_task_id_from_content_path(request_path))
                .and_then(|task_id| self.store.clone_openai(task_id))
                .map(LocalVideoTaskSnapshot::OpenAi),
            Some("gemini") => extract_gemini_short_id_from_path(request_path)
                .or_else(|| extract_gemini_short_id_from_cancel_path(request_path))
                .and_then(|short_id| self.store.clone_gemini(short_id))
                .map(LocalVideoTaskSnapshot::Gemini),
            _ => None,
        }
    }

    pub fn route_belongs_to_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
    ) -> bool {
        self.snapshot_for_route(route_family, request_path)
            .is_some_and(|snapshot| snapshot.belongs_to_user(user_id))
    }

    pub fn prepare_openai_content_stream_action(
        &self,
        request_path: &str,
        query_string: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskContentAction> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        let task_id = extract_openai_task_id_from_content_path(request_path)?;
        let seed = self.store.clone_openai(task_id)?;
        seed.build_content_stream_action(query_string, trace_id)
    }

    pub fn prepare_openai_content_stream_action_for_user(
        &self,
        request_path: &str,
        query_string: Option<&str>,
        trace_id: &str,
        user_id: &str,
    ) -> Option<LocalVideoTaskContentAction> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        let task_id = extract_openai_task_id_from_content_path(request_path)?;
        let seed = self.store.clone_openai(task_id)?;
        let snapshot = LocalVideoTaskSnapshot::OpenAi(seed.clone());
        if !snapshot.belongs_to_user(user_id) {
            return None;
        }
        seed.build_content_stream_action(query_string, trace_id)
    }

    pub fn snapshot_for_refresh_plan(
        &self,
        refresh_plan: &LocalVideoTaskReadRefreshPlan,
    ) -> Option<LocalVideoTaskSnapshot> {
        match &refresh_plan.projection_target {
            LocalVideoTaskProjectionTarget::OpenAi { task_id } => self
                .store
                .clone_openai(task_id)
                .map(LocalVideoTaskSnapshot::OpenAi),
            LocalVideoTaskProjectionTarget::Gemini { short_id } => self
                .store
                .clone_gemini(short_id)
                .map(LocalVideoTaskSnapshot::Gemini),
        }
    }

    pub fn project_openai_task_response(
        &self,
        task_id: &str,
        provider_body: &Map<String, Value>,
    ) -> bool {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return false;
        }
        self.store.project_openai(task_id, provider_body)
    }

    pub fn project_gemini_task_response(
        &self,
        short_id: &str,
        provider_body: &Map<String, Value>,
    ) -> bool {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return false;
        }
        self.store.project_gemini(short_id, provider_body)
    }

    pub fn prepare_read_refresh_sync_plan(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        trace_id: &str,
    ) -> Option<LocalVideoTaskReadRefreshPlan> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        let snapshot = self.snapshot_for_read_refresh_route(route_family, request_path)?;
        Self::build_read_refresh_sync_plan_from_snapshot(snapshot, trace_id)
    }

    pub fn prepare_read_refresh_sync_plan_for_user(
        &self,
        route_family: Option<&str>,
        request_path: &str,
        user_id: &str,
        trace_id: &str,
    ) -> Option<LocalVideoTaskReadRefreshPlan> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }
        let snapshot = self.snapshot_for_read_refresh_route(route_family, request_path)?;
        if !snapshot.belongs_to_user(user_id) {
            return None;
        }
        Self::build_read_refresh_sync_plan_from_snapshot(snapshot, trace_id)
    }

    fn snapshot_for_read_refresh_route(
        &self,
        route_family: Option<&str>,
        request_path: &str,
    ) -> Option<LocalVideoTaskSnapshot> {
        match route_family {
            Some("openai") => extract_openai_task_id_from_path(request_path)
                .and_then(|task_id| self.store.clone_openai(task_id))
                .map(LocalVideoTaskSnapshot::OpenAi),
            Some("gemini") => extract_gemini_short_id_from_path(request_path)
                .and_then(|short_id| self.store.clone_gemini(short_id))
                .map(LocalVideoTaskSnapshot::Gemini),
            _ => None,
        }
    }

    fn build_read_refresh_sync_plan_from_snapshot(
        snapshot: LocalVideoTaskSnapshot,
        trace_id: &str,
    ) -> Option<LocalVideoTaskReadRefreshPlan> {
        match snapshot {
            LocalVideoTaskSnapshot::OpenAi(seed) => Some(LocalVideoTaskReadRefreshPlan {
                plan: seed.build_get_follow_up_plan(trace_id)?,
                projection_target: LocalVideoTaskProjectionTarget::OpenAi {
                    task_id: seed.local_task_id.clone(),
                },
            }),
            LocalVideoTaskSnapshot::Gemini(seed) => Some(LocalVideoTaskReadRefreshPlan {
                plan: seed.build_get_follow_up_plan(trace_id)?,
                projection_target: LocalVideoTaskProjectionTarget::Gemini {
                    short_id: seed.local_short_id.clone(),
                },
            }),
        }
    }

    pub fn prepare_poll_refresh_batch(
        &self,
        limit: usize,
        trace_prefix: &str,
    ) -> Vec<LocalVideoTaskReadRefreshPlan> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative || limit == 0 {
            return Vec::new();
        }

        self.store
            .list_active_snapshots(limit)
            .into_iter()
            .enumerate()
            .filter_map(|(index, snapshot)| {
                let trace_id = format!("{trace_prefix}-{index}");
                match snapshot {
                    LocalVideoTaskSnapshot::OpenAi(seed) => Some(LocalVideoTaskReadRefreshPlan {
                        plan: seed.build_get_follow_up_plan(&trace_id)?,
                        projection_target: LocalVideoTaskProjectionTarget::OpenAi {
                            task_id: seed.local_task_id.clone(),
                        },
                    }),
                    LocalVideoTaskSnapshot::Gemini(seed) => Some(LocalVideoTaskReadRefreshPlan {
                        plan: seed.build_get_follow_up_plan(&trace_id)?,
                        projection_target: LocalVideoTaskProjectionTarget::Gemini {
                            short_id: seed.local_short_id.clone(),
                        },
                    }),
                }
            })
            .collect()
    }

    pub fn prepare_poll_refresh_plan_for_stored_task(
        &self,
        task: &StoredVideoTask,
        trace_id: &str,
    ) -> Option<LocalVideoTaskReadRefreshPlan> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }

        let snapshot = LocalVideoTaskSnapshot::from_stored_task(task)?;
        self.prepare_poll_refresh_plan_for_snapshot(snapshot, trace_id)
    }

    pub fn prepare_poll_refresh_plan_for_snapshot(
        &self,
        snapshot: LocalVideoTaskSnapshot,
        trace_id: &str,
    ) -> Option<LocalVideoTaskReadRefreshPlan> {
        if self.truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
            return None;
        }

        match snapshot {
            LocalVideoTaskSnapshot::OpenAi(seed) => Some(LocalVideoTaskReadRefreshPlan {
                plan: seed.build_get_follow_up_plan(trace_id)?,
                projection_target: LocalVideoTaskProjectionTarget::OpenAi {
                    task_id: seed.local_task_id.clone(),
                },
            }),
            LocalVideoTaskSnapshot::Gemini(seed) => Some(LocalVideoTaskReadRefreshPlan {
                plan: seed.build_get_follow_up_plan(trace_id)?,
                projection_target: LocalVideoTaskProjectionTarget::Gemini {
                    short_id: seed.local_short_id.clone(),
                },
            }),
        }
    }

    pub fn apply_read_refresh_projection(
        &self,
        refresh_plan: &LocalVideoTaskReadRefreshPlan,
        provider_body: &Map<String, Value>,
    ) -> bool {
        match &refresh_plan.projection_target {
            LocalVideoTaskProjectionTarget::OpenAi { task_id } => {
                self.project_openai_task_response(task_id, provider_body)
            }
            LocalVideoTaskProjectionTarget::Gemini { short_id } => {
                self.project_gemini_task_response(short_id, provider_body)
            }
        }
    }

    pub fn prepare_follow_up_sync_plan(
        &self,
        plan_kind: &str,
        request_path: &str,
        body_json: Option<&Value>,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        let snapshot = self.snapshot_for_follow_up_route(plan_kind, request_path)?;
        Self::build_follow_up_plan_from_snapshot(
            snapshot,
            plan_kind,
            body_json,
            fallback_user_id,
            fallback_api_key_id,
            trace_id,
        )
    }

    fn build_follow_up_plan_from_snapshot(
        snapshot: LocalVideoTaskSnapshot,
        plan_kind: &str,
        body_json: Option<&Value>,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        match (plan_kind, snapshot) {
            ("openai_video_remix_sync", LocalVideoTaskSnapshot::OpenAi(seed)) => seed
                .build_remix_follow_up_plan(
                    body_json?,
                    fallback_user_id,
                    fallback_api_key_id,
                    trace_id,
                ),
            ("openai_video_delete_sync", LocalVideoTaskSnapshot::OpenAi(seed)) => {
                seed.build_delete_follow_up_plan(fallback_user_id, fallback_api_key_id, trace_id)
            }
            ("openai_video_cancel_sync", LocalVideoTaskSnapshot::OpenAi(seed)) => {
                seed.build_cancel_follow_up_plan(fallback_user_id, fallback_api_key_id, trace_id)
            }
            ("gemini_video_cancel_sync", LocalVideoTaskSnapshot::Gemini(seed)) => {
                seed.build_cancel_follow_up_plan(fallback_user_id, fallback_api_key_id, trace_id)
            }
            _ => None,
        }
    }

    pub fn prepare_follow_up_sync_plan_for_user(
        &self,
        plan_kind: &str,
        request_path: &str,
        body_json: Option<&Value>,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        let snapshot = self.snapshot_for_follow_up_route(plan_kind, request_path)?;
        let user_id = fallback_user_id?.trim();
        if !snapshot.belongs_to_user(user_id) {
            return None;
        }
        Self::build_follow_up_plan_from_snapshot(
            snapshot,
            plan_kind,
            body_json,
            Some(user_id),
            fallback_api_key_id,
            trace_id,
        )
    }

    fn snapshot_for_follow_up_route(
        &self,
        plan_kind: &str,
        request_path: &str,
    ) -> Option<LocalVideoTaskSnapshot> {
        match plan_kind {
            "openai_video_remix_sync" => extract_openai_task_id_from_remix_path(request_path)
                .and_then(|task_id| self.store.clone_openai(task_id))
                .map(LocalVideoTaskSnapshot::OpenAi),
            "openai_video_delete_sync" => extract_openai_task_id_from_path(request_path)
                .and_then(|task_id| self.store.clone_openai(task_id))
                .map(LocalVideoTaskSnapshot::OpenAi),
            "openai_video_cancel_sync" => extract_openai_task_id_from_cancel_path(request_path)
                .and_then(|task_id| self.store.clone_openai(task_id))
                .map(LocalVideoTaskSnapshot::OpenAi),
            "gemini_video_cancel_sync" => extract_gemini_short_id_from_cancel_path(request_path)
                .and_then(|short_id| self.store.clone_gemini(short_id))
                .map(LocalVideoTaskSnapshot::Gemini),
            _ => None,
        }
    }
}
