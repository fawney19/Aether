import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import ServerUserSelector from '../ServerUserSelector.vue'

const getAllUsersMock = vi.hoisted(() => vi.fn())

vi.mock('@/api/users', () => ({
  usersApi: {
    getAllUsers: getAllUsersMock,
  },
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    Input: defineComponent({
      name: 'InputStub',
      props: { modelValue: String },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('input', {
          ...attrs,
          value: props.modelValue ?? '',
          onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
        })
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    Check: Icon,
    ChevronDown: Icon,
    Search: Icon,
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function flushPromises() {
  return Promise.resolve().then(() => undefined)
}

function mountSelector(props: Record<string, unknown> = {}) {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(defineComponent({
    setup() {
      return () => h(ServerUserSelector, {
        modelValue: '__all__',
        initialUsers: [],
        dropdown: true,
        ...props,
      })
    },
  }))

  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

beforeEach(() => {
  vi.useRealTimers()
  getAllUsersMock.mockReset()
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.useRealTimers()
})

describe('ServerUserSelector', () => {
  it('reports load failures and retries without changing the selection', async () => {
    getAllUsersMock.mockRejectedValueOnce(new Error('unavailable')).mockResolvedValueOnce([{ id: 'user-1', username: 'alice', email: '' }])
    const root = mountSelector({ label: '邀请人', placeholder: '邀请人' })
    const trigger = root.querySelector<HTMLButtonElement>('[aria-label="邀请人"]')!
    trigger.click(); await nextTick(); await flushPromises()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('用户加载失败')
    root.querySelector<HTMLButtonElement>('[role="alert"] button')!.click(); await nextTick(); await flushPromises()
    expect(root.querySelector('[role="alert"]')).toBeNull()
    expect(root.textContent).toContain('alice')
    expect(trigger.textContent).toContain('邀请人')
  })
  it('loads the initial user batch when opened', async () => {
    getAllUsersMock.mockResolvedValue([
      { id: 'user-1', username: 'alice', email: 'alice@example.com' },
    ])
    const root = mountSelector()

    root.querySelector('button')?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await nextTick()
    await flushPromises()

    expect(getAllUsersMock).toHaveBeenCalledWith({
      search: '',
      skip: 0,
      limit: 50,
      cacheTtlMs: 30_000,
    })
    expect(root.textContent).toContain('alice')
  })

  it('debounces remote search and bypasses cache for typed queries', async () => {
    vi.useFakeTimers()
    getAllUsersMock.mockResolvedValue([])
    const root = mountSelector()

    root.querySelector('button')?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await nextTick()
    const input = root.querySelector('input') as HTMLInputElement
    input.value = 'bob'
    input.dispatchEvent(new Event('input', { bubbles: true }))

    await vi.advanceTimersByTimeAsync(299)
    expect(getAllUsersMock).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    await flushPromises()

    expect(getAllUsersMock).toHaveBeenLastCalledWith({
      search: 'bob',
      skip: 0,
      limit: 50,
      cacheTtlMs: 0,
    })
  })

  it('keeps the selected user pinned when search results do not include it', async () => {
    getAllUsersMock.mockResolvedValue([
      { id: 'user-1', username: 'alice', email: 'alice@example.com' },
    ])
    const root = mountSelector({
      modelValue: 'user-99',
      initialUsers: [
        { id: 'user-99', username: 'pinned', email: 'pinned@example.com' },
      ],
    })

    root.querySelector('button')?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await nextTick()
    await flushPromises()

    expect(root.textContent).toContain('pinned')
    expect(root.textContent).toContain('alice')
  })
})
