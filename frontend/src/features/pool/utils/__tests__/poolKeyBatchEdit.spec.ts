import { describe, expect, it } from 'vitest'
import {
  buildPoolKeyBatchUpdatePatch,
  parsePoolKeyModelPatterns,
  type PoolKeyBatchEditState,
} from '../poolKeyBatchEdit'

function state(overrides: Partial<PoolKeyBatchEditState> = {}): PoolKeyBatchEditState {
  return {
    applyApiFormats: false,
    apiFormats: [],
    applyActive: false,
    isActive: true,
    applyInternalPriority: false,
    internalPriority: '0',
    applyRpmLimit: false,
    rpmLimit: '',
    applyConcurrentLimit: false,
    concurrentLimit: '',
    applyCacheTtl: false,
    cacheTtlMinutes: '5',
    applyProbeInterval: false,
    maxProbeIntervalMinutes: '32',
    applyNote: false,
    note: '',
    applyAutoFetchModels: false,
    autoFetchModels: false,
    includePatterns: '',
    excludePatterns: '',
    applyAllowedModels: false,
    unrestrictedModels: true,
    selectedModels: [],
    lockSelectedModels: true,
    ...overrides,
  }
}

describe('buildPoolKeyBatchUpdatePatch', () => {
  it('only emits fields explicitly enabled by the operator', () => {
    const result = buildPoolKeyBatchUpdatePatch(state({
      applyApiFormats: true,
      apiFormats: ['openai:responses', 'openai:responses', ' openai:chat '],
      applyRpmLimit: true,
      rpmLimit: '',
    }))

    expect(result.error).toBeNull()
    expect(result.patch).toEqual({
      api_formats: ['openai:responses', 'openai:chat'],
      rpm_limit: null,
    })
  })

  it('builds an explicit model access range without changing automatic discovery', () => {
    const result = buildPoolKeyBatchUpdatePatch(state({
      applyAllowedModels: true,
      unrestrictedModels: false,
      selectedModels: ['gpt-5.6-sol', 'gpt-5.6-sol', 'gpt-5.6-luna'],
      lockSelectedModels: false,
    }))

    expect(result.patch).toEqual({
      allowed_models: ['gpt-5.6-sol', 'gpt-5.6-luna'],
      locked_models: [],
    })
  })

  it('builds automatic discovery filters independently from the model access range', () => {
    const result = buildPoolKeyBatchUpdatePatch(state({
      applyAutoFetchModels: true,
      autoFetchModels: true,
      includePatterns: 'gpt-*,\nclaude-*',
      excludePatterns: '*-preview, *-beta',
    }))

    expect(result.patch).toEqual({
      auto_fetch_models: true,
      model_include_patterns: ['gpt-*', 'claude-*'],
      model_exclude_patterns: ['*-preview', '*-beta'],
    })
  })

  it('disables automatic discovery without rewriting model filters or access limits', () => {
    const result = buildPoolKeyBatchUpdatePatch(state({
      applyAutoFetchModels: true,
      autoFetchModels: false,
      includePatterns: 'gpt-*',
      excludePatterns: '*-preview',
    }))

    expect(result.patch).toEqual({
      auto_fetch_models: false,
    })
  })

  it('locks selected models only when the operator enables locking', () => {
    const result = buildPoolKeyBatchUpdatePatch(state({
      applyAllowedModels: true,
      unrestrictedModels: false,
      selectedModels: ['gpt-5.6-sol'],
      lockSelectedModels: true,
    }))

    expect(result.patch).toEqual({
      allowed_models: ['gpt-5.6-sol'],
      locked_models: ['gpt-5.6-sol'],
    })
  })

  it('rejects empty fields and invalid ranges before the request is sent', () => {
    expect(buildPoolKeyBatchUpdatePatch(state()).error).toBe('请至少启用一个批量编辑字段')
    expect(buildPoolKeyBatchUpdatePatch(state({
      applyApiFormats: true,
    })).error).toBe('请至少选择一个支持的 API')
    expect(buildPoolKeyBatchUpdatePatch(state({
      applyCacheTtl: true,
      cacheTtlMinutes: '61',
    })).error).toBe('缓存 TTL 必须是 0-60 的整数')
    expect(buildPoolKeyBatchUpdatePatch(state({
      applyAllowedModels: true,
      unrestrictedModels: false,
    })).error).toBe('请至少选择一个可用模型')
  })
})

describe('parsePoolKeyModelPatterns', () => {
  it('normalizes comma and line separated patterns', () => {
    expect(parsePoolKeyModelPatterns(' gpt-* , claude-*\ngpt-* ')).toEqual([
      'gpt-*',
      'claude-*',
    ])
  })
})
