import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/admin/ProviderManagement.vue'),
  'utf8',
)

describe('ProviderManagement detail drawer loading', () => {
  it('keeps the heavy detail drawer out of the initial route chunk', () => {
    expect(source).not.toContain(
      "import ProviderDetailDrawer from '@/features/providers/components/ProviderDetailDrawer.vue'",
    )
    expect(source).toContain(
      "() => import('@/features/providers/components/ProviderDetailDrawer.vue')",
    )
  })

  it('does not resolve the async drawer until it is opened', () => {
    const drawerTemplate = source
      .split('<ProviderDetailDrawer')[1]
      ?.split('/>')[0]

    expect(drawerTemplate).toBeTruthy()
    expect(drawerTemplate).toContain('v-if="providerDrawerOpen"')
  })
})
