import { describe, expect, it } from 'vitest'

import { getOAuthIcon, OAUTH_ICONS } from '../oauth-icons'

describe('getOAuthIcon', () => {
  it('keeps built-in icons independent of configured URLs', () => {
    expect(getOAuthIcon('github', 'https://attacker.example/icon.svg')).toBe(OAUTH_ICONS.github)
  })

  it('allows HTTPS and root-relative custom icons', () => {
    expect(getOAuthIcon('custom', 'https://cdn.example/icon.svg')).toContain(
      'src="https://cdn.example/icon.svg"',
    )
    expect(getOAuthIcon('custom', '/assets/oauth/custom.svg')).toContain(
      'src="/assets/oauth/custom.svg"',
    )
  })

  it.each([
    'javascript:alert(1)',
    'data:image/svg+xml,<svg onload=alert(1)>',
    '//attacker.example/icon.svg',
    'http://cdn.example/icon.svg',
    'https://cdn.example/icon.svg" onerror="alert(1)',
  ])('rejects executable or injectable custom icon URL %s', (iconUrl) => {
    const rendered = getOAuthIcon('custom', iconUrl)

    expect(rendered).toBe(OAUTH_ICONS.github)
    expect(rendered).not.toContain('onerror')
    expect(rendered).not.toContain('javascript:')
    expect(rendered).not.toContain('data:')
  })
})
