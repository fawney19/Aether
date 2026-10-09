import { describe, expect, it } from 'vitest'
import type { CheckUpdateResponse } from '@/api/admin'
import {
  buildUpdateErrorStatus,
  describeUpdateStatus,
  normalizeUpdateBlockerForDisplay,
} from '../updateStatus'

function updateStatus(overrides: Partial<CheckUpdateResponse> = {}): CheckUpdateResponse {
  return {
    current_version: '0.7.0-rc27',
    latest_version: null,
    has_update: false,
    updatable: false,
    update_blocker: null,
    release_url: null,
    release_notes: null,
    published_at: null,
    error: null,
    ...overrides,
  }
}

describe('updateStatus', () => {
  it.each([
    null,
    '当前为源码构建，请使用 git pull 后重新编译。',
    '当前为源码构建，请手动切换到对应标签后重新编译。',
    'This is a source build. Run git pull and rebuild.',
    '当前部署策略不支持在线自更新，请手动下载 Release 或使用安装脚本更新。',
  ])('replaces cached repository instructions with a manual installation hint: %s', reason => {
    expect(normalizeUpdateBlockerForDisplay(reason)).toBe('当前安装方式不支持在线更新，请手动安装目标版本。')
  })

  it('preserves update blockers that explain deployment limitations', () => {
    const reason = 'Docker 部署请使用 docker compose pull && docker compose up -d 更新。'
    expect(normalizeUpdateBlockerForDisplay(reason)).toBe(reason)
  })

  it('describes loading and latest states', () => {
    expect(describeUpdateStatus(null)).toBe('检查中')
    expect(describeUpdateStatus(updateStatus())).toBe('已是最新')
  })

  it('prioritizes update availability over latest-version text', () => {
    expect(describeUpdateStatus(updateStatus({
      latest_version: 'v0.7.0-rc28',
      has_update: true,
      release_url: 'https://github.com/fawney19/Aether/releases/tag/v0.7.0-rc28',
    }))).toBe('有新版本')
  })

  it('preserves the current version when building an error state', () => {
    const status = buildUpdateErrorStatus(
      updateStatus({ current_version: '0.7.0-rc28' }),
      new Error('network down')
    )

    expect(status.current_version).toBe('0.7.0-rc28')
    expect(status.has_update).toBe(false)
    expect(status.error).toBe('network down')
  })
})
