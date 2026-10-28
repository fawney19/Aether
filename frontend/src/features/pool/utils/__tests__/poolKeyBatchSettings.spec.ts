import { describe, expect, it } from 'vitest'

import {
  buildPoolKeySettingsPatch,
  createPoolKeyBatchSettingSelection,
  createPoolKeyBatchSettingsDraft,
  validatePoolKeyBatchSettings,
} from '../poolKeyBatchSettings'

describe('pool key batch settings', () => {
  it('defaults the queue draft to enabled without selecting it for updates', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()

    expect(draft.concurrency_queue_enabled).toBe(true)
    expect(selection.concurrency_queue_enabled).toBe(false)
    expect(buildPoolKeySettingsPatch(selection, draft)).toEqual({})
  })

  it.each([true, false])('serializes the selected queue setting %s without changing the limit', (enabled) => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()
    selection.concurrency_queue_enabled = true
    draft.concurrency_queue_enabled = enabled

    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual([])
    expect(JSON.parse(JSON.stringify(buildPoolKeySettingsPatch(selection, draft)))).toEqual({
      concurrency_queue_enabled: enabled,
    })
  })

  it('omits an unselected queue setting even when its draft is disabled', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()
    selection.concurrent_limit = true
    draft.concurrent_limit = 6
    draft.concurrency_queue_enabled = false

    expect(buildPoolKeySettingsPatch(selection, draft)).toEqual({ concurrent_limit: 6 })
  })

  it('only includes explicitly selected fields', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()
    selection.rpm_limit = true
    selection.proxy_node_id = true
    draft.rpm_limit = 800
    draft.proxy_mode = 'clear'

    expect(buildPoolKeySettingsPatch(selection, draft)).toEqual({
      rpm_limit: 800,
      proxy_node_id: null,
    })
  })

  it('supports adaptive RPM and unlimited concurrency', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()
    selection.rpm_limit = true
    selection.concurrent_limit = true

    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual([])
    expect(buildPoolKeySettingsPatch(selection, draft)).toEqual({
      rpm_limit: null,
      concurrent_limit: null,
    })
  })

  it('serializes a positive concurrency limit as a number', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()
    selection.concurrent_limit = true
    draft.concurrent_limit = 6

    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual([])
    expect(buildPoolKeySettingsPatch(selection, draft)).toEqual({
      concurrent_limit: 6,
    })
  })

  it('requires a selected field and a proxy node for set mode', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()

    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual(['请至少选择一个要修改的设置'])

    selection.proxy_node_id = true
    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual(['请选择要设置的代理节点'])
  })
})
