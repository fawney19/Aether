pub(crate) mod shared;

mod catalog_routes;
mod external_cache;
mod global;
mod global_models;
mod payloads;
mod routing;
mod write;

pub(crate) use self::catalog_routes::maybe_build_local_admin_model_catalog_response;
pub(crate) use self::external_cache::{
    clear_admin_external_models_cache, read_admin_external_models_cache,
};
pub(crate) use self::global::{
    build_admin_global_model_payload, build_admin_global_model_providers_payload,
    build_admin_global_model_response, build_admin_global_models_payload,
    build_admin_model_catalog_payload, resolve_admin_global_model_by_id_or_err,
};
pub(crate) use self::global_models::maybe_build_local_admin_global_models_response;
pub(crate) use self::routing::{
    build_admin_assign_global_model_to_providers_payload, build_admin_global_model_routing_payload,
};
pub(crate) use self::write::{
    build_admin_global_model_create_record, build_admin_global_model_update_record,
};
