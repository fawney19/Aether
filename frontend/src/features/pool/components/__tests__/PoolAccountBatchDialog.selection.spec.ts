import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import PoolAccountBatchDialog from '@/features/pool/components/PoolAccountBatchDialog.vue'

const poolApiMocks = vi.hoisted(() => ({
  listPoolKeys: vi.fn(),
  createPoolKeySelectionSnapshot: vi.fn(),
  batchActionPoolKeys: vi.fn(),
  getPoolBatchDeleteTask: vi.fn(),
  resolvePoolKeySelection: vi.fn(),
}))

vi.mock('@/api/endpoints/pool', () => ({
  listPoolKeys: poolApiMocks.listPoolKeys,
  createPoolKeySelectionSnapshot: poolApiMocks.createPoolKeySelectionSnapshot,
  batchActionPoolKeys: poolApiMocks.batchActionPoolKeys,
  getPoolBatchDeleteTask: poolApiMocks.getPoolBatchDeleteTask,
  resolvePoolKeySelection: poolApiMocks.resolvePoolKeySelection,
}))

vi.mock('@/api/endpoints/keys', () => ({
  exportKey: vi.fn(),
  refreshProviderQuota: vi.fn(),
}))

vi.mock('@/api/endpoints/provider_oauth', () => ({
  refreshProviderOAuth: vi.fn(),
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const Dialog = defineComponent({
    name: 'DialogStub',
    props: {
      modelValue: Boolean,
    },
    setup(props, { slots }) {
      return () => props.modelValue
        ? h('section', [slots.default?.(), slots.footer?.()])
        : null
    },
  })

  const Button = defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    props: {
      disabled: Boolean,
      variant: String,
      size: String,
    },
    setup(props, { attrs, slots }) {
      return () => h('button', {
        ...attrs,
        disabled: props.disabled,
        type: 'button',
      }, slots.default?.())
    },
  })

  const Input = defineComponent({
    name: 'InputStub',
    inheritAttrs: false,
    props: {
      modelValue: {
        type: [String, Number],
        default: '',
      },
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        value: props.modelValue ?? '',
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
      })
    },
  })

  const Checkbox = defineComponent({
    name: 'CheckboxStub',
    inheritAttrs: false,
    props: {
      checked: Boolean,
      disabled: Boolean,
      indeterminate: Boolean,
    },
    emits: ['update:checked'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        type: 'checkbox',
        checked: props.checked,
        disabled: props.disabled,
        onChange: (event: Event) => emit('update:checked', (event.target as HTMLInputElement).checked),
      })
    },
  })

  const Badge = defineComponent({
    name: 'BadgeStub',
    setup(_, { slots }) {
      return () => h('span', slots.default?.())
    },
  })

  return {
    Dialog,
    Button,
    Input,
    Checkbox,
    Badge,
  }
})

vi.mock('@/features/providers/components/ProxyNodeSelect.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'ProxyNodeSelectStub',
      props: {
        modelValue: {
          type: String,
          default: '',
        },
        disabled: Boolean,
      },
      emits: ['update:modelValue'],
      setup(props, { emit }) {
        return () => h('select', {
          value: props.modelValue,
          disabled: props.disabled,
          onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLSelectElement).value),
        })
      },
    }),
  }
})

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirm: vi.fn().mockResolvedValue(true),
  }),
}))

vi.mock('@/stores/proxy-nodes', () => ({
  useProxyNodesStore: () => ({
    ensureLoaded: vi.fn(),
  }),
}))

vi.mock('@/utils/oauthIdentity', () => ({
  getOAuthOrgBadge: () => null,
}))

vi.mock('@/utils/providerKeyAuth', () => ({
  canExportOAuthCredential: () => false,
  canRefreshOAuthCredential: () => false,
  getProviderAuthLabel: (key: { auth_type?: string }) => key.auth_type || 'api_key',
}))

vi.mock('@/utils/providerKeyStatus', () => ({
  getAccountStatusDisplay: () => ({ blocked: false, label: '' }),
  getAccountStatusTitle: () => '',
  getOAuthStatusDisplay: () => null,
  getOAuthStatusTitle: () => '',
}))

vi.mock('@/utils/providerKeyQuota', () => ({
  getQuotaDisplayText: () => null,
}))

vi.mock('@/utils/batchAction', () => ({
  runChunkedBatchAction: vi.fn(),
}))

type MountedDialog = {
  app: App
  root: HTMLDivElement
  vm: { open: boolean }
}

function key(id: string, name: string) {
  return {
    key_id: id,
    key_name: name,
    auth_type: 'api_key',
    is_active: true,
  }
}

function pageResponse(total: number) {
  return {
    total,
    page: 1,
    page_size: 50,
    keys: [key('key-visible-1', 'visible account')],
  }
}

function snapshotResponse(snapshotId: string, total: number) {
  return {
    total,
    page: 1,
    page_size: 50,
    selection_snapshot: {
      id: snapshotId,
      total,
      status: 'ready',
    },
  }
}

function driftedSnapshotResponse(snapshotId: string, expectedTotal: number, actualTotal: number) {
  return {
    ...snapshotResponse(snapshotId, actualTotal),
    selection_snapshot_mismatch: {
      reason: 'total_changed',
      expected_total: expectedTotal,
      actual_total: actualTotal,
    },
  }
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

async function flushAsync(): Promise<void> {
  await Promise.resolve()
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

async function mountDialog(): Promise<MountedDialog> {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const Parent = defineComponent({
    setup() {
      const open = ref(false)
      return { open }
    },
    render() {
      return h(PoolAccountBatchDialog, {
        modelValue: this.open,
        providerId: 'provider-1',
        providerName: 'Provider One',
        providerType: 'openai',
        'onUpdate:modelValue': (value: boolean) => {
          this.open = value
        },
      })
    },
  })

  const app = createApp(Parent)
  const vm = app.mount(root) as unknown as { open: boolean }
  vm.open = true
  await nextTick()
  await flushAsync()
  return { app, root, vm }
}

function firstCheckbox(root: ParentNode): HTMLInputElement {
  const checkbox = root.querySelector('input[type="checkbox"]')
  if (!(checkbox instanceof HTMLInputElement)) {
    throw new Error('checkbox should exist')
  }
  return checkbox
}

function buttonByText(root: ParentNode, text: string): HTMLButtonElement {
  const button = Array.from(root.querySelectorAll('button'))
    .find((item) => item.textContent?.trim() === text)
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`${text} button should exist`)
  }
  return button
}

afterEach(() => {
  vi.clearAllMocks()
  document.body.innerHTML = ''
})

describe('PoolAccountBatchDialog selection snapshots', () => {
  it('creates a snapshot only when selecting all filtered results', async () => {
    poolApiMocks.listPoolKeys.mockResolvedValueOnce(pageResponse(10))
    poolApiMocks.createPoolKeySelectionSnapshot.mockResolvedValueOnce(snapshotResponse('snapshot-visible', 10))

    const mounted = await mountDialog()
    expect(poolApiMocks.listPoolKeys).toHaveBeenCalledTimes(1)
    expect(poolApiMocks.listPoolKeys).toHaveBeenLastCalledWith(
      'provider-1',
      expect.not.objectContaining({ include_selection_snapshot: true }),
    )
    expect(poolApiMocks.createPoolKeySelectionSnapshot).not.toHaveBeenCalled()

    const checkbox = firstCheckbox(mounted.root)
    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()

    expect(poolApiMocks.listPoolKeys).toHaveBeenCalledTimes(1)
    expect(poolApiMocks.createPoolKeySelectionSnapshot).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        expected_total: 10,
        expected_page_key_ids: ['key-visible-1'],
      }),
    )
    expect(mounted.root.textContent).toContain('已选 10 个')
    mounted.app.unmount()
  })

  it('refreshes stale displayed filters before creating a snapshot', async () => {
    poolApiMocks.listPoolKeys
      .mockResolvedValueOnce(pageResponse(10))
      .mockResolvedValueOnce(pageResponse(12))
    poolApiMocks.createPoolKeySelectionSnapshot.mockResolvedValueOnce(snapshotResponse('snapshot-new-filter', 12))

    const mounted = await mountDialog()
    const search = mounted.root.querySelector('input:not([type="checkbox"])')
    if (!(search instanceof HTMLInputElement)) {
      throw new Error('search input should exist')
    }

    search.value = 'new filter'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const checkbox = firstCheckbox(mounted.root)
    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()

    expect(poolApiMocks.listPoolKeys).toHaveBeenCalledTimes(2)
    expect(poolApiMocks.listPoolKeys).toHaveBeenLastCalledWith(
      'provider-1',
      expect.objectContaining({
        search: 'new filter',
      }),
    )
    expect(poolApiMocks.createPoolKeySelectionSnapshot).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        search: 'new filter',
        expected_total: 12,
        expected_page_key_ids: ['key-visible-1'],
      }),
    )
    expect(mounted.root.textContent).toContain('已选 12 个')
    expect(mounted.root.textContent).toContain('共 12 个匹配账号')
    mounted.app.unmount()
  })

  it('keeps the snapshot selection when filtered results drift during creation', async () => {
    poolApiMocks.listPoolKeys.mockResolvedValueOnce(pageResponse(10))
    poolApiMocks.createPoolKeySelectionSnapshot.mockResolvedValueOnce(
      driftedSnapshotResponse('snapshot-drifted', 10, 12),
    )

    const mounted = await mountDialog()
    const checkbox = firstCheckbox(mounted.root)
    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()

    expect(poolApiMocks.listPoolKeys).toHaveBeenCalledTimes(1)
    expect(poolApiMocks.createPoolKeySelectionSnapshot).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        expected_total: 10,
        expected_page_key_ids: ['key-visible-1'],
      }),
    )
    expect(mounted.root.textContent).toContain('已选 12 个')
    expect(mounted.root.textContent).toContain('共 12 个匹配账号')
    mounted.app.unmount()
  })

  it('locks selection controls while creating the filtered snapshot', async () => {
    const pendingSnapshot = deferred<ReturnType<typeof snapshotResponse>>()
    poolApiMocks.listPoolKeys.mockResolvedValueOnce(pageResponse(100000))
    poolApiMocks.createPoolKeySelectionSnapshot.mockReturnValueOnce(pendingSnapshot.promise)

    const mounted = await mountDialog()
    const checkbox = firstCheckbox(mounted.root)
    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()

    expect(mounted.root.textContent).toContain('正在生成筛选结果快照')
    expect(checkbox.disabled).toBe(true)
    expect(buttonByText(mounted.root, '账号异常').disabled).toBe(true)
    expect(buttonByText(mounted.root, '本页全选').disabled).toBe(true)
    expect(buttonByText(mounted.root, '刷新额度').disabled).toBe(true)
    const search = mounted.root.querySelector('input:not([type="checkbox"])')
    expect(search).toBeInstanceOf(HTMLInputElement)
    expect((search as HTMLInputElement).disabled).toBe(true)

    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()
    expect(poolApiMocks.createPoolKeySelectionSnapshot).toHaveBeenCalledTimes(1)

    pendingSnapshot.resolve(snapshotResponse('snapshot-large', 100000))
    await flushAsync()

    expect(mounted.root.textContent).toContain('已选 100000 个')
    expect(mounted.root.textContent).not.toContain('正在生成筛选结果快照')
    expect(firstCheckbox(mounted.root).disabled).toBe(false)
    mounted.app.unmount()
  })

  it('clears snapshot selection before refreshing after a batch action', async () => {
    poolApiMocks.listPoolKeys
      .mockResolvedValueOnce(pageResponse(4220))
      .mockResolvedValueOnce(pageResponse(48))
    poolApiMocks.createPoolKeySelectionSnapshot.mockResolvedValueOnce(snapshotResponse('snapshot-before-action', 4220))
    poolApiMocks.batchActionPoolKeys.mockResolvedValueOnce({
      affected: 4220,
      message: 'ok',
    })

    const mounted = await mountDialog()
    const checkbox = firstCheckbox(mounted.root)
    checkbox.checked = true
    checkbox.dispatchEvent(new Event('change', { bubbles: true }))
    await flushAsync()
    expect(mounted.root.textContent).toContain('已选 4220 个')

    buttonByText(mounted.root, '禁用').click()
    await flushAsync()
    await flushAsync()

    expect(poolApiMocks.batchActionPoolKeys).toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({
        selection: {
          type: 'snapshot',
          snapshot_id: 'snapshot-before-action',
          expected_total: 4220,
        },
        action: 'disable',
      }),
    )
    expect(poolApiMocks.listPoolKeys).toHaveBeenCalledTimes(2)
    expect(mounted.root.textContent).toContain('共 48 个匹配账号')
    expect(mounted.root.textContent).toContain('已选 0 个')
    expect(mounted.root.textContent).not.toContain('已选 4220 个')
    mounted.app.unmount()
  })
})
