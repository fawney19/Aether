import { describe, expect, it } from 'vitest'

import { isCyberPolicyError } from '../cyberError'

describe('isCyberPolicyError', () => {
  it('recognizes the provider cybersecurity refusal message', () => {
    expect(isCyberPolicyError({
      error: {
        type: 'invalid_request',
        message: 'This content was flagged for possible cybersecurity risk. To get authorized for security work, join the Trusted Access for Cyber program: https://chatgpt.com/cyber',
        code: 400,
      },
    })).toBe(true)
  })

  it('recognizes an explicit cyber_policy code', () => {
    expect(isCyberPolicyError({ error: { code: 'CYBER_POLICY' } })).toBe(true)
  })

  it('recognizes explicit Cyber Policy types and reasons', () => {
    expect(isCyberPolicyError({ error: { type: 'cyber_policy' } })).toBe(true)
    expect(isCyberPolicyError({ error: { type: 'CYBER' } })).toBe(true)
    expect(isCyberPolicyError({ error: { reason: 'cyber-policy' } })).toBe(true)
    expect(isCyberPolicyError({ error: { category: 'cyber_policy_violation' } })).toBe(true)
    expect(isCyberPolicyError({ error: { type: 'cybersecurity-risk' } })).toBe(true)
  })

  it('recognizes structured Cyber classifiers inside a serialized error', () => {
    expect(isCyberPolicyError('{"error":{"type":"cyber"}}')).toBe(true)
  })

  it('does not classify ordinary invalid requests as Cyber Policy failures', () => {
    expect(isCyberPolicyError({
      error: {
        type: 'invalid_request',
        message: 'The request payload is malformed',
        code: 400,
      },
    })).toBe(false)
  })

  it('does not classify a generic use of the word cyber', () => {
    expect(isCyberPolicyError('The cyber security report was generated successfully')).toBe(false)
  })
})
