use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingRulePhase {
    #[default]
    ClientRequest,
    ProviderRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSetPriorityMode {
    #[default]
    Provider,
    GlobalKey,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSchedulingMode {
    #[default]
    CacheAffinity,
    LoadBalance,
    FixedOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum RoutingJsonPatchOperation {
    Add { path: String, value: Value },
    Replace { path: String, value: Value },
    Remove { path: String },
}

impl RoutingJsonPatchOperation {
    pub fn path(&self) -> &str {
        match self {
            Self::Add { path, .. } | Self::Replace { path, .. } | Self::Remove { path } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum RoutingHeaderPatch {
    Set { name: String, value: String },
    Remove { name: String },
}

impl RoutingHeaderPatch {
    pub fn name(&self) -> &str {
        match self {
            Self::Set { name, .. } | Self::Remove { name } => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RoutingAction {
    RestrictModels {
        models: Vec<String>,
    },
    RestrictProviders {
        provider_ids: Vec<String>,
    },
    RestrictKeys {
        key_ids: Vec<String>,
    },
    SetScheduling {
        priority_mode: Option<RoutingSetPriorityMode>,
        scheduling_mode: Option<RoutingSchedulingMode>,
        keep_priority_on_conversion: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sticky_key_attempts: Option<u32>,
    },
    SetProviderPriority {
        provider_id: String,
        priority: i32,
    },
    SetKeyPriority {
        key_id: String,
        priority: i32,
        /// When set, the override only applies to candidates served through
        /// this API format; otherwise it applies to the key on every format.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_format: Option<String>,
    },
    JsonPatchBody {
        patch: Vec<RoutingJsonPatchOperation>,
    },
    PatchHeaders {
        patch: Vec<RoutingHeaderPatch>,
    },
}
