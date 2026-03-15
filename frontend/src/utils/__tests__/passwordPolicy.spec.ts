import { describe, expect, it } from 'vitest'
import { getPasswordPolicyErrors, validatePasswordByPolicy } from '../passwordPolicy'

describe('passwordPolicy utils', () => {
  it('rejects passwords longer than 72 bytes', () => {
    expect(getPasswordPolicyErrors('a'.repeat(80), 'weak')).toContain('长度不能超过72字节')
  })

  it('rejects multibyte passwords longer than 72 bytes', () => {
    expect(getPasswordPolicyErrors('中'.repeat(25), 'weak')).toContain('长度不能超过72字节')
  })

  it('formats validation errors into a single message', () => {
    expect(validatePasswordByPolicy('abc', 'strong')).toBe(
      '密码需要：至少 8 个字符、包含大写字母、包含数字、包含特殊字符',
    )
  })
})
