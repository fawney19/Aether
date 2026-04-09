use super::{api_keys, ldap, oauth_routes, security};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_auth_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    if let Some(response) = security::maybe_build_local_admin_security_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = api_keys::maybe_build_local_admin_api_keys_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = ldap::maybe_build_local_admin_ldap_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = oauth_routes::maybe_build_local_admin_oauth_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
