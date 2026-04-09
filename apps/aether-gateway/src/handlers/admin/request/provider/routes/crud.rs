use crate::handlers::admin::provider::crud::{delete_task, pool, reads, writes};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::AdminRequestContext;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

impl<'a> AdminAppState<'a> {
    pub(crate) async fn maybe_build_admin_provider_crud_route_response(
        &self,
        request_context: &AdminRequestContext<'_>,
        request_body: Option<&Bytes>,
    ) -> Result<Option<Response<Body>>, GatewayError> {
        let Some(decision) = request_context.decision() else {
            return Ok(None);
        };
        if decision.route_family.as_deref() != Some("providers_manage") {
            return Ok(None);
        }

        let route_kind = decision.route_kind.as_deref();

        if let Some(response) = writes::maybe_build_local_admin_provider_writes_response(
            self,
            request_context,
            request_body,
            route_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }
        if let Some(response) = reads::maybe_build_local_admin_provider_reads_response(
            self,
            request_context,
            route_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }
        if let Some(response) = delete_task::maybe_build_local_admin_provider_delete_task_response(
            self,
            request_context,
            route_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }
        if let Some(response) =
            pool::maybe_build_local_admin_provider_pool_response(self, request_context, route_kind)
                .await?
        {
            return Ok(Some(response));
        }

        Ok(None)
    }
}
