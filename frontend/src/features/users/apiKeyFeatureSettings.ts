import type { FeatureSettings } from '@/api/users'
import {
  hasChatPiiRedactionFeatureSettings,
  mergeChatPiiRedactionFeatureSettings,
  readChatPiiRedactionFeatureSettings,
  removeChatPiiRedactionFeatureSettings,
  type ChatPiiRedactionFeatureSettings,
} from '@/utils/featureSettings'

export type ApiKeyRedactionMode = 'inherit' | 'custom'

export interface ApiKeyRedactionFormState extends ChatPiiRedactionFeatureSettings {
  mode: ApiKeyRedactionMode
}

export function resolveApiKeyRedactionFormState(
  apiKeyFeatureSettings: FeatureSettings | null | undefined,
  inheritedUserFeatureSettings: FeatureSettings | null | undefined,
): ApiKeyRedactionFormState {
  const hasCustomRedaction = hasChatPiiRedactionFeatureSettings(apiKeyFeatureSettings)
  const value = readChatPiiRedactionFeatureSettings(
    hasCustomRedaction ? apiKeyFeatureSettings : inheritedUserFeatureSettings,
  )
  return {
    mode: hasCustomRedaction ? 'custom' : 'inherit',
    ...value,
  }
}

/**
 * Builds only the feature-settings portion of an API-key mutation.
 *
 * An omitted field preserves inheritance. `null` (or an object with the
 * redaction key removed) is emitted only when an existing custom override is
 * explicitly switched back to inheritance.
 */
export function buildApiKeyRedactionFeatureSettingsPatch(options: {
  isEditing: boolean
  currentFeatureSettings: FeatureSettings | null | undefined
  mode: ApiKeyRedactionMode
  value: ChatPiiRedactionFeatureSettings
}): { feature_settings?: FeatureSettings | null } {
  if (options.mode === 'custom') {
    return {
      feature_settings: mergeChatPiiRedactionFeatureSettings(
        options.isEditing ? options.currentFeatureSettings : null,
        options.value,
      ),
    }
  }

  if (
    options.isEditing
    && hasChatPiiRedactionFeatureSettings(options.currentFeatureSettings)
  ) {
    return {
      feature_settings: removeChatPiiRedactionFeatureSettings(
        options.currentFeatureSettings,
      ),
    }
  }

  return {}
}
