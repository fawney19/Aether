use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderCapability {
    Chat,
    Responses,
    Embeddings,
    Images,
    Video,
    Files,
}
