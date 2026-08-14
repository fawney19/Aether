import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

function readSource(relativePath: string): string {
  return readFileSync(resolve(process.cwd(), relativePath), 'utf8')
}

describe('provider key scope canonical response wiring', () => {
  it('keeps the standalone list aligned with the canonical update response', () => {
    const source = readSource('src/views/admin/ApiKeys.vue')
    const submit = source
      .split('async function handleKeyFormSubmit(data: StandaloneKeyFormData)')[1]
      ?.split('</script>')[0]

    expect(submit).toBeTruthy()
    expect(submit).toContain('const { message: _, ...updated } = await adminApi.updateApiKey')
    expect(submit).toContain('...updated,')
    expect(submit).toContain('allowed_provider_key_ids: data.allowed_provider_key_ids ?? null')
  })

  it('reloads the canonical group after saving before the next edit', () => {
    const source = readSource('src/features/users/components/UserGroupsDialog.vue')
    const selectGroup = source
      .split('async function selectGroup(groupId: string)')[1]
      ?.split('function normalizeListMode')[0]
    const saveGroup = source
      .split('async function saveGroup()')[1]
      ?.split('async function deleteSelectedGroup')[0]

    expect(selectGroup).toContain('providerKeyScopeFromApi(group.allowed_provider_key_ids)')
    expect(saveGroup).toContain('const saved = editingGroupId.value')
    expect(saveGroup).toContain('await loadDialogData()')
  })
})
