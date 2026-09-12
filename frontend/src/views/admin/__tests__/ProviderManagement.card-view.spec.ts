import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'
import { createI18n, setI18nLocale } from '@/i18n'
import ProviderManagement from '../ProviderManagement.vue'

const apiMocks = vi.hoisted(() => ({
  getProvidersSummary: vi.fn(),
  getGlobalModels: vi.fn(),
  getProvider: vi.fn(),
  updateProvider: vi.fn(),
}))

vi.mock('@/api/endpoints', async (importOriginal) => ({
  ...await importOriginal<typeof import('@/api/endpoints')>(),
  ...apiMocks,
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({ confirmDanger: vi.fn().mockResolvedValue(false) }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }),
}))

vi.mock('@/features/providers/composables/useProviderBalance', () => ({
  useProviderBalance: () => ({
    loadArchitectureSchemas: vi.fn(),
    loadBalances: vi.fn(),
    getProviderBalance: () => ({ available: 125, currency: 'USD' }),
    getProviderBalanceBreakdown: () => null,
    getProviderBalanceError: () => null,
    isBalanceLoading: () => false,
    getProviderCheckin: () => null,
    getProviderCookieExpired: () => null,
    formatBalanceDisplay: () => '$125.00',
    formatResetCountdown: () => '',
    getProviderBalanceExtra: () => [],
    getQuotaUsedColorClass: () => '',
    startTick: vi.fn(),
    stopTick: vi.fn(),
  }),
}))

vi.mock('@/features/providers/components', () => ({
  ProviderFormDialog: { render: () => null },
  ProviderAuthDialog: { render: () => null },
}))

vi.mock('@/features/providers/components/ProviderBatchActionDialog.vue', () => ({
  default: { render: () => null },
}))

vi.mock('@/features/providers/components/ProviderDetailDrawer.vue', async () => {
  const { h } = await import('vue')
  return {
    __esModule: true,
    default: {
      props: ['open', 'providerId'],
      setup: (props: { open: boolean; providerId: string }) => () => props.open
        ? h('div', { 'data-provider-detail': props.providerId })
        : null,
    },
  }
})

function createProvider(overrides: Partial<ProviderWithEndpointsSummary> = {}): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Provider One',
    description: 'Primary provider',
    provider_type: 'custom',
    provider_priority: 10,
    keep_priority_on_conversion: false,
    enable_format_conversion: true,
    is_active: true,
    total_endpoints: 2,
    active_endpoints: 1,
    total_keys: 3,
    active_keys: 2,
    total_models: 4,
    active_models: 3,
    global_model_ids: ['model-1'],
    avg_health_score: 0.8,
    unhealthy_endpoints: 0,
    api_formats: ['openai:chat'],
    endpoint_health_details: [{
      api_format: 'openai:chat',
      health_score: 0.8,
      is_active: true,
      total_keys: 3,
      active_keys: 2,
    }],
    ops_configured: true,
    created_at: '2026-09-07T00:00:00Z',
    updated_at: '2026-09-07T00:00:00Z',
    ...overrides,
  }
}

let mountedApp: App | null = null
let mountedRoot: HTMLElement | null = null

async function settle() {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function mountView() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  mountedRoot = root
  mountedApp = createApp(ProviderManagement)
  mountedApp.use(createI18n())
  mountedApp.mount(root)
  await settle()
  return root
}

function unmountView() {
  mountedApp?.unmount()
  mountedRoot?.remove()
  mountedApp = null
  mountedRoot = null
}

function findButton(root: HTMLElement, title: string): HTMLButtonElement {
  const button = root.querySelector<HTMLButtonElement>(`button[title="${title}"]`)
  expect(button, `Missing button: ${title}`).not.toBeNull()
  return button!
}

beforeEach(() => {
  vi.clearAllMocks()
  apiMocks.getProvidersSummary.mockResolvedValue({
    items: [createProvider()],
    total: 40,
  })
  apiMocks.getGlobalModels.mockResolvedValue({ models: [{ id: 'model-1', name: 'Model One' }] })
  apiMocks.getProvider.mockResolvedValue(createProvider())
  apiMocks.updateProvider.mockResolvedValue(createProvider({ is_active: false }))
})

const originalElementFromPoint = Object.getOwnPropertyDescriptor(document, 'elementFromPoint')

afterEach(() => {
  unmountView()
  if (originalElementFromPoint) {
    Object.defineProperty(document, 'elementFromPoint', originalElementFromPoint)
  } else {
    Reflect.deleteProperty(document, 'elementFromPoint')
  }
})

describe('ProviderManagement card view', () => {
  it('places the view toggle immediately after refresh and switches layouts without reloading data', async () => {
    const root = await mountView()
    const toggle = findButton(root, '切换到卡片视图')
    const filters = [...root.querySelectorAll('[role="combobox"]')].slice(0, 3)

    expect(toggle.previousElementSibling).toBe(findButton(root, '刷新'))
    expect(toggle.getAttribute('aria-pressed')).toBe('false')
    expect(root.querySelector('table')).not.toBeNull()
    expect(root.querySelector('dl')).toBeNull()
    for (const filter of filters) {
      expect(filter.closest('.xl\\:hidden')).not.toBeNull()
    }

    const requests = apiMocks.getProvidersSummary.mock.calls.length
    toggle.click()
    await settle()

    expect(root.querySelector('table')).toBeNull()
    expect(root.querySelectorAll('dl')).toHaveLength(1)
    expect(root.textContent).toContain('$125.00')
    expect(root.textContent).toContain('Primary provider')
    expect(root.textContent).toContain('80%')
    expect(toggle.getAttribute('aria-pressed')).toBe('true')
    for (const filter of filters) {
      expect(filter.closest('.xl\\:hidden')).toBeNull()
    }

    findButton(root, '切换到列表视图').click()
    await settle()
    expect(root.querySelector('table')).not.toBeNull()
    expect(root.querySelector('dl')).toBeNull()
    expect(apiMocks.getProvidersSummary).toHaveBeenCalledTimes(requests)
  })

  it('fills available width with shared grid columns and keeps bounded card content scrollable', async () => {
    apiMocks.getProvidersSummary.mockResolvedValue({
      items: Array.from({ length: 5 }, (_, index) => createProvider({ id: `provider-${index + 1}` })),
      total: 5,
    })
    localStorage.setItem('aether-provider-card-view', 'true')
    const root = await mountView()
    const card = root.querySelector<HTMLElement>('[data-provider-sort-id="provider-1"]')!
    const lastCard = root.querySelector<HTMLElement>('[data-provider-sort-id="provider-5"]')!
    const [header, content, actions] = Array.from(card.children)

    expect(Array.from(card.classList)).toEqual(expect.arrayContaining(['max-h-96', 'w-full']))
    expect(card.classList.contains('max-w-sm')).toBe(false)
    expect(card.classList.contains('flex-1')).toBe(false)
    expect(lastCard.className).toBe(card.className)
    expect(lastCard.parentElement).toBe(card.parentElement)
    expect(card.parentElement?.classList.contains('grid-cols-[repeat(auto-fill,minmax(min(100%,22rem),1fr))]')).toBe(true)
    expect(card.parentElement?.classList.contains('grid')).toBe(true)
    expect(card.parentElement?.classList.contains('box-content')).toBe(false)
    expect(card.parentElement?.classList.contains('items-start')).toBe(false)
    expect(header.classList.contains('shrink-0')).toBe(true)
    expect(Array.from(content.classList)).toEqual(expect.arrayContaining(['min-h-0', 'overflow-y-auto']))
    expect(actions.classList.contains('shrink-0')).toBe(true)
    expect(actions.contains(findButton(root, '查看详情'))).toBe(true)
  })

  it('remembers the chosen layout across remounts', async () => {
    let root = await mountView()
    findButton(root, '切换到卡片视图').click()
    await settle()
    expect(localStorage.getItem('aether-provider-card-view')).toBe('true')

    unmountView()
    root = await mountView()
    expect(root.querySelector('table')).toBeNull()
    expect(findButton(root, '切换到列表视图').getAttribute('aria-pressed')).toBe('true')

    findButton(root, '切换到列表视图').click()
    await settle()
    expect(localStorage.getItem('aether-provider-card-view')).toBe('false')
  })

  it.each([
    { name: 'cards', initial: false, selected: true, title: '切换到卡片视图' },
    { name: 'table', initial: true, selected: false, title: '切换到列表视图' },
  ])('immediately saves $name and restores it while reloading data', async ({ initial, selected, title }) => {
    localStorage.setItem('aether-provider-card-view', String(initial))
    let root = await mountView()
    findButton(root, title).click()
    expect(localStorage.getItem('aether-provider-card-view')).toBe(String(selected))

    unmountView()
    let finishLoading!: (value: { items: ProviderWithEndpointsSummary[]; total: number }) => void
    apiMocks.getProvidersSummary.mockReturnValue(new Promise((resolve) => {
      finishLoading = resolve
    }))
    root = await mountView()
    const restoredTitle = selected ? '切换到列表视图' : '切换到卡片视图'
    expect(findButton(root, restoredTitle).getAttribute('aria-pressed')).toBe(String(selected))
    expect(findButton(root, '刷新').disabled).toBe(true)

    finishLoading({ items: [createProvider()], total: 1 })
    await settle()
    expect(root.querySelector('table') === null).toBe(selected)
    expect(root.querySelector('dl') !== null).toBe(selected)
    expect(localStorage.getItem('aether-provider-card-view')).toBe(String(selected))
  })

  it('keeps the current search and page when switching views', async () => {
    const root = await mountView()
    const search = root.querySelector<HTMLInputElement>('#provider-search')!
    search.value = 'Provider'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await vi.waitFor(() => {
      expect(apiMocks.getProvidersSummary).toHaveBeenLastCalledWith(
        expect.objectContaining({ search: 'Provider' }),
        expect.any(Object),
      )
    })

    const secondPage = [...root.querySelectorAll<HTMLButtonElement>('button')]
      .find(button => button.textContent?.trim() === '2')!
    secondPage.click()
    await settle()
    const requests = apiMocks.getProvidersSummary.mock.calls.length

    findButton(root, '切换到卡片视图').click()
    await settle()
    expect(search.value).toBe('Provider')
    expect(root.querySelector('[aria-current="page"]')?.textContent?.trim()).toBe('2')
    expect(apiMocks.getProvidersSummary).toHaveBeenCalledTimes(requests)
    expect(apiMocks.getProvidersSummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ search: 'Provider', page: 2 }),
      expect.any(Object),
    )
  })

  it('supports note editing, status actions, and details from cards', async () => {
    localStorage.setItem('aether-provider-card-view', 'true')
    const root = await mountView()
    findButton(root, 'Primary provider').click()
    await settle()

    const input = root.querySelector<HTMLInputElement>('[data-desc-editor] input')!
    expect(input.value).toBe('Primary provider')
    input.value = 'Updated note'
    input.dispatchEvent(new Event('input', { bubbles: true }))
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }))
    await settle()

    expect(apiMocks.updateProvider).toHaveBeenCalledWith('provider-1', { description: 'Updated note' })
    expect(root.textContent).toContain('Updated note')
    expect(root.querySelector('[data-provider-detail]')).toBeNull()

    findButton(root, '停用提供商').click()
    await settle()
    expect(apiMocks.updateProvider).toHaveBeenCalledWith('provider-1', { is_active: false })
    expect(root.querySelector('[data-provider-detail]')).toBeNull()

    findButton(root, 'Provider One').click()
    await vi.waitFor(() => {
      expect(root.querySelector('[data-provider-detail="provider-1"]')).not.toBeNull()
    })
  })

  it('uses account labels and handles providers without endpoints', async () => {
    localStorage.setItem('aether-provider-card-view', 'true')
    apiMocks.getProvidersSummary.mockResolvedValue({
      items: [createProvider({ provider_type: 'codex', is_active: false, endpoint_health_details: [] })],
      total: 1,
    })
    const root = await mountView()

    expect(root.querySelector('dl')?.textContent).toContain('账号')
    expect(root.textContent).toContain('暂无端点')
    expect(findButton(root, '启用提供商')).not.toBeNull()
  })

  it('does not display cards during loading or with an empty result', async () => {
    localStorage.setItem('aether-provider-card-view', 'true')
    let resolveRequest!: (value: { items: ProviderWithEndpointsSummary[]; total: number }) => void
    apiMocks.getProvidersSummary.mockReturnValue(new Promise((resolve) => {
      resolveRequest = resolve
    }))
    const root = await mountView()
    expect(root.querySelector('dl')).toBeNull()
    expect(findButton(root, '刷新').disabled).toBe(true)

    resolveRequest({ items: [], total: 0 })
    await settle()
    expect(root.querySelector('dl')).toBeNull()
    expect(root.textContent).toContain('暂无提供商，点击右上角添加')
    expect(findButton(root, '刷新').disabled).toBe(false)
  })

  it('translates the view switch labels', async () => {
    const root = await mountView()
    setI18nLocale('en-US')
    await settle()
    expect(findButton(root, 'Switch to card view').getAttribute('aria-label')).toBe('Card view')

    findButton(root, 'Switch to card view').click()
    await settle()
    expect(findButton(root, 'Switch to list view').getAttribute('aria-pressed')).toBe('true')
  })
})

function mockSortableProviders() {
  const providers = [1, 2, 3, 4].map(index => createProvider({
    id: `provider-${index}`,
    name: `Provider ${index}`,
    provider_priority: index * 10,
  }))
  apiMocks.getProvidersSummary.mockResolvedValue({ items: providers, total: providers.length })
  return providers
}

function providerElements(root: HTMLElement): HTMLElement[] {
  const container = root.querySelector('table') ?? root
  return [...container.querySelectorAll<HTMLElement>('[data-provider-sort-id]')]
}

function providerOrder(root: HTMLElement): string[] {
  return providerElements(root).map(element => element.dataset.providerSortId!)
}

function pointerEvent(type: string, clientX: number, clientY: number, options: { button?: number; pointerType?: string } = {}) {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX, clientY, button: options.button ?? 0 })
  Object.defineProperties(event, {
    pointerId: { value: 1 },
    isPrimary: { value: true },
    pointerType: { value: options.pointerType ?? 'mouse' },
  })
  return event
}

function startProviderDrag(root: HTMLElement, sourceId: string, targetId: string, pointerType = 'mouse') {
  const elements = providerElements(root)
  const handle = elements.find(element => element.dataset.providerSortId === sourceId)!
    .querySelector<HTMLButtonElement>('[data-provider-drag-handle]')!
  const target = elements.find(element => element.dataset.providerSortId === targetId)!
  const hitTest = vi.fn((): Element | null => target)
  Object.defineProperty(document, 'elementFromPoint', { configurable: true, value: hitTest })
  handle.dispatchEvent(pointerEvent('pointerdown', 40, 100, { pointerType }))
  window.dispatchEvent(pointerEvent('pointermove', 100, 200, { pointerType }))
  return { handle, target, hitTest }
}

async function dropProvider(handle: HTMLButtonElement) {
  window.dispatchEvent(pointerEvent('pointerup', 100, 200))
  handle.click()
  await settle()
}

describe('ProviderManagement shared display order', () => {
  it('drags table rows, synchronizes both card layouts, and leaves scheduling priorities unchanged', async () => {
    const providers = mockSortableProviders()
    const root = await mountView()
    const { handle, target } = startProviderDrag(root, 'provider-1', 'provider-3')
    await settle()
    expect(target.classList.contains('ring-2')).toBe(true)
    expect(providerElements(root)[0]?.classList.contains('opacity-40')).toBe(true)

    await dropProvider(handle)
    const expected = ['provider-2', 'provider-3', 'provider-1', 'provider-4']
    expect(providerOrder(root)).toEqual(expected)
    const mobileOrder = [...root.querySelectorAll<HTMLElement>('[data-provider-sort-id]')]
      .filter(element => !element.closest('table'))
      .map(element => element.dataset.providerSortId)
    expect(mobileOrder).toEqual(expected)
    expect(root.querySelector('[data-provider-detail]')).toBeNull()
    expect(apiMocks.updateProvider).not.toHaveBeenCalled()
    expect(providers.map(provider => provider.provider_priority)).toEqual([10, 20, 30, 40])

    findButton(root, '切换到卡片视图').click()
    await settle()
    expect(providerOrder(root)).toEqual(expected)
    expect(apiMocks.getProvidersSummary).toHaveBeenCalledTimes(1)
  })

  it('supports touch dragging on card headers and restores the order after refresh and remount', async () => {
    mockSortableProviders()
    localStorage.setItem('aether-provider-card-view', 'true')
    let root = await mountView()
    const { handle } = startProviderDrag(root, 'provider-4', 'provider-1', 'touch')
    await dropProvider(handle)
    const expected = ['provider-4', 'provider-1', 'provider-2', 'provider-3']
    expect(providerOrder(root)).toEqual(expected)
    expect(JSON.parse(localStorage.getItem('aether-provider-display-order')!)).toEqual(expected)

    findButton(root, '刷新').click()
    await settle()
    expect(providerOrder(root)).toEqual(expected)
    findButton(root, '切换到列表视图').click()
    await settle()
    expect(providerOrder(root)).toEqual(expected)

    unmountView()
    root = await mountView()
    expect(providerOrder(root)).toEqual(expected)
  })

  it.each(['escape', 'pointercancel', 'outside'] as const)('cancels a drag without saving when cancelled by %s', async (reason) => {
    mockSortableProviders()
    const root = await mountView()
    const original = providerOrder(root)
    const { handle, hitTest } = startProviderDrag(root, 'provider-1', 'provider-3')
    if (reason === 'escape') {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    } else if (reason === 'pointercancel') {
      window.dispatchEvent(pointerEvent('pointercancel', 100, 200))
    } else {
      hitTest.mockReturnValue(null)
    }
    await dropProvider(handle)

    expect(providerOrder(root)).toEqual(original)
    expect(JSON.parse(localStorage.getItem('aether-provider-display-order')!)).toEqual([])
    expect(root.querySelector('.opacity-40')).toBeNull()
    expect(root.querySelector('[data-provider-detail]')).toBeNull()
  })

  it('does not reorder on a handle click or a secondary mouse button', async () => {
    mockSortableProviders()
    const root = await mountView()
    const original = providerOrder(root)
    const handle = providerElements(root)[0]!.querySelector<HTMLButtonElement>('[data-provider-drag-handle]')!
    handle.dispatchEvent(pointerEvent('pointerdown', 40, 100))
    window.dispatchEvent(pointerEvent('pointermove', 42, 101))
    window.dispatchEvent(pointerEvent('pointerup', 42, 101))
    handle.click()
    handle.dispatchEvent(pointerEvent('pointerdown', 40, 100, { button: 2 }))
    window.dispatchEvent(pointerEvent('pointermove', 100, 200))
    window.dispatchEvent(pointerEvent('pointerup', 100, 200))
    await settle()

    expect(providerOrder(root)).toEqual(original)
    expect(root.querySelector('[data-provider-detail]')).toBeNull()
  })

  it('moves only visible providers while preserving filtered-out positions', async () => {
    const providers = mockSortableProviders()
    const root = await mountView()
    apiMocks.getProvidersSummary.mockResolvedValue({ items: [providers[0], providers[2]], total: 2 })
    const search = root.querySelector<HTMLInputElement>('#provider-search')!
    search.value = 'filtered'
    search.dispatchEvent(new Event('input', { bubbles: true }))
    await vi.waitFor(() => expect(providerOrder(root)).toEqual(['provider-1', 'provider-3']))

    const { handle } = startProviderDrag(root, 'provider-1', 'provider-3')
    await dropProvider(handle)
    expect(providerOrder(root)).toEqual(['provider-3', 'provider-1'])

    apiMocks.getProvidersSummary.mockResolvedValue({ items: providers, total: 4 })
    findButton(root, '重置筛选').click()
    await vi.waitFor(() => {
      expect(providerOrder(root)).toEqual(['provider-3', 'provider-2', 'provider-1', 'provider-4'])
    })
  })

  it('supports keyboard ordering and keeps focus on the moved handle', async () => {
    mockSortableProviders()
    const root = await mountView()
    const handle = providerElements(root)[0]!.querySelector<HTMLButtonElement>('[data-provider-drag-handle]')!
    handle.focus()
    handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }))
    await settle()

    expect(providerOrder(root)).toEqual(['provider-2', 'provider-1', 'provider-3', 'provider-4'])
    expect(document.activeElement).toBe(handle)
    expect(root.querySelector('[role="status"]')?.textContent).toContain('展示顺序已更新')
    expect(root.querySelector('[data-provider-detail]')).toBeNull()

    handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true, cancelable: true }))
    await settle()
    expect(providerOrder(root)).toEqual(['provider-1', 'provider-2', 'provider-3', 'provider-4'])
  })

  it('preserves saved ordering on other pages when reordering the current page', async () => {
    const providers = mockSortableProviders()
    apiMocks.getProvidersSummary.mockImplementation(async ({ page }: { page: number }) => ({
      items: page === 1 ? providers.slice(0, 2) : providers.slice(2),
      total: 40,
    }))
    const root = await mountView()
    const firstDrag = startProviderDrag(root, 'provider-1', 'provider-2')
    await dropProvider(firstDrag.handle)

    const secondPage = [...root.querySelectorAll<HTMLButtonElement>('button')]
      .find(button => button.textContent?.trim() === '2')!
    secondPage.click()
    await settle()
    expect(providerOrder(root)).toEqual(['provider-3', 'provider-4'])
    const secondDrag = startProviderDrag(root, 'provider-4', 'provider-3')
    await dropProvider(secondDrag.handle)
    expect(providerOrder(root)).toEqual(['provider-4', 'provider-3'])

    const firstPage = [...root.querySelectorAll<HTMLButtonElement>('button')]
      .find(button => button.textContent?.trim() === '1')!
    firstPage.click()
    await settle()
    expect(providerOrder(root)).toEqual(['provider-2', 'provider-1'])
    expect(JSON.parse(localStorage.getItem('aether-provider-display-order')!))
      .toEqual(['provider-2', 'provider-1', 'provider-4', 'provider-3'])
  })

  it('ignores stale IDs and appends providers that are not in the saved order', async () => {
    mockSortableProviders()
    localStorage.setItem('aether-provider-display-order', JSON.stringify(['deleted-provider', 'provider-3', 'provider-1']))
    const root = await mountView()
    expect(providerOrder(root)).toEqual(['provider-3', 'provider-1', 'provider-2', 'provider-4'])
  })
})
