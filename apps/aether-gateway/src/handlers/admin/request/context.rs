use crate::control::{GatewayControlDecision, GatewayPublicRequestContext};
use axum::http::Method;
use std::ops::Deref;

#[derive(Clone, Copy)]
pub(crate) struct AdminRequestContext<'a> {
    context: &'a GatewayPublicRequestContext,
}

impl<'a> AdminRequestContext<'a> {
    pub(crate) fn new(context: &'a GatewayPublicRequestContext) -> Self {
        Self { context }
    }

    pub(crate) fn decision(&self) -> Option<&GatewayControlDecision> {
        self.context.control_decision.as_ref()
    }

    pub(crate) fn route_family(&self) -> Option<&str> {
        self.decision()
            .and_then(|decision| decision.route_family.as_deref())
    }

    pub(crate) fn route_kind(&self) -> Option<&str> {
        self.decision()
            .and_then(|decision| decision.route_kind.as_deref())
    }

    pub(crate) fn method(&self) -> &Method {
        &self.context.request_method
    }

    pub(crate) fn path(&self) -> &str {
        &self.context.request_path
    }

    pub(crate) fn query_string(&self) -> Option<&str> {
        self.context.request_query_string.as_deref()
    }

    pub(crate) fn content_type(&self) -> Option<&str> {
        self.context.request_content_type.as_deref()
    }

    pub(crate) fn trace_id(&self) -> &str {
        self.context.trace_id.as_str()
    }

    pub(crate) fn public(&self) -> &GatewayPublicRequestContext {
        self.context
    }
}

impl<'a> Deref for AdminRequestContext<'a> {
    type Target = GatewayPublicRequestContext;

    fn deref(&self) -> &Self::Target {
        self.context
    }
}
