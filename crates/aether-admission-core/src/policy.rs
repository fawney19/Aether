use crate::{ResourceBudget, ResourceClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionRequest<'a> {
    pub trace_id: &'a str,
    pub class: ResourceClass,
    pub body_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    Admit(ResourceBudget),
    Reject(AdmissionRejectReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRejectReason {
    InvalidRequest,
    BodyTooLarge,
    ResourceSaturated,
    QueueDeadlineExceeded,
}

pub trait AdmissionPolicy: Send + Sync {
    fn decide(&self, request: AdmissionRequest<'_>) -> AdmissionDecision;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultAdmissionPolicy;

impl AdmissionPolicy for DefaultAdmissionPolicy {
    fn decide(&self, request: AdmissionRequest<'_>) -> AdmissionDecision {
        if request.trace_id.trim().is_empty() {
            return AdmissionDecision::Reject(AdmissionRejectReason::InvalidRequest);
        }
        let budget = ResourceBudget::for_class(request.class);
        if budget.body_bytes > 0 && request.body_bytes > budget.body_bytes {
            return AdmissionDecision::Reject(AdmissionRejectReason::BodyTooLarge);
        }
        AdmissionDecision::Admit(budget)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionDecision, AdmissionPolicy, AdmissionRejectReason, AdmissionRequest,
        DefaultAdmissionPolicy,
    };
    use crate::ResourceClass;

    #[test]
    fn default_policy_rejects_empty_trace_ids() {
        let decision = DefaultAdmissionPolicy.decide(AdmissionRequest {
            trace_id: " ",
            class: ResourceClass::Interactive,
            body_bytes: 0,
        });
        assert_eq!(
            decision,
            AdmissionDecision::Reject(AdmissionRejectReason::InvalidRequest)
        );
    }
}
