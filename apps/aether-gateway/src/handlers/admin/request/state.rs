use crate::{AppState, GatewayError};

#[derive(Clone, Copy)]
pub(crate) struct AdminAppState<'a> {
    pub(super) app: &'a AppState,
}

impl<'a> AdminAppState<'a> {
    pub(crate) fn new(app: &'a AppState) -> Self {
        Self { app }
    }

    pub(crate) fn app(&self) -> &AppState {
        self.app
    }

    pub(crate) fn cloned_app(&self) -> AppState {
        self.app.clone()
    }
}

impl<'a> AsRef<AppState> for AdminAppState<'a> {
    fn as_ref(&self) -> &AppState {
        self.app
    }
}

pub(crate) type AdminRouteResponse = axum::http::Response<axum::body::Body>;
pub(crate) type AdminRouteResult = Result<Option<AdminRouteResponse>, GatewayError>;
