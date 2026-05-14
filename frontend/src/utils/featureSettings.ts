export interface ChatPiiRedactionFeatureSettings {
  enabled: boolean
  inject_model_instruction: boolean
}

export type FeatureSettingsMap = Record<string, unknown>

const DEFAULT_CHAT_PII_REDACTION_FEATURE_SETTINGS: ChatPiiRedactionFeatureSettings = {
  enabled: false,
  inject_model_instruction: true,
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
