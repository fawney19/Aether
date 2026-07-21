import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const source = readFileSync(
  resolve(process.cwd(), 'src/views/shared/Usage.vue'),
  'utf8',
)

describe('admin usage initial loading', () => {
  it('starts the user filter request before analytics completes', () => {
    const mountedBlock = source
      .split('onMounted(async () => {')[1]
      ?.split('// 处理时间范围变化')[0]

    expect(mountedBlock).toBeTruthy()
    expect(mountedBlock).toContain('const adminUsersPromise = loadAdminUsers()')
    expect(mountedBlock).toContain('Promise.all([heatmapPromise, adminUsersPromise])')
    expect(mountedBlock?.indexOf('const adminUsersPromise = loadAdminUsers()'))
      .toBeLessThan(mountedBlock?.indexOf('await loadRecords(') ?? -1)
    expect(mountedBlock).not.toContain('await loadAdminUsers()')
  })

  it('uses authoritative active snapshots for errors and final-provider facts', () => {
    const pollBlock = source
      .split('async function pollActiveRequests()')[1]
      ?.split('async function discoverActiveRequests()')[0]

    expect(pollBlock).toBeTruthy()
    expect(pollBlock).toContain('const shouldApply = !updateSnapshotIsOlder && newRank >= currentRank')
    expect(pollBlock).toContain('!updateSnapshotIsOlder && currentRank < 2 && updateHasFailureSignal')
    expect(pollBlock).toContain('record.error_message = mergeUsageRecordErrorMessage(')
    expect(pollBlock).toContain('{ authoritative: shouldApply }')
    expect(pollBlock).toContain('record.target_model = typeof update.target_model')
    expect(pollBlock).toContain('record.reasoning_effort = typeof update.reasoning_effort')
    expect(pollBlock).toContain('record.service_tier = typeof update.service_tier')
    expect(pollBlock).not.toContain("if ('target_model' in update)")
    expect(pollBlock).not.toContain("if ('reasoning_effort' in update)")
    expect(pollBlock).toContain(
      "if (typeof update.requested_reasoning_effort === 'string' && update.requested_reasoning_effort.trim())",
    )
  })
})
