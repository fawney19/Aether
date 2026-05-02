use crate::AppState;

mod auth;
mod candidate_runtime;
mod executor;
mod scheduler;
mod transport;

pub(crate) use self::auth::GatewayAuthApiKeySnapshot;
pub(crate) use self::transport::{GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth};

#[derive(Clone, Copy)]
pub(crate) struct PlannerAppState<'a> {
    app: &'a AppState,
}

impl<'a> PlannerAppState<'a> {
    pub(crate) fn new(app: &'a AppState) -> Self {
        Self { app }
    }

    pub(crate) fn app(self) -> &'a AppState {
        self.app
    }
}
