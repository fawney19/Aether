export type PasswordPolicyLevel = 'weak' | 'medium' | 'strong'
export const PASSWORD_MAX_BYTES = 72

const textEncoder = new TextEncoder()

function getPasswordByteLength(password: string): number {
  return textEncoder.encode(password).length
}

export const PASSWORD_POLICY_OPTIONS: Array<{
  value: PasswordPolicyLevel
  label: string
  description: string
}> = [
  {
    value: 'weak',
    label: '弱密码',
    description: '至少 6 个字符',
  },
  {
    value: 'medium',
    label: '中等密码',
    description: '至少 8 个字符，且包含字母和数字',
  },
  {
    value: 'strong',
    label: '强密码',
    description: '至少 8 个字符，且包含大小写字母、数字和特殊字符',
  },
]

export function normalizePasswordPolicyLevel(value: unknown): PasswordPolicyLevel {
  if (value === 'medium' || value === 'strong') {
    return value
  }
  return 'weak'
}

export function getPasswordPolicyHint(level: unknown): string {
  switch (normalizePasswordPolicyLevel(level)) {
    case 'medium':
      return '至少 8 个字符，且需包含字母和数字'
    case 'strong':
      return '至少 8 个字符，且需包含大写字母、小写字母、数字和特殊字符'
    case 'weak':
    default:
      return '至少 6 个字符'
  }
}

export function getPasswordPolicyPlaceholder(level: unknown): string {
  switch (normalizePasswordPolicyLevel(level)) {
    case 'medium':
      return '至少 8 位，含字母和数字'
    case 'strong':
      return '至少 8 位，含大小写字母、数字和特殊字符'
    case 'weak':
    default:
      return '至少 6 个字符'
  }
}

/**
 * 返回所有未满足的密码策略条件。
 * 空数组 = 密码合规。
 */
export function getPasswordPolicyErrors(password: string, level: unknown): string[] {
  if (!password) return []

  const normalized = normalizePasswordPolicyLevel(level)
  const errors: string[] = []

  const byteLength = getPasswordByteLength(password)
  if (byteLength > PASSWORD_MAX_BYTES) {
    errors.push(`长度不能超过${PASSWORD_MAX_BYTES}字节`)
  }

  // 根据策略确定最小长度，不做两段式报错
  const minLen = normalized === 'weak' ? 6 : 8
  if (password.length < minLen) {
    errors.push(`至少 ${minLen} 个字符`)
  }

  if (normalized === 'medium') {
    if (!/[A-Za-z]/.test(password)) errors.push('包含字母')
    if (!/[0-9]/.test(password)) errors.push('包含数字')
  }

  if (normalized === 'strong') {
    if (!/[A-Z]/.test(password)) errors.push('包含大写字母')
    if (!/[a-z]/.test(password)) errors.push('包含小写字母')
    if (!/[0-9]/.test(password)) errors.push('包含数字')
    if (!/[!@#$%^&*()_+\-=[\]{};:'",.<>?/\\|`~]/.test(password)) errors.push('包含特殊字符')
  }

  return errors
}

/**
 * 兼容旧接口：返回单条错误字符串，空字符串表示通过。
 * 多条未满足条件时用顿号连接。
 */
export function validatePasswordByPolicy(password: string, level: unknown): string {
  const errors = getPasswordPolicyErrors(password, level)
  if (errors.length === 0) return ''
  if (errors.length === 1 && errors[0].startsWith('长度不能超过')) {
    return `密码${  errors[0]}`
  }
  return `密码需要：${  errors.join('、')}`
}
