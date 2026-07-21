import { describe, expect, it } from 'vitest'

import {
  buildPoolKeySettingsPatch,
  createPoolKeyBatchSettingSelection,
  createPoolKeyBatchSettingsDraft,
  validatePoolKeyBatchSettings,
} from '../poolKeyBatchSettings'

describe('pool key batch settings', () => {
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

  it('requires a selected field and a proxy node for set mode', () => {
    const selection = createPoolKeyBatchSettingSelection()
    const draft = createPoolKeyBatchSettingsDraft()

    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual(['请至少选择一个要修改的设置'])

    selection.proxy_node_id = true
    expect(validatePoolKeyBatchSettings(selection, draft)).toEqual(['请选择要设置的代理节点'])
  })
})