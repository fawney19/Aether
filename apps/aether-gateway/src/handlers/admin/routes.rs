use super::{
    announcements, auth, billing, endpoint, features, model, observability, provider, referrals,
    request, risk_control, routing, system, users,
};

pub(crate) async fn maybe_build_local_admin_response(
    request: request::AdminRouteRequest<'_>,
) -> request::AdminRouteResult {
    if let Some(response) =
        announcements::maybe_build_local_admin_announcements_response(request).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = model::maybe_build_local_admin_model_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = provider::maybe_build_local_admin_provider_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = routing::maybe_build_local_admin_routing_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) =
        risk_control::maybe_build_local_admin_risk_control_response(request).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = auth::maybe_build_local_admin_auth_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = system::maybe_build_local_admin_system_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) =
        observability::maybe_build_local_admin_observability_response(request).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        billing::maybe_build_local_admin_billing_routes_response(request).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = referrals::maybe_build_local_admin_referrals_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = features::maybe_build_local_admin_features_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = users::maybe_build_local_admin_users_response(request).await? {
        return Ok(Some(response));
    }

    if let Some(response) = endpoint::maybe_build_local_admin_endpoints_response(request).await? {
        return Ok(Some(response));
    }

    Ok(None)
}
