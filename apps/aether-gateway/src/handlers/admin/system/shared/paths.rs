pub(crate) fn is_admin_management_tokens_root(request_path: &str) -> bool {
    aether_admin::system::is_admin_management_tokens_root(request_path)
}

pub(crate) fn is_admin_system_configs_root(request_path: &str) -> bool {
    aether_admin::system::is_admin_system_configs_root(request_path)
}

pub(crate) fn is_admin_system_email_templates_root(request_path: &str) -> bool {
    aether_admin::system::is_admin_system_email_templates_root(request_path)
}

pub(crate) fn admin_system_config_key_from_path(request_path: &str) -> Option<String> {
    aether_admin::system::admin_system_config_key_from_path(request_path)
}

pub(crate) fn admin_system_email_template_type_from_path(request_path: &str) -> Option<String> {
    aether_admin::system::admin_system_email_template_type_from_path(request_path)
}

pub(crate) fn admin_system_email_template_preview_type_from_path(
    request_path: &str,
) -> Option<String> {
    aether_admin::system::admin_system_email_template_preview_type_from_path(request_path)
}

pub(crate) fn admin_system_email_template_reset_type_from_path(
    request_path: &str,
) -> Option<String> {
    aether_admin::system::admin_system_email_template_reset_type_from_path(request_path)
}

pub(crate) fn admin_management_token_id_from_path(request_path: &str) -> Option<String> {
    aether_admin::system::admin_management_token_id_from_path(request_path)
}

pub(crate) fn admin_management_token_status_id_from_path(request_path: &str) -> Option<String> {
    aether_admin::system::admin_management_token_status_id_from_path(request_path)
}
