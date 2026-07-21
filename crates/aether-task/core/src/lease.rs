use crate::FencingToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLease {
    pub task_key: String,
    pub owner: String,
    pub fencing_token: String,
    pub expires_at_unix_ms: u64,
}

impl TaskLease {
    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_at_unix_ms
    }

    pub fn parsed_fencing_token(&self) -> Result<FencingToken, TaskLeaseError> {
        self.fencing_token
            .parse::<u64>()
            .ok()
            .and_then(FencingToken::new)
            .ok_or(TaskLeaseError::InvalidFencingToken)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaskLeaseError {
    #[error("task lease is unavailable")]
    Unavailable,
    #[error("task lease has expired")]
    Expired,
    #[error("task fencing token is invalid")]
    InvalidFencingToken,
}

#[cfg(test)]
mod tests {
    use super::TaskLease;
    use crate::{RetryPolicy, TaskDefinition, TaskKind};

    #[test]
    fn singleton_definition_expiry_and_fencing_are_explicit() {
        let definition = TaskDefinition::singleton(
            "cleanup",
            TaskKind::Scheduled,
            "interval",
            true,
            RetryPolicy::default(),
        );
        assert!(definition.singleton);
        let lease = TaskLease {
            task_key: "cleanup".to_string(),
            owner: "node-a".to_string(),
            fencing_token: "1".to_string(),
            expires_at_unix_ms: 10,
        };
        assert!(lease.is_expired(10));
        assert_eq!(lease.parsed_fencing_token().unwrap().get(), 1);
    }
}
