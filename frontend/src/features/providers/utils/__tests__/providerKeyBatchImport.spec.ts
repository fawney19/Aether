import { describe, expect, it } from 'vitest'

import { parseProviderKeyBatchImport } from '../providerKeyBatchImport'

describe('provider key batch import parser', () => {
  it('parses required names and keys separated by four hyphens', () => {
    const result = parseProviderKeyBatchImport([
      'primary----sk-primary',
      'backup----sk-backup',
      'night----sk-night----suffix',
      '# ignored comment',
    ].join('\n'))

    expect(result.errors).toEqual([])
    expect(result.items).toEqual([
      { lineNumber: 1, name: 'primary', apiKey: 'sk-primary' },
      { lineNumber: 2, name: 'backup', apiKey: 'sk-backup' },
      { lineNumber: 3, name: 'night', apiKey: 'sk-night----suffix' },
    ])
  })

  it('reports invalid format, missing fields and duplicates', () => {
    const result = parseProviderKeyBatchImport([
      'one----sk-1',
      'two----sk-1',
      'one----sk-2',
      '----sk-3',
      'three----',
      'sk-without-name',
    ].join('\n'))

    expect(result.items).toHaveLength(1)
    expect(result.errors.map(error => error.message)).toEqual([
      'Key 与前面行重复',
      '名称与前面行重复',
      '名称不能为空',
      'Key 不能为空',
      '格式应为 名称----Key',
    ])
  })

  it('does not impose a client-side item limit', () => {
    const input = Array.from({ length: 750 }, (_, index) => `key-${index}----sk-${index}`).join('\n')
    const result = parseProviderKeyBatchImport(input)

    expect(result.items).toHaveLength(750)
    expect(result.errors).toEqual([])
  })
})