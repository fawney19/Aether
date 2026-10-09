import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App, type Component } from 'vue'
import { setI18nLocale } from '@/i18n'
import type { CheckUpdateResponse, ReleaseEntry } from '@/api/admin'
import VersionButton from '../VersionButton.vue'
import UpdateDialog from '../UpdateDialog.vue'

const getSystemReleases = vi.hoisted(() => vi.fn())
vi.mock('@/api/admin', () => ({ adminApi: { getSystemReleases } }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const { default: Button } = await import('@/components/ui/button.vue')
  const passthrough = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  return {
    Button,
    Popover: passthrough,
    PopoverTrigger: passthrough,
    PopoverContent: passthrough,
    Dialog: defineComponent({
      props: { modelValue: Boolean },
      setup: (props, { slots }) => () => props.modelValue
        ? h('section', { role: 'dialog' }, [slots.default?.(), slots.footer?.()])
        : null,
    }),
  }
})

const status: CheckUpdateResponse = {
  current_version: '1.0.0', latest_version: '1.1.0', has_update: true,
  updatable: true, update_blocker: null, error: null,
  release_url: 'https://github.com/fawney19/Aether/releases/tag/v1.1.0',
  release_notes: '- 修复退款状态', published_at: null,
}
const release: ReleaseEntry = {
  version: '1.1.0', release_url: status.release_url, release_notes: status.release_notes,
  published_at: null, is_current: false, is_newer: true, updatable: true, update_blocker: null,
}
const mounted: Array<{ app: App; root: HTMLElement }> = []
function mount(component: Component, props: Record<string, unknown>) {
  const root = document.createElement('div')
  document.body.append(root)
  const app = createApp(defineComponent({ setup: () => () => h(component, props) }))
  app.config.globalProperties.$legacyT = (text: string) => text
  app.mount(root)
  mounted.push({ app, root })
  return root
}
function button(root: HTMLElement, label: string) {
  const match = [...root.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === label)
  if (!match) throw new Error(`Button not found: ${label}; available: ${[...root.querySelectorAll('button')].map(item => item.textContent?.trim()).join(', ')}`)
  return match
}
function expectNoRepositoryEntry(root: HTMLElement) {
  expect(root.querySelector('a[href*="github.com"]')).toBeNull()
  expect(root.textContent).not.toMatch(/查看发布|查看标签页|查看更新|GitHub|git pull|源码构建/)
}
async function showRelease(root: HTMLElement) {
  button(root, '历史版本').click()
  await vi.waitFor(() => expect([...root.querySelectorAll('button')].some(item => item.textContent?.includes('详情'))).toBe(true))
  const row = [...root.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.includes('详情'))!
  row.click()
  await nextTick()
}

beforeEach(() => {
  setI18nLocale('zh-CN')
  getSystemReleases.mockReset()
  getSystemReleases.mockResolvedValue({ releases: [release], error: null })
})
afterEach(() => {
  for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() }
})

describe('version controls without repository entries', () => {
  it('keeps online update and historical version selection available', async () => {
    const apply = vi.fn()
    const preview = vi.fn()
    const root = mount(VersionButton, { status, updateSupported: true, onApplyUpdate: apply, onPreviewRelease: preview })
    expectNoRepositoryEntry(root)
    button(root, '立即更新').click()
    expect(apply).toHaveBeenCalledOnce()
    await showRelease(root)
    expect(root.querySelector('[role="dialog"]')?.textContent).toContain('修复退款状态')
    expectNoRepositoryEntry(root)
    button(root, '更新到此版本').click()
    expect(preview).toHaveBeenCalledWith(release)
  })

  it('replaces old server source-build hints in both the version list and details', async () => {
    const oldHint = '当前为源码构建，请使用 git pull 后重新编译。'
    getSystemReleases.mockResolvedValue({ releases: [{ ...release, updatable: false, update_blocker: oldHint }], error: null })
    const root = mount(VersionButton, { status: { ...status, updatable: false, update_blocker: oldHint }, updateSupported: false })
    await showRelease(root)
    expectNoRepositoryEntry(root)
    expect(root.querySelector('[role="dialog"]')?.textContent).toContain('手动安装目标版本')
    expect(root.textContent).not.toContain('更新到此版本')
  })

  it('keeps update and rollback actions in the confirmation dialog', async () => {
    const apply = vi.fn()
    const rollback = vi.fn()
    const root = mount(UpdateDialog, {
      modelValue: true, currentVersion: '1.0.0', latestVersion: '1.1.0',
      releaseNotes: status.release_notes, publishedAt: null, rollbackAvailable: true, updateSupported: true, updatable: true,
      onApplyUpdate: apply, onRollback: rollback,
    })
    expectNoRepositoryEntry(root)
    button(root, '立即更新').click()
    button(root, '回滚上一版本').click()
    await nextTick()
    expect(apply).toHaveBeenCalledOnce()
    expect(rollback).toHaveBeenCalledOnce()
  })

  it('shows manual installation guidance without offering a release-page shortcut', () => {
    const root = mount(UpdateDialog, {
      modelValue: true, currentVersion: '1.0.0', latestVersion: '1.1.0', releaseNotes: null,
      publishedAt: null, updateSupported: false, updateBlocker: '当前为源码构建，请使用 git pull 后重新编译。',
    })
    expectNoRepositoryEntry(root)
    expect(root.textContent).toContain('手动安装目标版本')
    expect(root.textContent).not.toContain('立即更新')
  })
})
