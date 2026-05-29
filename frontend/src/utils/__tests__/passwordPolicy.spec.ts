import { describe, expect, it } from 'vitest'
import { generatePasswordByPolicy, getPasswordPolicyErrors, validatePasswordByPolicy } from '../passwordPolicy'

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

  it('generates passwords that satisfy every policy level', () => {
    for (const level of ['weak', 'medium', 'strong']) {
      const password = generatePasswordByPolicy(level)
      expect(validatePasswordByPolicy(password, level)).toBe('')
    }
  })

  it('generates strong passwords with all required character classes', () => {
    const password = generatePasswordByPolicy('strong')

    expect(password).toMatch(/[A-Z]/)
    expect(password).toMatch(/[a-z]/)
    expect(password).toMatch(/[0-9]/)
    expect(password).toMatch(/[!@#$%^&*()_+\-=[\]{};:'",.<>?/\\|`~]/)
  })
})
