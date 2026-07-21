use serde::{Deserialize, Serialize};

use crate::RetryPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Scheduled,
    Daemon,
    OnDemand,
    FireAndForget,
}

impl TaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Daemon => "daemon",
            Self::OnDemand => "on_demand",
            Self::FireAndForget => "fire_and_forget",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDefinition {
    pub key: &'static str,
    pub kind: TaskKind,
    pub trigger: &'static str,
    pub singleton: bool,
    pub persist_history: bool,
    pub retry_policy: RetryPolicy,
}

impl TaskDefinition {
    pub const fn new(
        key: &'static str,
        kind: TaskKind,
        trigger: &'static str,
        singleton: bool,
        persist_history: bool,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            key,
            kind,
            trigger,
            singleton,
            persist_history,
            retry_policy,
        }
    }

    pub const fn singleton(
        key: &'static str,
        kind: TaskKind,
        trigger: &'static str,
        persist_history: bool,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self::new(key, kind, trigger, true, persist_history, retry_policy)
    }
}
