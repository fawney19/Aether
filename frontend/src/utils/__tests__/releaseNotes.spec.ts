import { describe, expect, it } from 'vitest'
import { normalizeReleaseNotesForDisplay, trimReleaseNotesForDisplay } from '../releaseNotes'

describe('trimReleaseNotesForDisplay', () => {
  it('keeps manual notes and removes GitHub auto-generated sections', () => {
    const input = [
      '### Features',
      '- 新增在线更新',
      '',
      '### Bug Fixes',
      '- 修复版本检查',
      '',
      '## What\'s Changed',
      '* fix: something by @someone in https://github.com/example/repo/pull/1',
      '',
      '## New Contributors',
      '* @someone made their first contribution',
      '',
      '**Full Changelog**: https://github.com/example/repo/compare/v1...v2',
    ].join('\n')

    expect(trimReleaseNotesForDisplay(input)).toBe([
      '### Features',
      '- 新增在线更新',
      '',
      '### Bug Fixes',
      '- 修复版本检查',
    ].join('\n'))
  })

  it('removes standalone full changelog lines', () => {
    const input = '**Full Changelog**: https://github.com/example/repo/compare/v1...v2'
    expect(trimReleaseNotesForDisplay(input)).toBe('')
  })

  it('returns original notes when there is no auto-generated footer', () => {
    const input = [
      '### Features',
      '- 支持历史版本切换',
    ].join('\n')

    expect(trimReleaseNotesForDisplay(input)).toBe(input)
  })
})

describe('normalizeReleaseNotesForDisplay', () => {
  it('keeps existing markdown structure intact', () => {
    const input = [
      '### Features',
      '- 新增在线更新',
      '',
      '### Fixes',
      '- 修复版本检查',
    ].join('\n')

    expect(normalizeReleaseNotesForDisplay(input)).toBe(input)
  })

  it('converts plain multi-section notes into markdown sections', () => {
    const input = [
      'Features',
      '新增在线更新弹窗',
      '支持选择历史版本',
      '',
      '问题修复',
      '修复下载进度显示不准',
      '修复版本详情跳转错误',
    ].join('\n')

    expect(normalizeReleaseNotesForDisplay(input)).toBe([
      '### Features',
      '- 新增在线更新弹窗',
      '- 支持选择历史版本',
      '',
      '### 问题修复',
      '- 修复下载进度显示不准',
      '- 修复版本详情跳转错误',
    ].join('\n'))
  })

  it('does not over-normalize plain paragraph text', () => {
    const input = '这次更新主要修复了在线更新流程中的代理问题，并优化了历史版本切换体验。'
    expect(normalizeReleaseNotesForDisplay(input)).toBe(input)
  })
})
