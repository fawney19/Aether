import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

function readSource(path: string): string {
  return readFileSync(resolve(process.cwd(), path), 'utf8')
}

describe('mobile dialog layout', () => {
  it('uses a full-height mobile sheet with stacked full-width actions', () => {
    const source = readSource('src/components/ui/dialog/Dialog.vue')

    expect(source).toContain('max-h-[100dvh]')
    expect(source).toContain('flex-col-reverse')
    expect(source).toContain('[&>button]:w-full')
    expect(source).toContain('sm:flex-row-reverse')
    expect(source).toContain('sm:[&>button]:w-auto')
  })

  it('keeps group navigation compact before the editor on mobile', () => {
    const dialogSource = readSource('src/features/users/components/UserGroupsDialog.vue')
    const listSource = readSource('src/features/users/components/UserGroupListPanel.vue')

    expect(dialogSource).toContain('lg:grid-cols-[17rem_minmax(0,1fr)]')
    expect(listSource).toContain('snap-x snap-mandatory')
    expect(listSource).toContain('overflow-x-auto')
    expect(listSource).toContain('min-h-10')
    expect(listSource).toContain('lg:w-full')
  })

  it('provides mobile state labels and 40px icon targets', () => {
    const accessSource = readSource('src/features/users/components/UserGroupAccessControlFields.vue')
    const headerSource = readSource('src/features/users/components/UserGroupEditorHeader.vue')

    expect(accessSource).toContain('sm:sr-only')
    expect(accessSource).toContain('h-10 w-10')
    expect(headerSource.match(/h-10 w-10/g)).toHaveLength(2)
  })
})