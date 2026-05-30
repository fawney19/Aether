import { describe, expect, it } from 'vitest'
import type { SystemUpdatePreflightResponse } from '@/api/admin'
import {
  appendDockerUpdateFlag,
  buildDockerGuidedCommands,
  isPreflightBlocking,
} from '../updateDialogLogic'

function buildPreflight(
  overrides: Partial<SystemUpdatePreflightResponse> = {}
): SystemUpdatePreflightResponse {
  return {
    overall_status: 'ok',
    can_apply_update: true,
    generated_at: '2026-05-28T00:00:00Z',
    current_version: 'v0.1.0',
    build_type: 'release',
    checks: [],
    ...overrides,
  }
}

describe('isPreflightBlocking', () => {
  it('returns false when preflight payload is null or undefined', () => {
    expect(isPreflightBlocking(null)).toBe(false)
    expect(isPreflightBlocking(undefined)).toBe(false)
  })

  it('returns true when server reports can_apply_update=false', () => {
    expect(
      isPreflightBlocking(
        buildPreflight({
          can_apply_update: false,
          overall_status: 'blocked',
          checks: [
            { key: 'disk_space', label: '磁盘空间', status: 'ok', message: 'ok' },
          ],
        })
      )
    ).toBe(true)
  })

  it('returns true when any check is blocked, even if server flag missing', () => {
    expect(
      isPreflightBlocking(
        buildPreflight({
          can_apply_update: true,
          checks: [
            { key: 'paths', label: '目录权限', status: 'ok', message: 'ok' },
            { key: 'database', label: '数据库迁移', status: 'blocked', message: 'pending' },
          ],
        })
      )
    ).toBe(true)
  })

  it('returns false when all checks are ok or warning', () => {
    expect(
      isPreflightBlocking(
        buildPreflight({
          checks: [
            { key: 'strategy', label: '部署策略', status: 'ok', message: 'ok' },
            { key: 'task_state', label: '更新任务', status: 'warning', message: 'last failed' },
          ],
        })
      )
    ).toBe(false)
  })
})

describe('appendDockerUpdateFlag', () => {
  it('trims the base command and appends the requested flag', () => {
    expect(appendDockerUpdateFlag(' ./update.sh ', '--prepare')).toBe('./update.sh --prepare')
  })

  it('does not append the same flag twice', () => {
    expect(appendDockerUpdateFlag('./update.sh --prepare', '--prepare')).toBe('./update.sh --prepare')
  })

  it('uses the default Docker update command when the base command is empty', () => {
    expect(appendDockerUpdateFlag('', '--apply-prepared')).toBe('bash ./update.sh --apply-prepared')
  })
})

describe('buildDockerGuidedCommands', () => {
  it('returns null when no command is available', () => {
    expect(buildDockerGuidedCommands({})).toBeNull()
  })

  it('builds prepare and apply commands from the base command', () => {
    expect(
      buildDockerGuidedCommands({
        updateCommand: 'bash ./update.sh --mode single-node',
      })
    ).toEqual({
      updateCommand: 'bash ./update.sh --mode single-node',
      prepareCommand: 'bash ./update.sh --mode single-node --prepare',
      applyCommand: 'bash ./update.sh --mode single-node --apply-prepared',
    })
  })

  it('prefers explicit prepare and apply commands from the server', () => {
    expect(
      buildDockerGuidedCommands({
        updateCommand: './update.sh',
        prepareCommand: './update.sh --prepare --project-name aether',
        applyCommand: './update.sh --apply-prepared --project-name aether',
      })
    ).toEqual({
      updateCommand: './update.sh',
      prepareCommand: './update.sh --prepare --project-name aether',
      applyCommand: './update.sh --apply-prepared --project-name aether',
    })
  })
})
