use super::{AdminAppState, AdminRequestContext};
use crate::{AppState, GatewayError};
use axum::body::{Body, Bytes};
use axum::http::Response;

pub(crate) enum AdminCancelVideoTaskError {
    NotFound,
    InvalidStatus(aether_data_contracts::repository::video_tasks::VideoTaskStatus),
    Response(Response<Body>),
    Gateway(GatewayError),
}

#[derive(Clone, Copy)]
pub(crate) struct AdminRouteRequest<'a> {
    state: AdminAppState<'a>,
    request_context: AdminRequestContext<'a>,
    request_body: Option<&'a Bytes>,
}

impl<'a> AdminRouteRequest<'a> {
    pub(crate) fn new(
        state: &'a AppState,
        request_context: &'a crate::control::GatewayPublicRequestContext,
        request_body: Option<&'a Bytes>,
    ) -> Self {
        Self {
            state: AdminAppState::new(state),
            request_context: AdminRequestContext::new(request_context),
            request_body,
        }
    }

    pub(crate) fn state(self) -> AdminAppState<'a> {
        self.state
    }

    pub(crate) fn request_context(self) -> AdminRequestContext<'a> {
        self.request_context
    }

    pub(crate) fn request_body(self) -> Option<&'a Bytes> {
        self.request_body
    }
}
