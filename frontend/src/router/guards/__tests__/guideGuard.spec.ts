import { describe, expect, it } from 'vitest'

import { resolveGuideRedirectPath } from '@/composables/useSiteInfo'

describe('resolveGuideRedirectPath', () => {
  it('ignores non-guide routes', () => {
    expect(resolveGuideRedirectPath('/', 'hidden')).toBeNull()
    expect(resolveGuideRedirectPath('/admin/settings', 'custom')).toBeNull()
  })

  it('sends hidden guide visits back home', () => {
    expect(resolveGuideRedirectPath('/guide', 'hidden')).toBe('/')
    expect(resolveGuideRedirectPath('/guide/concepts', 'hidden')).toBe('/')
  })

  it('collapses custom guide child routes onto /guide', () => {
    expect(resolveGuideRedirectPath('/guide', 'custom')).toBeNull()
    expect(resolveGuideRedirectPath('/guide/architecture', 'custom')).toBe('/guide')
  })

  it('keeps the builtin guide tree', () => {
    expect(resolveGuideRedirectPath('/guide', 'builtin')).toBeNull()
    expect(resolveGuideRedirectPath('/guide/faq', 'builtin')).toBeNull()
  })
})
