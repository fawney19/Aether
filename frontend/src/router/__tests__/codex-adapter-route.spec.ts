import { describe, expect, it } from 'vitest'
import router from '../index'

describe('codex adapter admin route', () => {
  it('resolves the module management config page', () => {
    const resolved = router.resolve('/admin/modules/codex-adapter')

    expect(resolved.name).toBe('CodexAdapterModule')
    expect(resolved.matched.at(-1)?.meta.module).toBe('codex_adapter')
  })
})
