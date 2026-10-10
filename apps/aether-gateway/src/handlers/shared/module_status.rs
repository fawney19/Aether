use crate::handlers::shared::system_config_bool;
use crate::important_notification::{
    IMPORTANT_NOTIFICATION_ENABLED_KEY, LEGACY_NOTIFICATION_EMAIL_ENABLED_KEY,
};
use crate::{AppState, GatewayError};

pub(crate) const REFERRAL_ENABLED_CONFIG_KEY: &str = "referral_enabled";

pub(crate) fn module_enabled_config_key(module_name: &str) -> String {
    match module_name {
        "model_directives" => {
            crate::system_features::ENABLE_MODEL_DIRECTIVES_CONFIG_KEY.to_string()
        }
        "important_notification" => IMPORTANT_NOTIFICATION_ENABLED_KEY.to_string(),
        "s3_backup" => crate::backup::S3_BACKUP_ENABLED_KEY.to_string(),
        // 邀请返利入口与实际奖励共用系统设置，避免两个开关互相脱节。
        "referral" => REFERRAL_ENABLED_CONFIG_KEY.to_string(),
        _ => format!("module.{module_name}.enabled"),
    }
}

pub(crate) async fn read_module_enabled(
    state: &AppState,
    module_name: &str,
    available: bool,
) -> Result<bool, GatewayError> {
    if !available {
        return Ok(false);
    }
    let enabled_value = state
        .read_system_config_json_value(&module_enabled_config_key(module_name))
        .await?;
    let enabled_value = if module_name == "important_notification" && enabled_value.is_none() {
        state
            .read_system_config_json_value(LEGACY_NOTIFICATION_EMAIL_ENABLED_KEY)
            .await?
    } else {
        enabled_value
    };
    Ok(system_config_bool(enabled_value.as_ref(), false))
}
