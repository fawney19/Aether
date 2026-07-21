import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/admin/WalletsManagement.vue'),
  'utf8',
)

describe('WalletsManagement wallet metadata loading', () => {
  it('only loads the full wallet metadata map for the orders tab', () => {
    const mountedBlock = source
      .split('onMounted(async () => {')[1]
      ?.split('})')[0]
    expect(mountedBlock).toBeTruthy()
    expect(mountedBlock).not.toContain('loadWalletMetaMap()')

    const ordersBranch = source
      .split("case 'orders':")[1]
      ?.split("case 'callbacks':")[0]
    expect(ordersBranch).toBeTruthy()
    expect(ordersBranch).toContain('Promise.all([loadOrders(), loadWalletMetaMap()])')
  })
})
