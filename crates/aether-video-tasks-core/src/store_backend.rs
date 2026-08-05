use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::{
    DoubaoVideoTaskSeed, GeminiVideoTaskSeed, LocalVideoTaskReadResponse,
    LocalVideoTaskRegistryMutation, LocalVideoTaskSnapshot, OpenAiVideoTaskSeed, VideoTaskRegistry,
    VideoTaskStore,
};

#[derive(Debug, Default)]
pub struct InMemoryVideoTaskStore {
    registry: Mutex<VideoTaskRegistry>,
}

#[derive(Debug)]
pub struct FileVideoTaskStore {
    path: PathBuf,
    registry: Mutex<VideoTaskRegistry>,
}

impl VideoTaskStore for InMemoryVideoTaskStore {
    fn insert(&self, snapshot: LocalVideoTaskSnapshot) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.insert(snapshot);
        }
    }

    fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_openai(task_id)
    }

    fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_gemini(short_id)
    }

    fn read_doubao(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_doubao(task_id)
    }

    fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_openai(task_id)
    }

    fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_gemini(short_id)
    }

    fn clone_doubao(&self, task_id: &str) -> Option<DoubaoVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_doubao(task_id)
    }

    fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot> {
        let Ok(registry) = self.registry.lock() else {
            return Vec::new();
        };
        registry.list_active_snapshots(limit)
    }

    fn apply_mutation(&self, mutation: LocalVideoTaskRegistryMutation) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.apply_mutation(mutation);
        }
    }

    fn project_openai(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        registry.project_openai(task_id, provider_body)
    }

    fn project_gemini(&self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        registry.project_gemini(short_id, provider_body)
    }

    fn project_doubao(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        registry.project_doubao(task_id, provider_body)
    }
}

impl FileVideoTaskStore {
    pub fn new(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let registry = Self::load_registry(&path)?;
        Ok(Self {
            path,
            registry: Mutex::new(registry),
        })
    }

    fn load_registry(path: &Path) -> std::io::Result<VideoTaskRegistry> {
        if !path.exists() {
            return Ok(VideoTaskRegistry::default());
        }
        let bytes = std::fs::read(path)?;
        if bytes.is_empty() {
            return Ok(VideoTaskRegistry::default());
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }

    fn persist_registry(&self, registry: &VideoTaskRegistry) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(registry)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let temp_path = self.path.with_extension("tmp");
        std::fs::write(&temp_path, bytes)?;
        std::fs::rename(temp_path, &self.path)?;
        Ok(())
    }

    fn mutate_registry(&self, mutator: impl FnOnce(&mut VideoTaskRegistry) -> bool) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        if !mutator(&mut registry) {
            return false;
        }
        self.persist_registry(&registry).is_ok()
    }
}

impl VideoTaskStore for FileVideoTaskStore {
    fn insert(&self, snapshot: LocalVideoTaskSnapshot) {
        let _ = self.mutate_registry(|registry| {
            registry.insert(snapshot);
            true
        });
    }

    fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_openai(task_id)
    }

    fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_gemini(short_id)
    }

    fn read_doubao(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_doubao(task_id)
    }

    fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_openai(task_id)
    }

    fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_gemini(short_id)
    }

    fn clone_doubao(&self, task_id: &str) -> Option<DoubaoVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_doubao(task_id)
    }

    fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot> {
        let Ok(registry) = self.registry.lock() else {
            return Vec::new();
        };
        registry.list_active_snapshots(limit)
    }

    fn apply_mutation(&self, mutation: LocalVideoTaskRegistryMutation) {
        let _ = self.mutate_registry(|registry| {
            registry.apply_mutation(mutation);
            true
        });
    }

    fn project_openai(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        self.mutate_registry(|registry| registry.project_openai(task_id, provider_body))
    }

    fn project_gemini(&self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        self.mutate_registry(|registry| registry.project_gemini(short_id, provider_body))
    }

    fn project_doubao(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        self.mutate_registry(|registry| registry.project_doubao(task_id, provider_body))
    }
}
