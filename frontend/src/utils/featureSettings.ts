export interface ChatPiiRedactionFeatureSettings {
  enabled: boolean
  inject_model_instruction: boolean
}

export interface NotificationPushServiceFeatureSettings {
  enabled: boolean
}

export type FeatureSettingsMap = Record<string, unknown>

const DEFAULT_CHAT_PII_REDACTION_FEATURE_SETTINGS: ChatPiiRedactionFeatureSettings = {
  enabled: false,
  inject_model_instruction: true,
}

const DEFAULT_NOTIFICATION_PUSH_SERVICE_FEATURE_SETTINGS: NotificationPushServiceFeatureSettings = {
  enabled: false,
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
}

export function readChatPiiRedactionFeatureSettings(
  featureSettings: unknown,
): ChatPiiRedactionFeatureSettings {
  const feature = isRecord(featureSettings)
    ? featureSettings.chat_pii_redaction
    : null
  if (!isRecord(feature)) {
    return { ...DEFAULT_CHAT_PII_REDACTION_FEATURE_SETTINGS }
  }
  return {
    enabled: feature.enabled === true,
    inject_model_instruction: feature.inject_model_instruction !== false,
  }
}

export function hasChatPiiRedactionFeatureSettings(featureSettings: unknown): boolean {
  const feature = isRecord(featureSettings)
    ? featureSettings.chat_pii_redaction
    : null
  return isRecord(feature)
}

export function mergeChatPiiRedactionFeatureSettings(
  featureSettings: unknown,
  chatPiiRedaction: ChatPiiRedactionFeatureSettings,
): FeatureSettingsMap | null {
  const settings: FeatureSettingsMap = isRecord(featureSettings)
    ? { ...featureSettings }
    : {}
  settings.chat_pii_redaction = {
    enabled: chatPiiRedaction.enabled,
    inject_model_instruction: chatPiiRedaction.inject_model_instruction,
  }
  return Object.keys(settings).length > 0 ? settings : null
}

export function removeChatPiiRedactionFeatureSettings(
  featureSettings: unknown,
): FeatureSettingsMap | null {
  if (!isRecord(featureSettings)) return null
  const settings: FeatureSettingsMap = { ...featureSettings }
  delete settings.chat_pii_redaction
  return Object.keys(settings).length > 0 ? settings : null
}

export function readNotificationPushServiceFeatureSettings(
  featureSettings: unknown,
): NotificationPushServiceFeatureSettings {
  const feature = isRecord(featureSettings)
    ? featureSettings.notification_push_service
    : null
  if (!isRecord(feature)) {
    return { ...DEFAULT_NOTIFICATION_PUSH_SERVICE_FEATURE_SETTINGS }
  }
  return {
    enabled: feature.enabled === true,
  }
}

export function mergeNotificationPushServiceFeatureSettings(
  featureSettings: unknown,
  notificationPushService: NotificationPushServiceFeatureSettings,
): FeatureSettingsMap | null {
  const settings: FeatureSettingsMap = isRecord(featureSettings)
    ? { ...featureSettings }
    : {}
  settings.notification_push_service = {
    enabled: notificationPushService.enabled,
  }
  return Object.keys(settings).length > 0 ? settings : null
}
