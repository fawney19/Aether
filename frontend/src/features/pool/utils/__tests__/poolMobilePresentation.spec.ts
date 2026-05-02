import { describe, expect, it } from 'vitest'

import {
  buildPoolMobileTagItems,
  splitPoolMobileActions,
} from '@/features/pool/utils/poolMobilePresentation'

describe('poolMobilePresentation', () => {
  it('prioritizes critical mobile tags before secondary identity tags', () => {
    expect(
      buildPoolMobileTagItems({
        priorityLabel: 'P40',
        authLabel: 'OAuth',
        oauthStatusLabel: 'Token 过期',
        oauthStatusTone: 'danger',
        accountStatusLabel: '账号停用',
        accountStatusTone: 'danger',
        planLabel: 'Team',
        orgLabel: 'Org A',
        proxyLabel: '独立代理',
      }).map(item => item.label),
    ).toEqual([
      '账号停用',
      'Token 过期',
      'P40',
      'OAuth',
      'Team',
      'Org A',
      '独立代理',
    ])
  })

  it('keeps all available mobile actions inline without overflow grouping', () => {
    expect(
      splitPoolMobileActions({
        canRefreshToken: true,
        canClearCooldown: true,
        canRecoverHealth: true,
        canDownloadOrCopy: true,
        hasProxy: true,
      }),
    ).toEqual({
      primary: [
        'copy_or_download',
        'refresh_token',
        'clear_cooldown',
        'recover_health',
        'permissions',
        'proxy',
        'edit',
        'toggle',
        'delete',
      ],
      overflow: [],
    })
  })

  it('can show a disabled refresh action even when refresh is not available', () => {
    expect(
      splitPoolMobileActions({
        canDownloadOrCopy: true,
        canRefreshToken: false,
        showRefreshToken: true,
      }).primary,
    ).toContain('refresh_token')
  })
})
