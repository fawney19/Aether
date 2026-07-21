import { describe, expect, it } from 'vitest'

import {
  buildApiKeyRedactionFeatureSettingsPatch,
  resolveApiKeyRedactionFormState,
} from '../apiKeyFeatureSettings'

const redaction = {
  enabled: true,
  inject_model_instruction: false,
}

describe('managed API key feature setting inheritance', () => {
  it('uses the target user value for an inherited key form', () => {
    expect(resolveApiKeyRedactionFormState(null, {
      chat_pii_redaction: redaction,
    })).toEqual({
      mode: 'inherit',
      ...redaction,
    })
  })

  it('omits feature_settings when a created key keeps inheritance', () => {
    expect(buildApiKeyRedactionFeatureSettingsPatch({
      isEditing: false,
      currentFeatureSettings: undefined,
      mode: 'inherit',
      value: redaction,
    })).toEqual({})
  })

  it('writes an override only when custom mode is selected', () => {
    expect(buildApiKeyRedactionFeatureSettingsPatch({
      isEditing: false,
      currentFeatureSettings: undefined,
      mode: 'custom',
      value: redaction,
    })).toEqual({
      feature_settings: {
        chat_pii_redaction: redaction,
      },
    })
  })

  it('does not create an override when only another field of an inherited key changes', () => {
    expect(buildApiKeyRedactionFeatureSettingsPatch({
      isEditing: true,
      currentFeatureSettings: null,
      mode: 'inherit',
      value: redaction,
    })).toEqual({})
  })

  it('removes only the existing redaction override when inheritance is restored', () => {
    expect(buildApiKeyRedactionFeatureSettingsPatch({
      isEditing: true,
      currentFeatureSettings: {
        chat_pii_redaction: { enabled: false },
        notification_push_service: { enabled: true },
      },
      mode: 'inherit',
      value: redaction,
    })).toEqual({
      feature_settings: {
        notification_push_service: { enabled: true },
      },
    })
  })
})
