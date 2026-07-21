use serde::{Deserialize, Serialize};

use crate::{UsageEventError, UsageEventKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEventEnvelope {
    pub event_id: String,
    pub request_id: String,
    pub subject_id: String,
    pub kind: UsageEventKind,
    pub sequence: u64,
}

impl UsageEventEnvelope {
    pub fn validate(&self) -> Result<(), UsageEventError> {
        if self.event_id.trim().is_empty() {
            return Err(UsageEventError::MissingEventId);
        }
        if self.request_id.trim().is_empty() {
            return Err(UsageEventError::MissingRequestId);
        }
        if self.subject_id.trim().is_empty() {
            return Err(UsageEventError::MissingSubjectId);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UsageEventEnvelope;
    use crate::UsageEventKind;

    #[test]
    fn envelope_rejects_missing_identity() {
        let event = UsageEventEnvelope {
            event_id: String::new(),
            request_id: "request".to_string(),
            subject_id: "user".to_string(),
            kind: UsageEventKind::Completed,
            sequence: 1,
        };
        assert!(event.validate().is_err());
    }
}
