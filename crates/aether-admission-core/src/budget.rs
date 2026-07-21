use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbClass {
    None,
    ForegroundRead,
    ForegroundWrite,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedisLane {
    None,
    Fast,
    Stream,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceClass {
    Interactive,
    Streaming,
    Upload,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub request_permits: usize,
    pub body_bytes: usize,
    pub db_class: DbClass,
    pub redis_lane: RedisLane,
    pub upstream_permits: usize,
    pub stream_permits: usize,
}

impl ResourceBudget {
    pub const fn interactive() -> Self {
        Self {
            request_permits: 1,
            body_bytes: 0,
            db_class: DbClass::ForegroundRead,
            redis_lane: RedisLane::Fast,
            upstream_permits: 1,
            stream_permits: 0,
        }
    }

    pub const fn streaming() -> Self {
        Self {
            request_permits: 1,
            body_bytes: 0,
            db_class: DbClass::ForegroundWrite,
            redis_lane: RedisLane::Fast,
            upstream_permits: 1,
            stream_permits: 1,
        }
    }

    pub const fn for_class(class: ResourceClass) -> Self {
        match class {
            ResourceClass::Interactive => Self::interactive(),
            ResourceClass::Streaming => Self::streaming(),
            ResourceClass::Upload => Self {
                body_bytes: 64 * 1024 * 1024,
                ..Self::interactive()
            },
            ResourceClass::Background => Self {
                request_permits: 1,
                body_bytes: 0,
                db_class: DbClass::Background,
                redis_lane: RedisLane::Admin,
                upstream_permits: 0,
                stream_permits: 0,
            },
        }
    }

    pub fn validate(&self) -> Result<(), AdmissionConfigError> {
        if self.request_permits == 0 {
            return Err(AdmissionConfigError::ZeroRequestPermits);
        }
        if self.upstream_permits == 0 && self.stream_permits > 0 {
            return Err(AdmissionConfigError::StreamWithoutUpstream);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdmissionConfigError {
    #[error("admission budget must reserve at least one request permit")]
    ZeroRequestPermits,
    #[error("stream permits require an upstream permit")]
    StreamWithoutUpstream,
}

#[cfg(test)]
mod tests {
    use super::ResourceBudget;

    #[test]
    fn streaming_budget_has_valid_upstream_shape() {
        assert!(ResourceBudget::streaming().validate().is_ok());
    }
}
