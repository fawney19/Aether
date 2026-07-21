import { describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import AntigravityQuotaDialog from '@/features/providers/components/AntigravityQuotaDialog.vue'
import type { UpstreamMetadata } from '@/api/endpoints/types'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string) => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h('div', [
        slots.headerActions?.(),
        slots.default?.(),
        slots.footer?.(),
      ])
    },
  })

  return {
    Dialog: passthrough('DialogStub'),
    DropdownMenu: passthrough('DropdownMenuStub'),
    DropdownMenuTrigger: passthrough('DropdownMenuTriggerStub'),
    DropdownMenuContent: passthrough('DropdownMenuContentStub'),
    DropdownMenuItem: defineComponent({
      name: 'DropdownMenuItemStub',
      emits: ['select'],
      setup(_, { emit, slots }) {
        return () => h('button', { type: 'button', onClick: () => emit('select') }, slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/button.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'ButtonStub',
      setup(_, { attrs, slots }) {
        return () => h('button', { ...attrs, type: 'button' }, slots.default?.())
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
    BarChart3: Icon,
    Loader2: Icon,
    Play: Icon,
  }
})

vi.mock('@/api/endpoints/providers', () => ({
  testModel: vi.fn(),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
  }),
}))

vi.mock('@/utils/errorParser', () => ({
  parseApiError: (value: unknown) => String(value),
}))

function mount(metadata: UpstreamMetadata) {
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(defineComponent({
    setup() {
      return () => h(AntigravityQuotaDialog, {
        open: true,
        metadata,
        keyName: 'Key-1',
      })
    },
  }))
  app.mount(root)

  return {
    root,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

describe('AntigravityQuotaDialog', () => {
  it('renders opaque quota identifiers with concise visible labels', () => {
    const rawIdentifier = 'RateLimitResetCredit_05cbb6eeeb9c81918e011d8300f9ebfb'
    const { root, unmount } = mount({
      antigravity: {
        quota_by_model: {
          [rawIdentifier]: {
            display_name: rawIdentifier,
            remaining_fraction: 0.25,
            used_percent: 75,
          },
        },
      },
    })

    expect(root.textContent).toContain('Key-1')
    expect(root.textContent).not.toContain(rawIdentifier)
    expect(root.textContent).toContain('25.0%')

    unmount()
  })

  it('orders important Gemini and Claude quota rows before low-priority rows', () => {
    const { root, unmount } = mount({
      antigravity: {
        quota_by_model: {
          tab_flash_lite_preview: {
            display_name: 'Tab Flash Lite Preview',
            remaining_fraction: 0.01,
            used_percent: 99,
          },
          'gemini-3-flash-agent': {
            display_name: 'Gemini 3.5 Flash (High)',
            remaining_fraction: 0.9,
            used_percent: 10,
          },
          'gemini-pro-agent': {
            display_name: 'gemini-pro-agent',
            remaining_fraction: 0.95,
            used_percent: 5,
          },
          'gemini-3.5-flash-low': {
            display_name: 'Gemini 3.5 Flash (Medium)',
            remaining_fraction: 0.8,
            used_percent: 20,
          },
          'claude-opus-4-6-thinking': {
            display_name: 'Claude Opus 4.6 (Thinking)',
            remaining_fraction: 1,
            used_percent: 0,
          },
          chat_20706: {
            display_name: 'chat_20706',
            remaining_fraction: 0,
            used_percent: 100,
          },
        },
      },
    })
    const text = root.textContent || ''

    expect(text).not.toContain('gemini-pro-agent')
    expect(text).not.toContain('claude-opus-4-6-thinking')
    expect(text.indexOf('Claude Opus 4.6 (Thinking)')).toBeLessThan(text.indexOf('Gemini 3.1 Pro (High)'))
    expect(text.indexOf('Gemini 3.1 Pro (High)')).toBeLessThan(text.indexOf('Gemini 3.5 Flash (High)'))
    expect(text.indexOf('Gemini 3.5 Flash (High)')).toBeLessThan(text.indexOf('Gemini 3.5 Flash (Medium)'))
    expect(text.indexOf('Gemini 3.5 Flash (Medium)')).toBeLessThan(text.indexOf('Tab Flash Lite Preview'))
    expect(text.indexOf('Tab Flash Lite Preview')).toBeLessThan(text.indexOf('chat_20706'))

    unmount()
  })

  it('renders one row for duplicate quota labels and keeps the preferred active bucket', () => {
    const { root, unmount } = mount({
      antigravity: {
        quota_by_model: {
          'gemini-3.1-pro-high': {
            display_name: 'Gemini 3.1 Pro (High)',
            remaining_fraction: 0.95,
            used_percent: 5,
          },
          'gemini-pro-agent': {
            display_name: 'Gemini 3.1 Pro (High)',
            remaining_fraction: 0.4,
            used_percent: 60,
          },
          'gemini-3-flash-agent': {
            display_name: 'Gemini 3.5 Flash (High)',
            remaining_fraction: 0.9,
            used_percent: 10,
          },
        },
      },
    })
    const text = root.textContent || ''

    expect(text.match(/Gemini 3\.1 Pro \(High\)/g)).toHaveLength(1)
    expect(text).toContain('40.0%')
    expect(text).not.toContain('95.0%')
    expect(text.indexOf('Gemini 3.1 Pro (High)')).toBeLessThan(text.indexOf('Gemini 3.5 Flash (High)'))

    unmount()
  })
})
