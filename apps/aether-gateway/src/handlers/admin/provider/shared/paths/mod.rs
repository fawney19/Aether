mod crud;
mod endpoint_keys;
mod oauth;
mod ops;
mod strategy;

pub(crate) use self::crud::{
    admin_provider_assign_global_models_path, admin_provider_available_source_models_path,
    admin_provider_clear_pool_cooldown_parts, admin_provider_delete_task_parts,
    admin_provider_id_for_health_monitor, admin_provider_id_for_manage_path,
    admin_provider_id_for_mapping_preview, admin_provider_id_for_models_list,
    admin_provider_id_for_pool_status, admin_provider_id_for_summary,
    admin_provider_import_models_path, admin_provider_model_route_parts,
    admin_provider_models_batch_path, admin_provider_reset_pool_cost_parts,
    is_admin_providers_root,
};
pub(crate) use self::endpoint_keys::{
    admin_clear_oauth_invalid_key_id, admin_export_key_id, admin_provider_id_for_keys,
    admin_provider_id_for_refresh_quota, admin_reveal_key_id, admin_update_key_id,
};
pub(crate) use self::oauth::{
    admin_provider_oauth_batch_import_provider_id, admin_provider_oauth_batch_import_task_path,
    admin_provider_oauth_batch_import_task_provider_id, admin_provider_oauth_complete_key_id,
    admin_provider_oauth_complete_provider_id, admin_provider_oauth_device_authorize_provider_id,
    admin_provider_oauth_device_poll_provider_id, admin_provider_oauth_import_provider_id,
    admin_provider_oauth_refresh_key_id, admin_provider_oauth_start_key_id,
    admin_provider_oauth_start_provider_id,
};
pub(crate) use self::ops::{
    admin_provider_id_for_provider_ops_balance, admin_provider_id_for_provider_ops_checkin,
    admin_provider_id_for_provider_ops_config, admin_provider_id_for_provider_ops_connect,
    admin_provider_id_for_provider_ops_disconnect, admin_provider_id_for_provider_ops_status,
    admin_provider_id_for_provider_ops_verify, admin_provider_ops_action_route_parts,
    admin_provider_ops_architecture_id_from_path, is_admin_provider_ops_architectures_root,
    is_admin_provider_ops_batch_balance_root,
};
pub(crate) use self::strategy::{
    admin_provider_id_for_provider_strategy_billing, admin_provider_id_for_provider_strategy_quota,
    admin_provider_id_for_provider_strategy_stats, is_admin_provider_strategy_strategies_root,
};
